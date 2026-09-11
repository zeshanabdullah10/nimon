//! End-to-end integration tests for the self-healing pipeline:
//! AlertManager → ActionExecutor → edge session → action_result correlation,
//! plus alert persistence and the ack-after-restart flow.

use std::sync::Arc;
use std::time::Duration;

use actix::prelude::*;
use tokio::sync::mpsc;

use nimon_core::actor::messages::{
    ActionResult, DeviceStatusUpdate, ExecuteAction as WireExecuteAction,
};
use nimon_core::alert::rules::{AlertRule, ComparisonOp, RuleCondition};
use nimon_core::alert::{Alert, AlertStatus};
use nimon_core::db::AlertRepository;
use nimon_core::protocol::WsMessageType;
use nimon_core::{HealthStatus, MetricValue, Severity};
use nimon_hub::action::actions::{Action, ActionStatus, ActionType};
use nimon_hub::action::executor::{ActionContext, CompleteAction};
use nimon_hub::alert::manager::{AlertManager, AlertManagerConfig, GetActiveAlerts, ResolveAlert};
use nimon_hub::session::{EdgeSession, SessionStore};

async fn test_db() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
    nimon_core::db::init_database(&pool).await.unwrap();
    // sqlx enables foreign_keys=ON: seed the edge/device registry rows
    // that alerts and action_history reference.
    let edges = nimon_core::db::edge_repo::EdgeRepository::new(&pool);
    edges
        .upsert(&nimon_core::EdgeNode {
            id: "edge-1".to_string(),
            name: "Edge 1".to_string(),
            hostname: None,
            ip_address: None,
            last_seen: None,
            status: nimon_core::EdgeStatus::Online,
        })
        .await
        .unwrap();
    let devices = nimon_core::db::device_repo::DeviceRepository::new(&pool);
    devices
        .upsert(&nimon_core::Device {
            id: "edge-1:daq-1".to_string(),
            edge_id: "edge-1".to_string(),
            device_name: "daq-1".to_string(),
            device_type: nimon_core::DeviceType::Daq,
            model: None,
            serial_number: None,
            firmware_version: None,
            driver_version: None,
            ip_address: None,
            slot: None,
            chassis: None,
            is_simulated: false,
        })
        .await
        .unwrap();
    pool
}

fn hot_rule_with_action() -> AlertRule {
    AlertRule {
        id: "rule-hot-action".to_string(),
        name: "Hot with action".to_string(),
        description: "temperature above 80 with action".to_string(),
        enabled: true,
        severity: Severity::Critical,
        condition: RuleCondition::MetricThreshold {
            metric_name: "temperature".to_string(),
            operator: ComparisonOp::GreaterThan,
            threshold: 80.0,
            duration_minutes: None,
        },
        cooldown_minutes: 0,
        notification_channels: vec![],
        suppress_repeat: false,
        max_firing_count: None,
        action: Some(nimon_core::alert::rules::ActionRef::PowerCycle { delay_secs: None }),
    }
}

fn status_update(device: &str, temp: f64) -> DeviceStatusUpdate {
    let mut metrics = std::collections::HashMap::new();
    metrics.insert("temperature".to_string(), MetricValue::Float(temp));
    DeviceStatusUpdate {
        device_id: device.to_string(),
        edge_id: "edge-1".to_string(),
        status: HealthStatus::Healthy,
        metrics,
        timestamp: chrono::Utc::now(),
        is_simulated: false,
    }
}

#[actix::test]
async fn edge_command_result_is_correlated_and_persisted() {
    let sessions = Arc::new(SessionStore::new());
    let (tx, mut rx) = mpsc::unbounded_channel();
    sessions.add(EdgeSession::with_outbound(
        "edge-1".to_string(),
        "Edge 1".to_string(),
        None,
        None,
        tx,
    ));

    let pool = test_db().await;
    let executor = nimon_hub::action::ActionExecutor::new()
        .with_sessions(sessions.clone())
        .with_db_pool(pool.clone())
        .start();

    let mut action = Action::edge_command("act-1", "Test command", "integration test");
    if let ActionType::EdgeCommand {
        command,
        parameters,
    } = &mut action.action_type
    {
        *command = "resend_state".to_string();
        parameters.insert("reason".to_string(), "integration".to_string());
    }
    action.timeout_secs = 5;
    action.retry_config.max_attempts = 1;

    let exec_addr = executor.clone();
    let exec_task = tokio::task::spawn_local(async move {
        exec_addr
            .send(nimon_hub::action::executor::ExecuteAction {
                action,
                context: ActionContext::new("edge-1", "edge-1:daq-1", ""),
            })
            .await
            .unwrap()
    });

    // The action must arrive on the edge's outbound channel as
    // execute_action with the EdgeCommand parameters
    let wire = rx.recv().await.expect("wire message");
    assert_eq!(wire.msg_type, WsMessageType::ExecuteAction);
    let payload: WireExecuteAction = wire.payload().unwrap();
    assert_eq!(payload.action_id, "act-1");
    assert_eq!(payload.device_id, "edge-1:daq-1");

    // Simulate the edge completing the action: correlate by msg_id
    executor
        .send(CompleteAction {
            msg_id: wire.msg_id.clone(),
            result: ActionResult {
                action_id: "act-1".to_string(),
                success: true,
                output: Some("done".to_string()),
                error: None,
                exit_code: Some(0),
                duration_ms: 12,
            },
        })
        .await
        .unwrap();

    let result = exec_task.await.unwrap();
    assert_eq!(result.status, ActionStatus::Succeeded);
    assert_eq!(result.exit_code, Some(0));
    assert!(result.output.contains("done"));

    // The execution is recorded in action_history
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM action_history WHERE action_id = 'act-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "action result must be persisted");
}

