//! End-to-end integration tests for the self-healing pipeline:
//! AlertManager → ActionExecutor → edge session → action_result correlation,
//! alert persistence (re-fires, acknowledge, restart restore) and the
//! WebSocket protocol handling against a real listening hub.

use std::sync::Arc;
use std::time::Duration;

use actix::prelude::*;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as TMessage;

use nimon_core::actor::messages::{
    ActionResult, ActionType as WireActionType, DeviceStatusUpdate, EdgeRegister,
    ExecuteAction as WireExecuteAction,
};
use nimon_core::alert::rules::{AlertRule, ComparisonOp, RuleCondition};
use nimon_core::alert::{Alert, AlertStatus};
use nimon_core::db::AlertRepository;
use nimon_core::protocol::{WsMessage, WsMessageType};
use nimon_core::{HealthStatus, MetricValue, Severity};
use nimon_hub::action::actions::{Action, ActionStatus, ActionType};
use nimon_hub::action::executor::{ActionContext, CompleteAction};
use nimon_hub::alert::manager::{
    AcknowledgeAlert, AlertManager, AlertManagerConfig, GetActiveAlerts, ResolveAlert,
};
use nimon_hub::db_writer::DbWriter;
use nimon_hub::session::{EdgeSession, SessionStore};

async fn test_db() -> sqlx::SqlitePool {
    let pool = nimon_core::db::connect(":memory:").await.unwrap();
    // Foreign keys are ON: seed the edge/device registry rows that alerts
    // and action_history reference.
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

fn hot_rule(id: &str, cooldown_minutes: i32) -> AlertRule {
    AlertRule {
        id: id.to_string(),
        name: "Hot".to_string(),
        description: "temperature above 80".to_string(),
        enabled: true,
        severity: Severity::Critical,
        condition: RuleCondition::MetricThreshold {
            metric_name: "temperature".to_string(),
            operator: ComparisonOp::GreaterThan,
            threshold: 80.0,
            duration_minutes: None,
            hysteresis: None,
        },
        cooldown_minutes,
        notification_channels: vec![],
        suppress_repeat: false,
        max_firing_count: None,
        action: None,
    }
}

fn hot_rule_with_action() -> AlertRule {
    AlertRule {
        action: Some(nimon_core::alert::rules::ActionRef::PowerCycle { delay_secs: None }),
        ..hot_rule("rule-hot-action", 0)
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

fn connected_edge(sessions: &SessionStore) -> mpsc::Receiver<WsMessage> {
    let (tx, rx) = mpsc::channel(64);
    sessions.add(EdgeSession::with_outbound(
        "edge-1".to_string(),
        "Edge 1".to_string(),
        None,
        None,
        tx,
    ));
    rx
}

fn ok_result(action_id: &str) -> ActionResult {
    ActionResult {
        action_id: action_id.to_string(),
        success: true,
        output: Some("done".to_string()),
        error: None,
        exit_code: Some(0),
        duration_ms: 12,
    }
}

async fn wait_until<F, Fut>(mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !check().await {
        assert!(
            std::time::Instant::now() < deadline,
            "condition not reached in time"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[actix::test]
async fn edge_command_result_is_correlated_via_reply_to_and_persisted() {
    let sessions = Arc::new(SessionStore::new());
    let mut rx = connected_edge(&sessions);

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

    // An unknown command runs as an allowlisted edge script of that name
    let request = rx.recv().await.expect("wire message");
    assert_eq!(request.msg_type, WsMessageType::ExecuteAction);
    let payload: WireExecuteAction = request.payload().unwrap();
    assert_eq!(payload.action_id, "act-1");
    assert_eq!(payload.device_id, "edge-1:daq-1");
    assert_eq!(payload.action_type, WireActionType::CustomScript);
    assert_eq!(payload.parameters.get("script").unwrap(), "resend_state");

    // The edge replies with its OWN msg_id and reply_to = request id; the
    // hub correlates on correlation_id()
    let reply = WsMessage::action_result(ok_result("act-1")).with_reply_to(request.msg_id.clone());
    assert_ne!(reply.msg_id, request.msg_id);
    executor
        .send(CompleteAction {
            edge_id: "edge-1".to_string(),
            msg_id: reply.correlation_id().to_string(),
            result: reply.into_payload().unwrap(),
        })
        .await
        .unwrap();

    let result = exec_task.await.unwrap();
    assert_eq!(result.status, ActionStatus::Succeeded);
    assert_eq!(result.exit_code, Some(0));
    assert!(result.output.contains("done"));

    wait_until(|| {
        let pool = pool.clone();
        async move {
            let (count,): (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM action_history WHERE action_id = 'act-1'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            count == 1
        }
    })
    .await;
}

#[actix::test]
async fn edge_result_echoing_request_msg_id_still_correlates() {
    let sessions = Arc::new(SessionStore::new());
    let mut rx = connected_edge(&sessions);
    let executor = nimon_hub::action::ActionExecutor::new()
        .with_sessions(sessions.clone())
        .start();

    let mut action = Action::power_cycle("act-echo", "Power cycle", "integration test");
    action.timeout_secs = 5;
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

    let request = rx.recv().await.expect("wire message");
    let payload: WireExecuteAction = request.payload().unwrap();
    assert_eq!(payload.action_type, WireActionType::PowerCycle);

    // Legacy peer: no reply_to, but its msg_id equals the request id
    let mut reply = WsMessage::action_result(ok_result("act-echo"));
    reply.msg_id = request.msg_id.clone();
    assert!(reply.reply_to.is_none());
    executor
        .send(CompleteAction {
            edge_id: "edge-1".to_string(),
            msg_id: reply.correlation_id().to_string(),
            result: reply.into_payload().unwrap(),
        })
        .await
        .unwrap();

    assert_eq!(exec_task.await.unwrap().status, ActionStatus::Succeeded);
}

#[actix::test]
async fn edge_action_timeout_is_not_retried() {
    let sessions = Arc::new(SessionStore::new());
    let mut rx = connected_edge(&sessions);
    let executor = nimon_hub::action::ActionExecutor::new()
        .with_sessions(sessions.clone())
        .start();

    let mut action = Action::edge_command("act-slow", "Slow", "integration test");
    action.action_type = ActionType::EdgeCommand {
        command: "reset_driver".to_string(),
        parameters: Default::default(),
    };
    action.timeout_secs = 1;
    action.retry_config.max_attempts = 3;
    action.retry_config.delay_ms = 10;

    let result = executor
        .send(nimon_hub::action::executor::ExecuteAction {
            action,
            context: ActionContext::new("edge-1", "edge-1:daq-1", ""),
        })
        .await
        .unwrap();
    assert_eq!(result.status, ActionStatus::Timeout);

    let first = rx.try_recv().expect("dispatched once");
    let payload: WireExecuteAction = first.payload().unwrap();
    assert_eq!(payload.action_type, WireActionType::ResetDriver);
    assert!(
        rx.try_recv().is_err(),
        "a timed-out edge action must not be re-dispatched"
    );
}

#[actix::test]
async fn edge_action_to_offline_edge_fails_promptly() {
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
    let mut rx = connected_edge(&sessions);

    let pool = test_db().await;
    let writer = DbWriter::spawn(pool.clone());
    let executor = nimon_hub::action::ActionExecutor::new()
        .with_sessions(sessions.clone())
        .with_db_writer(writer.clone())
        .start();

    let config = AlertManagerConfig {
        rules: vec![hot_rule_with_action()],
        ..Default::default()
    };
    let manager = AlertManager::new(config)
        .with_action_executor(executor.clone())
        .with_db_writer(writer.clone())
        .with_sessions(sessions.clone())
        .start();

    manager
        .send(status_update("edge-1:daq-1", 90.0))
        .await
        .unwrap();

    let alerts = manager.send(GetActiveAlerts).await.unwrap();
    assert_eq!(alerts.len(), 1, "rule with action must fire");
    assert_eq!(alerts[0].rule_id, "rule-hot-action");

    let wire = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timed out waiting for edge action")
        .expect("channel closed");
    assert_eq!(wire.msg_type, WsMessageType::ExecuteAction);

    let reply = WsMessage::action_result(ok_result("rule-power-cycle")).with_reply_to(wire.msg_id);
    executor
        .send(CompleteAction {
            edge_id: "edge-1".to_string(),
            msg_id: reply.correlation_id().to_string(),
            result: reply.into_payload().unwrap(),
        })
        .await
        .unwrap();

    let repo_pool = pool.clone();
    wait_until(move || {
        let pool = repo_pool.clone();
        async move {
            let recent = AlertRepository::new(&pool).list_recent(10).await.unwrap();
            recent.len() == 1 && recent[0].action_taken.as_deref() == Some("power_cycle")
        }
    })
    .await;
    let recent = AlertRepository::new(&pool).list_recent(10).await.unwrap();
    assert_eq!(recent[0].notification_sent, 1);
}

#[actix::test]
async fn refire_updates_the_same_row() {
    let pool = test_db().await;
    let writer = DbWriter::spawn(pool.clone());
    let config = AlertManagerConfig {
        rules: vec![hot_rule("rule-refire", 0)],
        ..Default::default()
    };
    let manager = AlertManager::new(config)
        .with_db_writer(writer.clone())
        .start();

    for temp in [90.0, 92.0, 95.0] {
        manager
            .send(status_update("edge-1:daq-1", temp))
            .await
            .unwrap();
    }
    writer.flush().await;

    let records = AlertRepository::new(&pool).list_recent(10).await.unwrap();
    assert_eq!(records.len(), 1, "re-fires must not insert new rows");
    assert_eq!(records[0].fired_count, 3);
    assert_eq!(records[0].metric_value, Some(95.0));
    assert!(records[0].last_fired_at.is_some());
}

fn persisted_alert(id: &str) -> Alert {
    Alert {
        id: id.to_string(),
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
    }
}

fn high_temp_config() -> AlertManagerConfig {
    AlertManagerConfig {
        rules: vec![AlertRule {
            condition: threshold_70(),
            ..hot_rule("rule-high-temp", 5)
        }],
        ..Default::default()
    }
}

#[actix::test]
async fn restart_restores_active_alerts_and_resolve_works() {
    let pool = test_db().await;
    let repo = AlertRepository::new(&pool);
    repo.insert(&persisted_alert("01INTEGRATIONTEST"))
        .await
        .unwrap();

    let manager = AlertManager::new(high_temp_config())
        .with_db_pool(pool.clone())
        .start();

    // Restored into the active set: visible, and a hot report re-fires
    // the SAME alert instead of creating a duplicate
    let active = manager.send(GetActiveAlerts).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "01INTEGRATIONTEST");
    manager
        .send(status_update("edge-1:daq-1", 95.0))
        .await
        .unwrap();
    let active = manager.send(GetActiveAlerts).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "01INTEGRATIONTEST");

    manager
        .send(ResolveAlert {
            alert_id: "01INTEGRATIONTEST".to_string(),
        })
        .await
        .unwrap()
        .expect("restored alerts must be resolvable");
    let record = repo.get("01INTEGRATIONTEST").await.unwrap();
    assert_eq!(record.status, "resolved");
    assert!(record.resolved_at.is_some());
    assert!(manager.send(GetActiveAlerts).await.unwrap().is_empty());

    // Unknown alert: not found
    assert!(manager
        .send(ResolveAlert {
            alert_id: "01UNKNOWN".to_string(),
        })
        .await
        .unwrap()
        .is_err());
}

#[actix::test]
async fn acknowledge_persists_and_survives_restart() {
    let pool = test_db().await;
    let repo = AlertRepository::new(&pool);
    repo.insert(&persisted_alert("01ACKME")).await.unwrap();

    let manager = AlertManager::new(high_temp_config())
        .with_db_pool(pool.clone())
        .start();
    let acked = manager
        .send(AcknowledgeAlert {
            alert_id: "01ACKME".to_string(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(acked.status, AlertStatus::Acknowledged);
    assert_eq!(repo.get("01ACKME").await.unwrap().status, "acknowledged");
    drop(manager);

    // A fresh manager (hub restart) restores it as active + acknowledged
    let manager = AlertManager::new(high_temp_config())
        .with_db_pool(pool.clone())
        .start();
    let active = manager.send(GetActiveAlerts).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].status, AlertStatus::Acknowledged);
}

fn offline_rule(id: &str) -> AlertRule {
    AlertRule {
        condition: RuleCondition::DeviceOffline {
            max_minutes_since_poll: 5,
        },
        suppress_repeat: true,
        ..hot_rule(id, 5)
    }
}

fn persisted(id: &str, rule_id: &str, device_id: &str) -> Alert {
    Alert {
        rule_id: rule_id.to_string(),
        device_id: device_id.to_string(),
        title: format!("{}: {}", rule_id, device_id),
        metric_name: None,
        metric_value: None,
        threshold: None,
        ..persisted_alert(id)
    }
}

async fn db_status(pool: &sqlx::SqlitePool, id: &str) -> String {
    AlertRepository::new(pool).get(id).await.unwrap().status
}

#[actix::test]
async fn restart_does_not_resolve_offline_alerts_of_dead_edges() {
    let pool = test_db().await;
    // The device last reported (healthy) 10 minutes ago; its edge is gone
    nimon_core::db::device_repo::DeviceRepository::new(&pool)
        .upsert_status(&nimon_core::DeviceStatus {
            device_id: "edge-1:daq-1".to_string(),
            status: HealthStatus::Healthy,
            last_poll: chrono::Utc::now() - chrono::Duration::minutes(10),
            metrics: Default::default(),
            error_message: None,
            error_count: 0,
            uptime_seconds: 0,
        })
        .await
        .unwrap();
    AlertRepository::new(&pool)
        .insert(&persisted("01OFFLINE", "offline", "edge-1:daq-1"))
        .await
        .unwrap();

    let config = AlertManagerConfig {
        rules: vec![offline_rule("offline")],
        ..Default::default()
    };
    let manager = AlertManager::new(config).with_db_pool(pool.clone()).start();
    for _ in 0..2 {
        manager
            .send(nimon_hub::alert::manager::RunSweep)
            .await
            .unwrap();
    }
    let active = manager.send(GetActiveAlerts).await.unwrap();
    assert_eq!(active.len(), 1, "restored state must not clear the alert");
    assert_eq!(active[0].id, "01OFFLINE");
    assert_eq!(active[0].fired_count, 1, "no re-fire either");
    assert_eq!(db_status(&pool, "01OFFLINE").await, "firing");

    // A live report does resolve it
    manager
        .send(status_update("edge-1:daq-1", 20.0))
        .await
        .unwrap();
    assert!(manager.send(GetActiveAlerts).await.unwrap().is_empty());
    wait_until(|| {
        let pool = pool.clone();
        async move { db_status(&pool, "01OFFLINE").await == "resolved" }
    })
    .await;
}

#[actix::test]
async fn restore_resolves_orphaned_rules_and_seeds_expired_devices() {
    let pool = test_db().await;
    // Registered but never polled (no restorable state)
    nimon_core::db::device_repo::DeviceRepository::new(&pool)
        .upsert_snapshot("edge-1:gone", "edge-1")
        .await
        .unwrap();
    let repo = AlertRepository::new(&pool);
    // Rule renamed since: nothing can ever resolve it
    repo.insert(&persisted("01RENAMED", "old-rule-name", "edge-1:daq-1"))
        .await
        .unwrap();
    // Dynamic families are kept
    repo.insert(&persisted("01EDGEALERT", "edge:fan", "edge-1:daq-1"))
        .await
        .unwrap();
    // Configured rule on a device without restorable state
    repo.insert(&persisted("01HOTGONE", "rule-hot", "edge-1:gone"))
        .await
        .unwrap();

    let config = AlertManagerConfig {
        rules: vec![hot_rule("rule-hot", 5), offline_rule("offline")],
        ..Default::default()
    };
    let manager = AlertManager::new(config).with_db_pool(pool.clone()).start();
    let ids = |alerts: &[Alert]| {
        let mut ids: Vec<String> = alerts.iter().map(|a| a.id.clone()).collect();
        ids.sort();
        ids
    };
    let active = manager.send(GetActiveAlerts).await.unwrap();
    assert_eq!(ids(&active), vec!["01EDGEALERT", "01HOTGONE"]);
    wait_until(|| {
        let pool = pool.clone();
        async move { db_status(&pool, "01RENAMED").await == "resolved" }
    })
    .await;

    // The seeded device is evaluated by sweeps: it is offline
    manager
        .send(nimon_hub::alert::manager::RunSweep)
        .await
        .unwrap();
    let active = manager.send(GetActiveAlerts).await.unwrap();
    assert_eq!(active.len(), 3, "{:?}", active);
    assert!(active
        .iter()
        .any(|a| a.rule_id == "offline" && a.device_id == "edge-1:gone"));
    assert!(active.iter().any(|a| a.id == "01HOTGONE"));
}

fn threshold_70() -> RuleCondition {
    RuleCondition::MetricThreshold {
        metric_name: "temperature".to_string(),
        operator: ComparisonOp::GreaterThan,
        threshold: 70.0,
        duration_minutes: None,
        hysteresis: None,
    }
}

#[actix::test]
async fn protocol_version_gate_accepts_same_major() {
    assert!(nimon_core::protocol::is_compatible("1.0"));
    assert!(nimon_core::protocol::is_compatible("1.7"));
    assert!(!nimon_core::protocol::is_compatible("2.0"));
}

// ----------------------------------------------------------------------
// WebSocket protocol against a real listening hub
// ----------------------------------------------------------------------

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn start_hub() -> (nimon_hub::HubState, std::net::SocketAddr) {
    start_hub_with(|state| state).await
}

async fn start_hub_with(
    customize: impl FnOnce(nimon_hub::HubState) -> nimon_hub::HubState,
) -> (nimon_hub::HubState, std::net::SocketAddr) {
    let pool = test_db().await;
    let config = nimon_hub::config::HubConfig::default();
    let state = customize(nimon_hub::start_hub_services(config, pool));
    let app = nimon_hub::build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (state, addr)
}

async fn connect(addr: std::net::SocketAddr) -> Client {
    let (client, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", addr))
        .await
        .unwrap();
    client
}

async fn send(client: &mut Client, msg: &WsMessage) {
    client
        .send(TMessage::Text(msg.to_json().unwrap()))
        .await
        .unwrap();
}

/// Next hub message of `ty` (skipping others such as config pushes).
async fn recv_type(client: &mut Client, ty: WsMessageType) -> WsMessage {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let next = tokio::time::timeout_at(deadline, client.next())
            .await
            .expect("timed out waiting for hub message")
            .expect("socket closed")
            .unwrap();
        if let TMessage::Text(text) = next {
            let msg = WsMessage::from_json(&text).unwrap();
            if msg.msg_type == ty {
                return msg;
            }
        }
    }
}

fn register(edge_id: &str) -> WsMessage {
    WsMessage::edge_register(EdgeRegister {
        edge_id: edge_id.to_string(),
        name: format!("Edge {}", edge_id),
        hostname: None,
        ip_address: None,
    })
}

#[actix::test]
async fn websocket_register_mismatch_and_action_roundtrip() {
    let (state, addr) = start_hub().await;
    let mut client = connect(addr).await;

    // Payloads before registration are rejected
    let early = WsMessage::device_status(status_update("edge-1:daq-1", 20.0));
    send(&mut client, &early).await;
    let err = recv_type(&mut client, WsMessageType::Error).await;
    assert_eq!(err.reply_to.as_deref(), Some(early.msg_id.as_str()));

    let reg = register("edge-1");
    send(&mut client, &reg).await;
    let ack = recv_type(&mut client, WsMessageType::Ack).await;
    assert_eq!(ack.correlation_id(), reg.msg_id);

    // A second registration with another id on the same socket is rejected
    send(&mut client, &register("edge-2")).await;
    let err = recv_type(&mut client, WsMessageType::Error).await;
    assert!(err.payload.to_string().contains("EDGE_ID_MISMATCH"));
    assert!(state.sessions().get("edge-2").is_none());

    // Payload for a different edge is rejected
    let mut foreign = status_update("edge-9:dev", 20.0);
    foreign.edge_id = "edge-9".to_string();
    send(&mut client, &WsMessage::device_status(foreign)).await;
    let err = recv_type(&mut client, WsMessageType::Error).await;
    assert!(err.payload.to_string().contains("EDGE_ID_MISMATCH"));

    // Valid status is tracked on the session
    send(
        &mut client,
        &WsMessage::device_status(status_update("edge-1:daq-1", 20.0)),
    )
    .await;
    let session = state.sessions().get("edge-1").unwrap();
    wait_until(|| {
        let session = session.clone();
        async move { session.device_count().await == 1 }
    })
    .await;

    // Unknown message types are ignored (connection stays up)
    send(
        &mut client,
        &WsMessage::from_json(
            r#"{"version":"1.9","type":"future_thing","payload":{},"timestamp":"2024-01-01T00:00:00Z","msg_id":"X1"}"#,
        )
        .unwrap(),
    )
    .await;

    // Action round trip: the edge answers with reply_to
    let executor = state.action_executor().unwrap();
    let action_task = tokio::task::spawn_local(async move {
        let mut action = Action::power_cycle("act-ws", "Power cycle", "ws test");
        action.timeout_secs = 5;
        executor
            .send(nimon_hub::action::executor::ExecuteAction {
                action,
                context: ActionContext::new("edge-1", "edge-1:daq-1", ""),
            })
            .await
            .unwrap()
    });
    let request = recv_type(&mut client, WsMessageType::ExecuteAction).await;
    send(
        &mut client,
        &WsMessage::action_result(ok_result("act-ws")).with_reply_to(request.msg_id),
    )
    .await;
    assert_eq!(action_task.await.unwrap().status, ActionStatus::Succeeded);
}

#[actix::test]
async fn websocket_replacement_keeps_new_session() {
    let (state, addr) = start_hub().await;

    let mut first = connect(addr).await;
    send(&mut first, &register("edge-1")).await;
    recv_type(&mut first, WsMessageType::Ack).await;
    let first_conn = state
        .sessions()
        .get("edge-1")
        .unwrap()
        .conn_id()
        .to_string();

    let mut second = connect(addr).await;
    send(&mut second, &register("edge-1")).await;
    recv_type(&mut second, WsMessageType::Ack).await;

    // The old socket is closed by the hub
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match first.next().await {
                None | Some(Err(_)) | Some(Ok(TMessage::Close(_))) => break,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "replaced socket must be closed");

    // ... and its exit must not unregister the new session
    tokio::time::sleep(Duration::from_millis(200)).await;
    let current = state.sessions().get("edge-1").expect("new session kept");
    assert_ne!(current.conn_id(), first_conn);

    // Closing the new socket unregisters it
    second.close(None).await.unwrap();
    wait_until(|| {
        let sessions = state.sessions();
        async move { sessions.get("edge-1").is_none() }
    })
    .await;
}

#[actix::test]
async fn websocket_incompatible_version_gets_error_then_close() {
    let (_state, addr) = start_hub().await;
    let mut client = connect(addr).await;
    let mut msg = register("edge-1");
    msg.version = "2.0".to_string();
    send(&mut client, &msg).await;
    let err = recv_type(&mut client, WsMessageType::Error).await;
    assert!(err.payload.to_string().contains("VERSION_MISMATCH"));
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match client.next().await {
                None | Some(Err(_)) | Some(Ok(TMessage::Close(_))) => break,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(closed.is_ok());
}

async fn wait_closed(client: &mut Client, within: Duration) -> bool {
    tokio::time::timeout(within, async {
        loop {
            match client.next().await {
                None | Some(Err(_)) | Some(Ok(TMessage::Close(_))) => break,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .is_ok()
}

#[actix::test]
async fn websocket_without_registration_is_closed_after_deadline() {
    let (_state, addr) =
        start_hub_with(|s| s.with_registration_timeout(Duration::from_millis(300))).await;
    let mut idle = connect(addr).await;
    assert!(
        wait_closed(&mut idle, Duration::from_secs(5)).await,
        "unregistered connection must be closed"
    );

    // A registered connection is not affected by the deadline
    let mut client = connect(addr).await;
    send(&mut client, &register("edge-1")).await;
    recv_type(&mut client, WsMessageType::Ack).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    send(&mut client, &WsMessage::ping()).await;
    recv_type(&mut client, WsMessageType::Pong).await;
}

#[actix::test]
async fn websocket_rejects_devices_not_owned_by_the_edge() {
    let (state, addr) = start_hub().await;
    let mut client = connect(addr).await;
    send(&mut client, &register("edge-1")).await;
    recv_type(&mut client, WsMessageType::Ack).await;

    for foreign in ["edge-2:daq-1", "edge-10:x", "daq-1", "edge-1:", "edge-2"] {
        let msg = WsMessage::device_status(status_update(foreign, 20.0));
        send(&mut client, &msg).await;
        let err = recv_type(&mut client, WsMessageType::Error).await;
        assert!(
            err.payload.to_string().contains("DEVICE_ID_NOT_OWNED"),
            "{}: {}",
            foreign,
            err.payload
        );
        assert_eq!(err.reply_to.as_deref(), Some(msg.msg_id.as_str()));
    }
    let removed = WsMessage::device_removed(nimon_core::actor::messages::DeviceRemoved {
        edge_id: "edge-1".to_string(),
        device_id: "edge-2:daq-1".to_string(),
        timestamp: chrono::Utc::now(),
    });
    send(&mut client, &removed).await;
    let err = recv_type(&mut client, WsMessageType::Error).await;
    assert!(err.payload.to_string().contains("DEVICE_ID_NOT_OWNED"));
    assert!(!state.is_device_removed("edge-2:daq-1"));

    // Owned forms are accepted: `<edge>:<name>` and the edge id itself
    let own = WsMessage::device_status(status_update("edge-1:daq-1", 20.0));
    send(&mut client, &own).await;
    let edge_level = WsMessage::device_alert(nimon_core::actor::messages::DeviceAlert {
        device_id: "edge-1".to_string(),
        edge_id: "edge-1".to_string(),
        severity: Severity::Warning,
        message: "edge-level".to_string(),
        metric_name: None,
        metric_value: None,
        timestamp: chrono::Utc::now(),
    });
    send(&mut client, &edge_level).await;
    send(&mut client, &WsMessage::ping()).await;
    // Next message is the pong: neither owned payload produced an error
    let next = recv_type_any(&mut client).await;
    assert_eq!(next.msg_type, WsMessageType::Pong, "{:?}", next.payload);
}

/// Next non-config hub message
async fn recv_type_any(client: &mut Client) -> WsMessage {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("timed out waiting for hub message")
            .expect("socket closed")
            .unwrap();
        if let TMessage::Text(text) = next {
            let msg = WsMessage::from_json(&text).unwrap();
            if msg.msg_type != WsMessageType::ConfigUpdate {
                return msg;
            }
        }
    }
}

#[actix::test]
async fn websocket_action_result_from_another_edge_is_ignored() {
    let (state, addr) = start_hub().await;
    let mut edge_a = connect(addr).await;
    send(&mut edge_a, &register("edge-1")).await;
    recv_type(&mut edge_a, WsMessageType::Ack).await;
    let mut edge_b = connect(addr).await;
    send(&mut edge_b, &register("edge-2")).await;
    recv_type(&mut edge_b, WsMessageType::Ack).await;

    let executor = state.action_executor().unwrap();
    let task = tokio::task::spawn_local(async move {
        let mut action = Action::power_cycle("rule-restart", "Power cycle", "ws test");
        action.timeout_secs = 10;
        executor
            .send(nimon_hub::action::executor::ExecuteAction {
                action,
                context: ActionContext::new("edge-1", "edge-1:daq-1", ""),
            })
            .await
            .unwrap()
    });
    let request = recv_type(&mut edge_a, WsMessageType::ExecuteAction).await;

    // Edge B answers edge A's request (by request id and by action id)
    let mut failed = ok_result("rule-restart");
    failed.success = false;
    send(
        &mut edge_b,
        &WsMessage::action_result(failed.clone()).with_reply_to(request.msg_id.clone()),
    )
    .await;
    send(&mut edge_b, &WsMessage::action_result(failed)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !task.is_finished(),
        "edge B must not complete edge A's action"
    );

    send(
        &mut edge_a,
        &WsMessage::action_result(ok_result("rule-restart")).with_reply_to(request.msg_id),
    )
    .await;
    assert_eq!(task.await.unwrap().status, ActionStatus::Succeeded);
}

#[actix::test]
async fn websocket_disconnect_fails_pending_edge_actions_promptly() {
    let (state, addr) = start_hub().await;
    let mut client = connect(addr).await;
    send(&mut client, &register("edge-1")).await;
    recv_type(&mut client, WsMessageType::Ack).await;

    let executor = state.action_executor().unwrap();
    let task = tokio::task::spawn_local(async move {
        let mut action = Action::power_cycle("act-drop", "Power cycle", "ws test");
        action.timeout_secs = 60;
        executor
            .send(nimon_hub::action::executor::ExecuteAction {
                action,
                context: ActionContext::new("edge-1", "edge-1:daq-1", ""),
            })
            .await
            .unwrap()
    });
    recv_type(&mut client, WsMessageType::ExecuteAction).await;
    client.close(None).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("pending action must fail when its edge disconnects")
        .unwrap();
    assert_eq!(result.status, ActionStatus::Failed);
    assert!(result.error.contains("dropped"), "{}", result.error);
}

#[actix::test]
async fn websocket_stalled_edge_is_dropped_and_unregistered() {
    let (state, addr) = start_hub().await;
    let mut client = connect(addr).await;
    send(&mut client, &register("edge-1")).await;
    recv_type(&mut client, WsMessageType::Ack).await;
    let session = state.sessions().get("edge-1").unwrap();

    // The edge stops reading; the hub keeps pushing large frames until
    // the socket buffers are full and a write stalls
    let big = "x".repeat(512 * 1024);
    for _ in 0..120 {
        if session
            .send(WsMessage::error("TEST".to_string(), big.clone(), None))
            .is_err()
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    wait_until_for(Duration::from_secs(15), || {
        let sessions = state.sessions();
        async move { sessions.get("edge-1").is_none() }
    })
    .await;
    drop(client);
}

async fn wait_until_for<F, Fut>(within: Duration, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + within;
    while !check().await {
        assert!(
            std::time::Instant::now() < deadline,
            "condition not reached in time"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