#[actix::test]
async fn edge_action_to_offline_edge_fails_promptly() {
    // No session for edge-1: the executor must fail fast, not hang.
    let sessions = Arc::new(SessionStore::new());
    let executor = nimon_hub::action::ActionExecutor::new()
        .with_sessions(sessions)
        .start();

    let action = Action::power_cycle("act-2", "Power cycle", "integration test");
    let result = executor
        .send(nimon_hub::action::executor::ExecuteAction {
            action,
            context: ActionContext::new("edge-1", "edge-1:daq-1", ""),
        })
        .await
        .unwrap();

    assert_eq!(result.status, ActionStatus::Failed);
    assert!(result.error.contains("not connected"));
}

#[actix::test]
async fn rule_fires_action_and_persists_alert_outcome() {
    let sessions = Arc::new(SessionStore::new());
    let (tx, mut rx) = mpsc::unbounded_channel();
    sessions.add(EdgeSession::with_outbound(
        "edge-1".to_string(),
        "Edge 1".to_string(),
        None,
        None,
        tx,
    ));

    let pool = test_db().await;
    let executor = nimon_hub::action::ActionExecutor::new()
        .with_sessions(sessions.clone())
        .with_db_pool(pool.clone())
        .start();

    let mut config = AlertManagerConfig::default();
    config.rules = vec![hot_rule_with_action()];
    let manager = AlertManager::new(config)
        .with_action_executor(executor.clone())
        .with_db_pool(pool.clone())
        .with_sessions(sessions.clone())
        .start();

    // Hot device: rule fires, action is dispatched to the edge
    manager
        .send(status_update("edge-1:daq-1", 90.0))
        .await
        .unwrap();

    // Wait for the alert to appear and the wire action to be dispatched
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let alerts = loop {
        let alerts = manager.send(GetActiveAlerts).await.unwrap();
        if !alerts.is_empty() || std::time::Instant::now() > deadline {
            break alerts;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(alerts.len(), 1, "rule with action must fire");
    assert_eq!(alerts[0].rule_id, "rule-hot-action");

    let wire = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timed out waiting for edge action")
        .expect("channel closed");
    assert_eq!(wire.msg_type, WsMessageType::ExecuteAction);

    // Complete the edge action; the alert outcome is recorded in the DB
    executor
        .send(CompleteAction {
            msg_id: wire.msg_id.clone(),
            result: ActionResult {
                action_id: "act-1".to_string(),
                success: true,
                output: None,
                error: None,
                exit_code: Some(0),
                duration_ms: 5,
            },
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;
    let repo = AlertRepository::new(&pool);
    let recent = repo.list_recent(10).await.unwrap();
    assert_eq!(recent.len(), 1, "alert must be persisted");
    assert_eq!(
        recent[0].action_taken.as_deref(),
        Some("power_cycle"),
        "rule action outcome recorded on the alert"
    );
}

#[actix::test]
async fn acknowledge_works_for_restart_restored_alerts() {
    let pool = test_db().await;

    // Simulate a firing alert persisted by a previous hub process
    let repo = AlertRepository::new(&pool);
    let alert = Alert {
        id: "01INTEGRATIONTEST".to_string(),
        rule_id: "rule-high-temp".to_string(),
        edge_id: "edge-1".to_string(),
        device_id: "edge-1:daq-1".to_string(),
        severity: Severity::Warning,
        status: AlertStatus::Firing,
        title: "High Temperature: edge-1:daq-1".to_string(),
        message: "test".to_string(),
        metric_name: Some("temperature".to_string()),
        metric_value: Some(90.0),
        threshold: Some(70.0),
        triggered_at: chrono::Utc::now(),
        resolved_at: None,
        fired_count: 1,
        notification_sent: false,
    };
    repo.insert(&alert).await.unwrap();

    let mut config = AlertManagerConfig::default();
    config.rules = vec![AlertRule {
        id: "rule-high-temp".to_string(),
        name: "High Temperature".to_string(),
        description: String::new(),
        enabled: true,
        severity: Severity::Warning,
        condition: RuleCondition::MetricThreshold {
            metric_name: "temperature".to_string(),
            operator: ComparisonOp::GreaterThan,
            threshold: 70.0,
            duration_minutes: None,
        },
        cooldown_minutes: 5,
        notification_channels: vec![],
        suppress_repeat: false,
        max_firing_count: None,
        action: None,
    }];
    let manager = AlertManager::new(config).with_db_pool(pool.clone()).start();

    // The alert is not in the in-memory registry (fresh start), but ack
    // must still succeed against the database row.
    let result = manager
        .send(ResolveAlert {
            alert_id: "01INTEGRATIONTEST".to_string(),
        })
        .await
        .unwrap();
    assert!(result.is_ok(), "restart-restored alerts must be ackable");

    let recent = repo.list_recent(10).await.unwrap();
    assert_eq!(recent[0].status, "resolved");
    assert!(recent[0].resolved_at.is_some());

    // Acking an unknown alert reports not-found
    let result = manager
        .send(ResolveAlert {
            alert_id: "01UNKNOWN".to_string(),
        })
        .await
        .unwrap();
    assert!(result.is_err());
}

#[actix::test]
async fn protocol_version_gate_accepts_same_major() {
    // The WS loop rejects different major versions; here we pin the
    // core compatibility helper contract used by that gate.
    assert!(nimon_core::protocol::is_compatible("1.0"));
    assert!(nimon_core::protocol::is_compatible("1.7"));
    assert!(!nimon_core::protocol::is_compatible("2.0"));
}
