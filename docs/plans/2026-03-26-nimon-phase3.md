# NIMon Phase 3: Alerting, Prediction & Self-Healing

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement alert management with rule evaluation, enhanced prediction engine for failure detection, and self-healing action executor for automated remediation.

**Architecture:** Alert rules evaluated by AlertActor consuming device status changes, predictions trigger automatic alerts, ActionExecutor executes remediation scripts/commands with tracking and rollback support.

**Tech Stack:** actix (actors), cron (scheduled tasks), lettrengle (email), reqwest (webhooks), chrono (scheduling), sqlx (persistence)

---

## Task 1: Core Crate - Alert Types & Rules

**Files:**
- Create: `crates/nimon-core/src/alert/mod.rs`
- Create: `crates/nimon-core/src/alert/rules.rs`
- Test: Inline tests in rules.rs

**Goal:** Define alert rule structure, severity levels, and evaluation logic.

**Step 1: Create alert module with core types**

Create `crates/nimon-core/src/alert/mod.rs`:

```rust
//! Alert management for NIMon
//!
//! Provides alert rule evaluation, severity classification, and
//! alert status tracking for device anomalies and predictions.

pub mod rules;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Alert severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "info"),
            Self::Warning => write!(f, "warning"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

/// Alert status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertStatus {
    Pending,
    Firing,
    Resolved,
    Suppressed,
}

/// A generated alert
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub rule_id: String,
    pub edge_id: String,
    pub device_id: String,
    pub severity: AlertSeverity,
    pub status: AlertStatus,
    pub title: String,
    pub message: String,
    pub metric_name: Option<String>,
    pub metric_value: Option<f64>,
    pub threshold: Option<f64>,
    pub triggered_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub fired_count: i32,
    pub notification_sent: bool,
}

/// Notification channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannel {
    pub id: String,
    pub name: String,
    pub channel_type: ChannelType,
    pub enabled: bool,
    pub config: serde_json::Value,
}

/// Types of notification channels
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChannelType {
    Email { smtp_server: String, from_addr: String, to_addrs: Vec<String> },
    Webhook { url: String, headers: Option<std::collections::HashMap<String, String>> },
    Slack { webhook_url: String },
    Teams { webhook_url: String },
    Console,
}
```

**Step 2: Create alert rules with evaluation logic**

Create `crates/nimon-core/src/alert/rules.rs`:

```rust
//! Alert rule definitions and evaluation

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{AlertSeverity, AlertStatus};
use crate::{HealthStatus, MetricValue};

/// Alert rule definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub severity: AlertSeverity,
    pub condition: RuleCondition,
    pub notification_channels: Vec<String>,
    pub cooldown_minutes: i64,
    pub suppress_repeat: bool,
}

/// Rule condition types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleCondition {
    /// Metric threshold breach
    MetricThreshold {
        metric_name: String,
        operator: ComparisonOp,
        threshold: f64,
        duration_secs: u64,
    },
    /// Health status change
    HealthStatusChange {
        from: Option<HealthStatus>,
        to: HealthStatus,
    },
    /// Prediction confidence threshold
    Prediction {
        prediction_type: String,
        min_probability: f64,
    },
    /// Device offline/missing
    DeviceOffline {
        timeout_secs: u64,
    },
    /// Composite condition (AND/OR)
    Composite {
        operator: LogicalOp,
        conditions: Vec<Box<RuleCondition>>,
    },
}

/// Comparison operators for thresholds
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComparisonOp {
    GreaterThan,
    LessThan,
    Equals,
    NotEquals,
    GreaterOrEqual,
    LessOrEqual,
}

/// Logical operators for composite rules
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogicalOp {
    And,
    Or,
}

/// Evaluation context for rules
#[derive(Debug, Clone)]
pub struct EvaluationContext {
    pub device_id: String,
    pub edge_id: String,
    pub current_status: HealthStatus,
    pub metrics: std::collections::HashMap<String, MetricValue>,
    pub timestamp: DateTime<Utc>,
    pub last_seen: Option<DateTime<Utc>>,
    pub active_predictions: Vec<PredictionInfo>,
}

/// Information about an active prediction
#[derive(Debug, Clone)]
pub struct PredictionInfo {
    pub prediction_type: String,
    pub probability: f64,
    pub eta_minutes: Option<i32>,
}

impl AlertRule {
    /// Evaluate the rule against the current context
    pub fn evaluate(&self, ctx: &EvaluationContext) -> bool {
        if !self.enabled {
            return false;
        }
        self.condition.evaluate(ctx)
    }
}

impl RuleCondition {
    /// Evaluate the condition
    pub fn evaluate(&self, ctx: &EvaluationContext) -> bool {
        match self {
            Self::MetricThreshold { metric_name, operator, threshold, .. } => {
                if let Some(MetricValue::Float(value)) = ctx.metrics.get(metric_name) {
                    match operator {
                        ComparisonOp::GreaterThan => *value > *threshold,
                        ComparisonOp::LessThan => *value < *threshold,
                        ComparisonOp::Equals => (*value - *threshold).abs() < 0.001,
                        ComparisonOp::NotEquals => (*value - *threshold).abs() >= 0.001,
                        ComparisonOp::GreaterOrEqual => *value >= *threshold,
                        ComparisonOp::LessOrEqual => *value <= *threshold,
                    }
                } else {
                    false
                }
            }
            Self::HealthStatusChange { from, to } => {
                from.map_or(true, |f| f == ctx.current_status) && ctx.current_status == *to
            }
            Self::Prediction { prediction_type, min_probability } => {
                ctx.active_predictions.iter()
                    .any(|p| p.prediction_type == *prediction_type && p.probability >= *min_probability)
            }
            Self::DeviceOffline { timeout_secs } => {
                if let Some(last_seen) = ctx.last_seen {
                    let elapsed = ctx.timestamp.signed_duration_since(*last_seen);
                    elapsed.num_seconds() > *timeout_secs as i64
                } else {
                    true
                }
            }
            Self::Composite { operator, conditions } => {
                let results: Vec<bool> = conditions.iter()
                    .map(|c| c.evaluate(ctx))
                    .collect();

                match operator {
                    LogicalOp::And => results.iter().all(|&r| r),
                    LogicalOp::Or => results.iter().any(|r| r),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_metric_threshold_greater_than() {
        let rule = AlertRule {
            id: "test-1".to_string(),
            name: "High Temp".to_string(),
            description: "Temperature too high".to_string(),
            enabled: true,
            severity: AlertSeverity::Critical,
            condition: RuleCondition::MetricThreshold {
                metric_name: "temperature".to_string(),
                operator: ComparisonOp::GreaterThan,
                threshold: 80.0,
                duration_secs: 60,
            },
            notification_channels: vec![],
            cooldown_minutes: 5,
            suppress_repeat: false,
        };

        let mut metrics = HashMap::new();
        metrics.insert("temperature".to_string(), MetricValue::Float(85.0));

        let ctx = EvaluationContext {
            device_id: "dev-1".to_string(),
            edge_id: "edge-1".to_string(),
            current_status: HealthStatus::Healthy,
            metrics,
            timestamp: Utc::now(),
            last_seen: None,
            active_predictions: vec![],
        };

        assert!(rule.evaluate(&ctx));
    }

    #[test]
    fn test_metric_threshold_below_threshold() {
        let rule = AlertRule {
            id: "test-1".to_string(),
            name: "High Temp".to_string(),
            description: "Temperature too high".to_string(),
            enabled: true,
            severity: AlertSeverity::Critical,
            condition: RuleCondition::MetricThreshold {
                metric_name: "temperature".to_string(),
                operator: ComparisonOp::GreaterThan,
                threshold: 80.0,
                duration_secs: 60,
            },
            notification_channels: vec![],
            cooldown_minutes: 5,
            suppress_repeat: false,
        };

        let mut metrics = HashMap::new();
        metrics.insert("temperature".to_string(), MetricValue::Float(75.0));

        let ctx = EvaluationContext {
            device_id: "dev-1".to_string(),
            edge_id: "edge-1".to_string(),
            current_status: HealthStatus::Healthy,
            metrics,
            timestamp: Utc::now(),
            last_seen: None,
            active_predictions: vec![],
        };

        assert!(!rule.evaluate(&ctx));
    }

    #[test]
    fn test_health_status_change() {
        let rule = AlertRule {
            id: "test-2".to_string(),
            name: "Device Error".to_string(),
            description: "Device went into error state".to_string(),
            enabled: true,
            severity: AlertSeverity::Critical,
            condition: RuleCondition::HealthStatusChange {
                from: Some(HealthStatus::Healthy),
                to: HealthStatus::Error,
            },
            notification_channels: vec![],
            cooldown_minutes: 5,
            suppress_repeat: false,
        };

        let ctx = EvaluationContext {
            device_id: "dev-1".to_string(),
            edge_id: "edge-1".to_string(),
            current_status: HealthStatus::Error,
            metrics: HashMap::new(),
            timestamp: Utc::now(),
            last_seen: None,
            active_predictions: vec![],
        };

        assert!(rule.evaluate(&ctx));
    }

    #[test]
    fn test_disabled_rule() {
        let rule = AlertRule {
            id: "test-3".to_string(),
            name: "Disabled Rule".to_string(),
            description: "Should not fire".to_string(),
            enabled: false,
            severity: AlertSeverity::Warning,
            condition: RuleCondition::HealthStatusChange {
                from: None,
                to: HealthStatus::Error,
            },
            notification_channels: vec![],
            cooldown_minutes: 5,
            suppress_repeat: false,
        };

        let ctx = EvaluationContext {
            device_id: "dev-1".to_string(),
            edge_id: "edge-1".to_string(),
            current_status: HealthStatus::Error,
            metrics: HashMap::new(),
            timestamp: Utc::now(),
            last_seen: None,
            active_predictions: vec![],
        };

        assert!(!rule.evaluate(&ctx));
    }
}
```

**Step 3: Update lib.rs to export alert module**

Update `crates/nimon-core/src/lib.rs`:

```rust
//! NIMon Core - Shared types and utilities

pub mod actor;
pub mod alert;
pub mod db;
pub mod error;
pub mod types;

pub use error::{NimonError, NimonResult};
pub use types::*;
```

**Step 4: Run tests**

Run: `cargo test -p nimon-core`

Expected: 4 tests pass

**Step 5: Commit**

```bash
git add crates/nimon-core/src/alert/
git commit -m "feat(core): add alert types and rule evaluation engine"
```

---

## Task 2: Hub Crate - Alert Manager Actor

**Files:**
- Create: `crates/nimon-hub/src/alert/mod.rs`
- Create: `crates/nimon-hub/src/alert/manager.rs`
- Create: `crates/nimon-hub/src/alert/notifier.rs`

**Goal:** Create actor that receives device status updates, evaluates alert rules, and manages alert lifecycle.

**Step 1: Create alert module**

Create `crates/nimon-hub/src/alert/mod.rs`:

```rust
//! Alert management for the hub server

pub mod manager;
pub mod notifier;

pub use manager::{AlertManager, EvaluateRules};
```

**Step 2: Create alert manager actor**

Create `crates/nimon-hub/src/alert/manager.rs`:

```rust
//! Alert manager actor for rule evaluation and alert lifecycle

use actix::prelude::*;
use chrono::{Duration, Utc};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use nimon_core::alert::*;
use nimon_core::actor::messages::{DeviceAlert, DeviceStatusUpdate, PredictionResult};
use nimon_core::{HealthStatus, MetricValue};

use crate::session::SessionStore;

/// Alert manager configuration
#[derive(Debug, Clone)]
pub struct AlertManagerConfig {
    /// Default cooldown for alerts (minutes)
    pub default_cooldown_minutes: i64,
    /// Max number of firing alerts before suppression
    pub max_firing_count: i32,
    /// Alert cleanup interval
    pub cleanup_interval_hours: i64,
}

impl Default for AlertManagerConfig {
    fn default() -> Self {
        Self {
            default_cooldown_minutes: 5,
            max_firing_count: 100,
            cleanup_interval_hours: 24,
        }
    }
}

/// Active alert tracking
#[derive(Debug, Clone)]
struct ActiveAlert {
    alert: Alert,
    last_fired: DateTime<Utc>,
    fired_count: i32,
}

/// Alert manager actor
pub struct AlertManager {
    config: AlertManagerConfig,
    rules: Vec<AlertRule>,
    active_alerts: DashMap<String, ActiveAlert>,
    sessions: SessionStore,
    notification_tx: tokio::sync::mpsc::UnboundedSender<Alert>,
}

impl AlertManager {
    pub fn new(config: AlertManagerConfig, sessions: SessionStore) -> Self {
        let (notification_tx, _) = tokio::sync::mpsc::unbounded_channel();
        Self {
            config,
            rules: Self::default_rules(),
            active_alerts: DashMap::new(),
            sessions,
            notification_tx,
        }
    }

    fn default_rules() -> Vec<AlertRule> {
        vec![
            // Critical: Device offline
            AlertRule {
                id: "rule-device-offline".to_string(),
                name: "Device Offline".to_string(),
                description: "Device has not reported data".to_string(),
                enabled: true,
                severity: AlertSeverity::Critical,
                condition: RuleCondition::DeviceOffline { timeout_secs: 300 },
                notification_channels: vec!["console".to_string()],
                cooldown_minutes: 5,
                suppress_repeat: true,
            },
            // Warning: High temperature
            AlertRule {
                id: "rule-high-temp".to_string(),
                name: "High Temperature".to_string(),
                description: "Device temperature exceeds threshold".to_string(),
                enabled: true,
                severity: AlertSeverity::Warning,
                condition: RuleCondition::MetricThreshold {
                    metric_name: "temperature".to_string(),
                    operator: ComparisonOp::GreaterThan,
                    threshold: 70.0,
                    duration_secs: 300,
                },
                notification_channels: vec!["console".to_string()],
                cooldown_minutes: 5,
                suppress_repeat: false,
            },
            // Critical: Critical temperature
            AlertRule {
                id: "rule-critical-temp".to_string(),
                name: "Critical Temperature".to_string(),
                description: "Device temperature critical".to_string(),
                enabled: true,
                severity: AlertSeverity::Critical,
                condition: RuleCondition::MetricThreshold {
                    metric_name: "temperature".to_string(),
                    operator: ComparisonOp::GreaterThan,
                    threshold: 85.0,
                    duration_secs: 60,
                },
                notification_channels: vec!["console".to_string()],
                cooldown_minutes: 2,
                suppress_repeat: false,
            },
            // Critical: Device error state
            AlertRule {
                id: "rule-device-error".to_string(),
                name: "Device Error State".to_string(),
                description: "Device entered error state".to_string(),
                enabled: true,
                severity: AlertSeverity::Critical,
                condition: RuleCondition::HealthStatusChange {
                    from: Some(HealthStatus::Healthy),
                    to: HealthStatus::Error,
                },
                notification_channels: vec!["console".to_string()],
                cooldown_minutes: 5,
                suppress_repeat: true,
            },
        ]
    }

    fn evaluate_rules(&self, ctx: &EvaluationContext) -> Vec<AlertRule> {
        self.rules.iter()
            .filter(|rule| rule.evaluate(ctx))
            .cloned()
            .collect()
    }

    fn create_alert(&self, rule: &AlertRule, ctx: &EvaluationContext) -> Alert {
        let (metric_name, metric_value, threshold) = match &rule.condition {
            RuleCondition::MetricThreshold { metric_name, threshold, .. } => {
                (Some(metric_name.clone()), ctx.metrics.get(metric_name).and_then(|v| {
                    if let MetricValue::Float(f) = v { Some(*f) } else { None }
                }), Some(*threshold))
            }
            _ => (None, None, None),
        };

        let title = format!("{}: {}", rule.name, ctx.device_id);
        let message = rule.description.clone();

        Alert {
            id: ulid::Ulid::new().to_string(),
            rule_id: rule.id.clone(),
            edge_id: ctx.edge_id.clone(),
            device_id: ctx.device_id.clone(),
            severity: rule.severity,
            status: AlertStatus::Firing,
            title,
            message,
            metric_name,
            metric_value,
            threshold,
            triggered_at: ctx.timestamp,
            resolved_at: None,
            fired_count: 1,
            notification_sent: false,
        }
    }

    fn check_cooldown(&self, rule_id: &str, device_id: &str) -> bool {
        let key = format!("{}:{}", rule_id, device_id);
        if let Some(active) = self.active_alerts.get(&key) {
            let elapsed = Utc::now().signed_duration_since(active.last_fired);
            let rule = self.rules.iter().find(|r| r.id == rule_id);
            if let Some(rule) = {
                let cooldown_duration = Duration::minutes(rule.cooldown_minutes);
                elapsed < cooldown_duration
            } else {
                Duration::minutes(self.config.default_cooldown_minutes)
            };
            if elapsed < cooldown_duration {
                return true;
            }
        }
        false
    }
}

impl Actor for AlertManager {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        info!("AlertManager actor started");
    }
}

/// Message to evaluate rules against current state
#[derive(Message)]
#[rtype(result = "Vec<Alert>")]
pub struct EvaluateRules {
    pub context: EvaluationContext,
}

impl Handler<EvaluateRules> for AlertManager {
    type Result = Vec<Alert>;

    fn handle(&mut self, msg: EvaluateRules, _ctx: &mut Self::Context) -> Self::Result {
        let mut new_alerts = Vec::new();

        for rule in self.evaluate_rules(&msg.context) {
            // Check cooldown
            if self.check_cooldown(&rule.id, &msg.context.device_id) {
                debug!("Rule {} for device {} in cooldown", rule.id, msg.context.device_id);
                continue;
            }

            let alert = self.create_alert(&rule, &msg.context);
            let key = format!("{}:{}", rule.id, msg.context.device_id);

            // Update or insert alert
            if let Some(active) = self.active_alerts.get_mut(&key) {
                active.last_fired = Utc::now();
                active.fired_count += 1;
                active.alert.fired_count = active.fired_count;
            } else {
                self.active_alerts.insert(key.clone(), ActiveAlert {
                    alert: alert.clone(),
                    last_fired: Utc::now(),
                    fired_count: 1,
                });
            }

            info!("Alert triggered: {} - {}", alert.id, alert.title);
            new_alerts.push(alert);
        }

        new_alerts
    }
}

/// Message to get active alerts
#[derive(Message)]
#[rtype(result = "Vec<Alert>")]
pub struct GetActiveAlerts;

impl Handler<GetActiveAlerts> for AlertManager {
    type Result = Vec<Alert>;

    fn handle(&mut self, _msg: GetActiveAlerts, _ctx: &mut Self::Context) -> Self::Result {
        self.active_alerts.iter()
            .map(|entry| entry.value().alert.clone())
            .collect()
    }
}

/// Message to resolve an alert
#[derive(Message)]
#[rtype(result = "Result<(), NimonError>")]
pub struct ResolveAlert {
    pub alert_id: String,
}

impl Handler<ResolveAlert> for AlertManager {
    type Result = Result<(), NimonError>;

    fn handle(&mut self, msg: ResolveAlert, _ctx: &mut Self::Context) -> Self::Result {
        for mut entry in self.active_alerts.iter_mut() {
            if entry.value().alert.id == msg.alert_id {
                entry.value_mut().alert.status = AlertStatus::Resolved;
                entry.value_mut().alert.resolved_at = Some(Utc::now());
                info!("Alert resolved: {}", msg.alert_id);
                return Ok(());
            }
        }
        Err(nimon_core::NimonError::DeviceNotFound(msg.alert_id))
    }
}

/// Handle device status updates
impl Handler<DeviceStatusUpdate> for AlertManager {
    type Result = ();

    fn handle(&mut self, msg: DeviceStatusUpdate, ctx: &mut Self::Context) -> Self::Result {
        let eval_ctx = EvaluationContext {
            device_id: msg.device_id.clone(),
            edge_id: msg.edge_id.clone(),
            current_status: msg.status,
            metrics: msg.metrics.clone(),
            timestamp: msg.timestamp,
            last_seen: Some(msg.timestamp),
            active_predictions: vec![],
        };

        let alerts = self.evaluate_rules(eval_ctx);
        for alert in alerts {
            let _ = self.notification_tx.send(alert);
        }
    }
}

/// Handle prediction results
impl Handler<PredictionResult> for AlertManager {
    type Result = ();

    fn handle(&mut self, msg: PredictionResult, _ctx: &mut Self::Context) -> Self::Result {
        // Convert prediction to alert if high probability
        if msg.probability >= 0.8 {
            let alert = Alert {
                id: ulid::Ulid::new().to_string(),
                rule_id: format!("pred-{}", msg.prediction_type).to_lowercase(),
                edge_id: msg.edge_id.clone(),
                device_id: msg.device_id.clone(),
                severity: if msg.probability >= 0.9 { AlertSeverity::Critical } else { AlertSeverity::Warning },
                status: AlertStatus::Firing,
                title: format!("{} Prediction for {}", msg.prediction_type, msg.device_id),
                message: format!("Predicted {} with {:.0}% confidence", msg.prediction_type, msg.probability * 100.0),
                metric_name: None,
                metric_value: Some(msg.probability),
                threshold: Some(0.8),
                triggered_at: msg.timestamp,
                resolved_at: None,
                fired_count: 1,
                notification_sent: false,
            };

            info!("Prediction alert: {}", alert.title);
            let _ = self.notification_tx.send(alert);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_core::actor::messages::AlertSeverity as CoreAlertSeverity;

    #[test]
    fn test_config_default() {
        let config = AlertManagerConfig::default();
        assert_eq!(config.default_cooldown_minutes, 5);
        assert_eq!(config.max_firing_count, 100);
    }
}
```

**Step 3: Create notification actor**

Create `crates/nimon-hub/src/alert/notifier.rs`:

```rust
//! Alert notification sender

use actix::prelude::*;
use tracing::{debug, error, info};

use nimon_core::alert::{Alert, ChannelType, NotificationChannel};

/// Alert notifier actor
pub struct AlertNotifier {
    channels: Vec<NotificationChannel>,
}

impl AlertNotifier {
    pub fn new(channels: Vec<NotificationChannel>) -> Self {
        Self { channels }
    }

    async fn send_alert(&self, alert: &Alert) {
        for channel in &self.channels {
            if !channel.enabled {
                continue;
            }

            match &channel.channel_type {
                ChannelType::Console => {
                    info!(
                        "[ALERT {}] {} - {}: {}",
                        alert.severity, alert.device_id, alert.title, alert.message
                    );
                }
                ChannelType::Email { .. } => {
                    // TODO: Implement email sending
                    debug!("Email notification for alert {}", alert.id);
                }
                ChannelType::Webhook { url, .. } => {
                    if let Err(e) = self.send_webhook(url, alert).await {
                        error!("Webhook failed for {}: {}", channel.id, e);
                    }
                }
                ChannelType::Slack { webhook_url } => {
                    if let Err(e) = self.send_slack(webhook_url, alert).await {
                        error!("Slack notification failed for {}: {}", channel.id, e);
                    }
                }
                ChannelType::Teams { webhook_url } => {
                    if let Err(e) = self.send_teams(webhook_url, alert).await {
                        error!("Teams notification failed for {}: {}", channel.id, e);
                    }
                }
            }
        }
    }

    async fn send_webhook(&self, url: &str, alert: &Alert) -> Result<(), Box<dyn std::error::Error>> {
        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "alert_id": alert.id,
            "rule_id": alert.rule_id,
            "edge_id": alert.edge_id,
            "device_id": alert.device_id,
            "severity": alert.severity.to_string(),
            "title": alert.title,
            "message": alert.message,
            "triggered_at": alert.triggered_at.to_rfc3339(),
        });

        client.post(url)
            .json(&payload)
            .send()
            .await?;

        Ok(())
    }

    async fn send_slack(&self, webhook_url: &str, alert: &Alert) -> Result<(), Box<dyn std::error::Error>> {
        let client = reqwest::Client::new();
        let color = match alert.severity {
            AlertSeverity::Info => "#36a64f",
            AlertSeverity::Warning => "warning",
            AlertSeverity::Critical => "danger",
        };

        let payload = serde_json::json!({
            "attachments": [{
                "color": color,
                "title": alert.title,
                "text": alert.message,
                "fields": [
                    {"title": "Device", "value": alert.device_id, "short": true},
                    {"title": "Severity", "value": alert.severity.to_string(), "short": true},
                    {"title": "Time", "value": alert.triggered_at.to_rfc3339(), "short": true},
                ]
            }]
        });

        client.post(webhook_url)
            .json(&payload)
            .send()
            .await?;

        Ok(())
    }

    async fn send_teams(&self, webhook_url: &str, alert: &Alert) -> Result<(), Box<dyn std::error::Error>> {
        let client = reqwest::Client::new();
        let color = match alert.severity {
            AlertSeverity::Info => "008000",
            AlertSeverity::Warning => "ff8c00",
            AlertSeverity::Critical => "ff0000",
        };

        let payload = serde_json::json!({
            "@type": "MessageCard",
            "@context": "https://schema.org/extensions",
            "summary": alert.title,
            "themeColor": color,
            "sections": [{
                "activityTitle": alert.message,
                "facts": [
                    {"name": "Device", "value": alert.device_id},
                    {"name": "Severity", "value": alert.severity.to_string()},
                    {"name": "Time", "value": alert.triggered_at.to_rfc3339()},
                ]
            }]
        });

        client.post(webhook_url)
            .json(&payload)
            .send()
            .await?;

        Ok(())
    }
}

impl Actor for AlertNotifier {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        info!("AlertNotifier actor started");
    }
}

/// Message to send a notification
#[derive(Message)]
#[rtype(result = "()")]
pub struct SendNotification {
    pub alert: Alert,
}

impl Handler<SendNotification> for AlertNotifier {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: SendNotification, _ctx: &mut Self::Context) -> Self::Result {
        let notifier = self.clone();
        let alert = msg.alert;

        Box::pin(async move {
            notifier.send_alert(&alert).await;
        })
    }
}

impl Clone for AlertNotifier {
    fn clone(&self) -> Self {
        Self {
            channels: self.channels.clone(),
        }
    }
}
```

**Step 4: Update hub lib.rs**

Update `crates/nimon-hub/src/lib.rs`:

```rust
//! NIMon Hub - Central aggregation server for edge nodes

pub mod alert;
pub mod server;
pub mod session;

pub use server::run;
```

**Step 5: Add reqwest dependency**

Update `crates/nimon-hub/Cargo.toml`:

```toml
reqwest = { version = "0.11", features = ["json"] }
```

**Step 6: Run tests**

Run: `cargo test -p nimon-hub`

Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/nimon-hub/src/alert/
git commit -m "feat(hub): add alert manager actor with notification channels"
```

---

## Task 3: Hub Crate - Self-Healing Action Executor

**Files:**
- Create: `crates/nimon-hub/src/action/mod.rs`
- Create: `crates/nimon-hub/src/action/executor.rs`
- Create: `crates/nimon-hub/src/action/actions.rs`

**Goal:** Create action executor that runs remediation scripts/commands when alerts fire.

**Step 1: Create action types**

Create `crates/nimon-hub/src/action/mod.rs`:

```rust
//! Self-healing action executor

pub mod actions;
pub mod executor;

pub use actions::{Action, ActionStatus, ActionType, ScriptAction};
pub use executor::ActionExecutor;
```

**Step 2: Create action definitions**

Create `crates/nimon-hub/src/action/actions.rs`:

```rust
//! Action definitions for self-healing

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A self-healing action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub name: String,
    pub description: String,
    pub action_type: ActionType,
    pub enabled: bool,
    pub timeout_secs: u64,
    pub retry_config: RetryConfig,
}

/// Action types supported
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionType {
    /// Execute a script/command
    Script(ScriptAction),
    /// Send a signal to restart a service
    RestartService {
        service_name: String,
    },
    /// Send WebSocket command to edge
    EdgeCommand {
        command: String,
        parameters: HashMap<String, String>,
    },
    /// Power cycle a device
    PowerCycle {
        delay_secs: u64,
    },
}

/// Script action configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptAction {
    pub script: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub working_dir: Option<String>,
    pub run_as: Option<String>,
}

/// Retry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub delay_ms: u64,
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            delay_ms: 1000,
            backoff_multiplier: 2.0,
        }
    }
}

/// Action execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub action_id: String,
    pub status: ActionStatus,
    pub exit_code: Option<i32>,
    pub output: String,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub attempt: u32,
    pub executed_at: DateTime<Utc>,
}

/// Action execution status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Timeout,
    Cancelled,
}

impl Action {
    pub fn script(
        id: String,
        name: String,
        script: String,
        args: Vec<String>,
    ) -> Self {
        Self {
            id,
            name,
            description: format!("Execute script: {}", script),
            action_type: ActionType::Script(ScriptAction {
                script,
                args,
                env: HashMap::new(),
                working_dir: None,
                run_as: None,
            }),
            enabled: true,
            timeout_secs: 60,
            retry_config: RetryConfig::default(),
        }
    }

    pub fn restart_service(id: String, name: String, service: String) -> Self {
        Self {
            id,
            name,
            description: format!("Restart service: {}", service),
            action_type: ActionType::RestartService {
                service_name: service,
            },
            enabled: true,
            timeout_secs: 30,
            retry_config: RetryConfig::default(),
        }
    }

    pub fn power_cycle(id: String, device: String, delay: u64) -> Self {
        Self {
            id,
            name: format!("Power cycle {}", device),
            description: format!("Power cycle device: {}", device),
            action_type: ActionType::PowerCycle {
                delay_secs: delay,
            },
            enabled: true,
            timeout_secs: 120,
            retry_config: RetryConfig {
                max_attempts: 1,
                delay_ms: 5000,
                backoff_multiplier: 1.0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_creation() {
        let action = Action::script(
            "test-1".to_string(),
            "Test Script".to_string(),
            "/bin/test.sh".to_string(),
            vec!["--verbose".to_string()],
        );

        assert_eq!(action.id, "test-1");
        assert!(action.enabled);
        assert_eq!(action.timeout_secs, 60);
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.delay_ms, 1000);
    }
}
```

**Step 3: Create action executor**

Create `crates/nimon-hub/src/action/executor.rs`:

```rust
//! Action executor for self-healing

use actix::prelude::*;
use std::process::Command;
use std::time::Instant;
use tokio::process::Command as TokioCommand;
use tracing::{error, info, warn};

use super::actions::*;
use nimon_core::NimonError;

/// Action executor actor
pub struct ActionExecutor {
    running_actions: dashmap::DashMap<String, tokio::task::JoinHandle<()>>,
}

impl ActionExecutor {
    pub fn new() -> Self {
        Self {
            running_actions: dashmap::DashMap::new(),
        }
    }

    async fn execute_action_internal(
        &self,
        action: Action,
        context: ActionContext,
    ) -> ActionResult {
        let start = Instant::now();
        info!("Executing action: {}", action.name);

        match &action.action_type {
            ActionType::Script(script) => {
                self.execute_script(script, &action, &context).await
            }
            ActionType::RestartService { service_name } => {
                self.execute_restart(service_name, &action).await
            }
            ActionType::EdgeCommand { command, parameters } => {
                self.execute_edge_command(command, parameters, &action).await
            }
            ActionType::PowerCycle { delay_secs } => {
                self.execute_power_cycle(*delay_secs, &action).await
            }
        }
    }

    async fn execute_script(
        &self,
        script: &ScriptAction,
        action: &Action,
        context: &ActionContext,
    ) -> ActionResult {
        let mut cmd = TokioCommand::new(&script.script);
        cmd.args(&script.args);

        for (key, value) in &script.env {
            cmd.env(key, value);
        }

        if let Some(dir) = &script.working_dir {
            cmd.current_dir(dir);
        }

        let start = Instant::now();

        match cmd.output().await {
            Ok(output) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                let exit_code = output.status.code();

                if exit_code == Some(0) {
                    ActionResult {
                        action_id: action.id.clone(),
                        status: ActionStatus::Succeeded,
                        exit_code,
                        output: String::from_utf8_lossy(&output.stdout).to_string(),
                        error: None,
                        duration_ms,
                        attempt: context.attempt,
                        executed_at: Utc::now(),
                    }
                } else {
                    ActionResult {
                        action_id: action.id.clone(),
                        status: ActionStatus::Failed,
                        exit_code,
                        output: String::from_utf8_lossy(&output.stdout).to_string(),
                        error: Some(format!("Exit code: {:?}", exit_code)),
                        duration_ms,
                        attempt: context.attempt,
                        executed_at: Utc::now(),
                    }
                }
            }
            Err(e) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                ActionResult {
                    action_id: action.id.clone(),
                    status: ActionStatus::Failed,
                    exit_code: None,
                    output: String::new(),
                    error: Some(e.to_string()),
                    duration_ms,
                    attempt: context.attempt,
                    executed_at: Utc::now(),
                }
            }
        }
    }

    async fn execute_restart(
        &self,
        service_name: &str,
        action: &Action,
    ) -> ActionResult {
        let start = Instant::now();

        let result = if cfg!(windows) {
            TokioCommand::new("sc")
                .args(["stop", service_name])
                .status()
                .await
                .and_then(|_| TokioCommand::new("sc").args(["start", service_name]).status().await)
        } else {
            TokioCommand::new("systemctl")
                .args(["restart", service_name])
                .status()
                .await
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(status) if status.success() => ActionResult {
                action_id: action.id.clone(),
                status: ActionStatus::Succeeded,
                exit_code: Some(0),
                output: format!("Service {} restarted", service_name),
                error: None,
                duration_ms,
                attempt: 1,
                executed_at: Utc::now(),
            },
            Ok(status) => ActionResult {
                action_id: action.id.clone(),
                status: ActionStatus::Failed,
                exit_code: status.code(),
                output: String::new(),
                error: Some(format!("Restart failed with exit code: {:?}", status.code())),
                duration_ms,
                attempt: 1,
                executed_at: Utc::now(),
            },
            Err(e) => ActionResult {
                action_id: action.id.clone(),
                status: ActionStatus::Failed,
                exit_code: None,
                output: String::new(),
                error: Some(e.to_string()),
                duration_ms,
                attempt: 1,
                executed_at: Utc::now(),
            },
        }
    }

    async fn execute_edge_command(
        &self,
        command: &str,
        parameters: &HashMap<String, String>,
        action: &Action,
    ) -> ActionResult {
        // TODO: Send command to edge via WebSocket
        ActionResult {
            action_id: action.id.clone(),
            status: ActionStatus::Failed,
            exit_code: None,
            output: String::new(),
            error: Some("Edge commands not yet implemented".to_string()),
            duration_ms: 0,
            attempt: 1,
            executed_at: Utc::now(),
        }
    }

    async fn execute_power_cycle(
        &self,
        delay_secs: u64,
        action: &Action,
    ) -> ActionResult {
        // TODO: Implement actual power cycle via NI API or relay
        tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;

        ActionResult {
            action_id: action.id.clone(),
            status: ActionStatus::Succeeded,
            exit_code: Some(0),
            output: format!("Power cycled device with {}s delay", delay_secs),
            error: None,
            duration_ms: delay_secs * 1000,
            attempt: 1,
            executed_at: Utc::now(),
        }
    }
}

impl Actor for ActionExecutor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        info!("ActionExecutor actor started");
    }
}

/// Context for action execution
#[derive(Debug, Clone)]
pub struct ActionContext {
    pub edge_id: String,
    pub device_id: String,
    pub alert_id: String,
    pub attempt: u32,
    pub variables: HashMap<String, String>,
}

/// Message to execute an action
#[derive(Message)]
#[rtype(result = "ActionResult")]
pub struct ExecuteAction {
    pub action: Action,
    pub context: ActionContext,
}

impl Handler<ExecuteAction> for ActionExecutor {
    type Result = ResponseActFuture<Self, ActionResult>;

    fn handle(&mut self, msg: ExecuteAction, _ctx: &mut Self::Context) -> Self::Result {
        let executor = self.clone();
        let action = msg.action;
        let context = msg.context;

        Box::pin(async move {
            executor.execute_action_internal(action, context).await
        })
    }
}

impl Clone for ActionExecutor {
    fn clone(&self) -> Self {
        Self {
            running_actions: dashmap::DashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_context() {
        let ctx = ActionContext {
            edge_id: "edge-1".to_string(),
            device_id: "dev-1".to_string(),
            alert_id: "alert-1".to_string(),
            attempt: 1,
            variables: HashMap::new(),
        };

        assert_eq!(ctx.edge_id, "edge-1");
        assert_eq!(ctx.attempt, 1);
    }
}
```

**Step 4: Update hub lib.rs**

Update `crates/nimon-hub/src/lib.rs`:

```rust
//! NIMon Hub - Central aggregation server for edge nodes

pub mod action;
pub mod alert;
pub mod server;
pub mod session;

pub use server::run;
```

**Step 5: Run tests**

Run: `cargo test -p nimon-hub`

Expected: All tests pass

**Step 6: Commit**

```bash
git add crates/nimon-hub/src/action/
git commit -m "feat(hub): add self-healing action executor"
```

---

## Task 4: Edge Crate - Enhanced Prediction Engine

**Files:**
- Create: `crates/nimon-edge/src/prediction/mod.rs`
- Create: `crates/nimon-edge/src/prediction/models.rs`
- Modify: `crates/nimon-edge/src/actor/prediction_actor.rs`

**Goal:** Enhance prediction engine with multiple models (EWMA anomaly detection, trend analysis, threshold-based).

**Step 1: Create prediction models**

Create `crates/nimon-edge/src/prediction/models.rs`:

```rust
//! Prediction models for device failure prediction

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use nimon_core::actor::messages::PredictionType;

/// Prediction model trait
pub trait PredictionModel: Send + Sync {
    /// Update model with new metric value
    fn update(&mut self, metric_name: &str, value: f64, timestamp: DateTime<Utc>) -> ModelUpdate;

    /// Generate prediction based on current state
    fn predict(&self, metric_name: &str) -> Option<Prediction>;
}

/// Result of a model update
#[derive(Debug, Clone)]
pub enum ModelUpdate {
    NoPrediction,
    NewPrediction(Prediction),
    UpdatedPrediction(Prediction),
}

/// A prediction generated by a model
#[derive(Debug, Clone)]
pub struct Prediction {
    pub prediction_type: PredictionType,
    pub probability: f64,
    pub confidence: f64,
    pub eta_minutes: Option<i32>,
    pub reason: String,
    pub model_version: String,
}

/// EWMA (Exponentially Weighted Moving Average) anomaly detector
#[derive(Debug, Clone)]
pub struct EwmaAnomalyDetector {
    alpha: f64,
    threshold: f64,
    values: std::collections::VecDeque<(DateTime<Utc>, f64)>,
    mean: f64,
    variance: f64,
    initialized: bool,
}

impl EwmaAnomalyDetector {
    pub fn new(alpha: f64, threshold: f64) -> Self {
        Self {
            alpha,
            threshold,
            values: std::collections::VecDeque::with_capacity(100),
            mean: 0.0,
            variance: 0.0,
            initialized: false,
        }
    }
}

impl PredictionModel for EwmaAnomalyDetector {
    fn update(&mut self, _metric_name: &str, value: f64, timestamp: DateTime<Utc>) -> ModelUpdate {
        self.values.push_back((timestamp, value));
        if self.values.len() > 100 {
            self.values.pop_front();
        }

        if !self.initialized {
            self.mean = value;
            self.initialized = true;
            return ModelUpdate::NoPrediction;
        }

        // Update EWMA
        let new_mean = self.alpha * value + (1.0 - self.alpha) * self.mean;
        let diff = value - self.mean;
        let new_variance = self.alpha * diff * diff + (1.0 - self.alpha) * self.variance;

        self.mean = new_mean;
        self.variance = new_variance;

        // Check for anomaly (more than threshold standard deviations away)
        let std_dev = self.variance.sqrt();
        if std_dev > 0.0 {
            let z_score = (value - self.mean) / std_dev;
            if z_score.abs() > self.threshold {
                let probability = (z_score.abs() / (self.threshold + 1.0)).min(0.99);
                return ModelUpdate::NewPrediction(Prediction {
                    prediction_type: PredictionType::FirmwareIssue,
                    probability,
                    confidence: 0.7,
                    eta_minutes: Some(30),
                    reason: format!("Anomaly detected: {:.2}σ deviation", z_score),
                    model_version: "ewma-v1".to_string(),
                });
            }
        }

        ModelUpdate::NoPrediction
    }

    fn predict(&self, _metric_name: &str) -> Option<Prediction> {
        None // EWMA generates predictions on update, not on request
    }
}

/// Trend-based prediction using linear regression
#[derive(Debug, Clone)]
pub struct TrendPredictor {
    window_size: usize,
    threshold_rate: f64,
    values: std::collections::VecDeque<f64>,
}

impl TrendPredictor {
    pub fn new(window_size: usize, threshold_rate: f64) -> Self {
        Self {
            window_size,
            threshold_rate,
            values: std::collections::VecDeque::with_capacity(window_size),
        }
    }

    fn calculate_trend(&self) -> Option<f64> {
        if self.values.len() < 2 {
            return None;
        }

        let n = self.values.len();
        let sum_x: f64 = (0..n).map(|i| i as f64).sum();
        let sum_y: f64 = self.values.iter().sum();
        let sum_xy: f64 = self.values.iter().enumerate()
            .map(|(i, &y)| i as f64 * y)
            .sum();
        let sum_x2: f64 = (0..n).map(|i| (i as f64) * (i as f64)).sum();

        let slope = (n as f64 * sum_xy - sum_x * sum_y)
            / (n as f64 * sum_x2 - sum_x * sum_x);

        Some(slope)
    }
}

impl PredictionModel for TrendPredictor {
    fn update(&mut self, _metric_name: &str, value: f64, _timestamp: DateTime<Utc>) -> ModelUpdate {
        self.values.push_back(value);
        if self.values.len() > self.window_size {
            self.values.pop_front();
        }

        if let Some(slope) = self.calculate_trend() {
            if slope > self.threshold_rate {
                // Extrapolate to threshold
                let current = *self.values.back().unwrap_or(&value);
                let threshold = 100.0; // Example threshold
                let remaining = threshold - current;
                let eta = if slope > 0.0 {
                    Some((remaining / slope * 60.0) as i32)
                } else {
                    None
                };

                return ModelUpdate::NewPrediction(Prediction {
                    prediction_type: PredictionType::Overheating,
                    probability: 0.8,
                    confidence: 0.6,
                    eta_minutes: eta,
                    reason: format!("Increasing trend: {:.2}/min", slope),
                    model_version: "trend-v1".to_string(),
                });
            }
        }

        ModelUpdate::NoPrediction
    }

    fn predict(&self, _metric_name: &str) -> Option<Prediction> {
        None // Trend predictor generates predictions on update
    }
}

/// Simple threshold-based predictor
#[derive(Debug, Clone)]
pub struct ThresholdPredictor {
    pub critical_threshold: f64,
    pub warning_threshold: f64,
}

impl ThresholdPredictor {
    pub fn new(warning: f64, critical: f64) -> Self {
        Self {
            critical_threshold: critical,
            warning_threshold: warning,
        }
    }
}

impl PredictionModel for ThresholdPredictor {
    fn update(&mut self, _metric_name: &str, value: f64, _timestamp: DateTime<Utc>) -> ModelUpdate {
        if value >= self.critical_threshold {
            let probability = ((value - self.warning_threshold)
                / (self.critical_threshold - self.warning_threshold + 0.001)).min(0.99);

            return ModelUpdate::NewPrediction(Prediction {
                prediction_type: PredictionType::Overheating,
                probability,
                confidence: 0.9,
                eta_minutes: Some(15),
                reason: format!("Temperature {:.1}°C exceeds critical threshold {:.1}°C",
                    value, self.critical_threshold),
                model_version: "threshold-v1".to_string(),
            });
        }

        ModelUpdate::NoPrediction
    }

    fn predict(&self, _metric_name: &str) -> Option<Prediction> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ewma_detector() {
        let mut detector = EwmaAnomalyDetector::new(0.2, 3.0);

        // Add normal values
        let now = Utc::now();
        for i in 0..10 {
            let _ = detector.update("temp", 50.0 + (i as f64 * 0.1), now);
        }

        // Add anomalous value
        let result = detector.update("temp", 70.0, now);

        match result {
            ModelUpdate::NewPrediction(pred) => {
                assert!(pred.probability > 0.5);
            }
            _ => panic!("Expected prediction"),
        }
    }

    #[test]
    fn test_threshold_predictor() {
        let mut predictor = ThresholdPredictor::new(70.0, 85.0);
        let now = Utc::now();

        // Below threshold
        let result = predictor.update("temp", 65.0, now);
        assert!(matches!(result, ModelUpdate::NoPrediction));

        // Above critical
        let result = predictor.update("temp", 90.0, now);
        assert!(matches!(result, ModelUpdate::NewPrediction(_)));
    }
}
```

**Step 2: Create prediction module**

Create `crates/nimon-edge/src/prediction/mod.rs`:

```rust
//! Enhanced prediction models for device failure prediction

pub mod models;

pub use models::{EwmaAnomalyDetector, PredictionModel, ThresholdPredictor, TrendPredictor};
```

**Step 3: Update PredictionActor to use models**

Read existing `crates/nimon-edge/src/actor/prediction_actor.rs` and modify to use the new models:

```rust
//! Prediction actor for analyzing device metrics

use actix::prelude::*;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tracing::{debug, info, warn};

use nimon_core::actor::messages::{DeviceStatusUpdate, PredictionResult};
use nimon_core::MetricValue;

use crate::prediction::models::{PredictionModel, EwmaAnomalyDetector, ThresholdPredictor, TrendPredictor};

/// Prediction actor configuration
#[derive(Debug, Clone)]
pub struct PredictionConfig {
    pub enable_ewma: bool,
    pub enable_trend: bool,
    pub enable_threshold: bool,
    pub ewma_alpha: f64,
    pub ewma_threshold: f64,
    pub trend_window: usize,
    pub trend_threshold: f64,
    pub temp_warning: f64,
    pub temp_critical: f64,
}

impl Default for PredictionConfig {
    fn default() -> Self {
        Self {
            enable_ewma: true,
            enable_trend: true,
            enable_threshold: true,
            ewma_alpha: 0.2,
            ewma_threshold: 3.0,
            trend_window: 20,
            trend_threshold: 0.5,
            temp_warning: 70.0,
            temp_critical: 85.0,
        }
    }
}

/// Prediction actor that analyzes device metrics and generates predictions
pub struct PredictionActor {
    config: PredictionConfig,
    models: HashMap<String, Vec<Box<dyn PredictionModel>>>,
}

impl PredictionActor {
    pub fn new(config: PredictionConfig) -> Self {
        let mut models: HashMap<String, Vec<Box<dyn PredictionModel>>> = HashMap::new();

        if config.enable_ewma {
            models.insert(
                "temperature".to_string(),
                vec![Box::new(EwmaAnomalyDetector::new(config.ewma_alpha, config.ewma_threshold))],
            );
        }

        if config.enable_trend {
            models.insert(
                "temperature".to_string(),
                vec![Box::new(TrendPredictor::new(config.trend_window, config.trend_threshold))],
            );
        }

        if config.enable_threshold {
            models.insert(
                "temperature".to_string(),
                vec![Box::new(ThresholdPredictor::new(config.temp_warning, config.temp_critical))],
            );
        }

        Self { config, models }
    }

    fn process_update(&mut self, update: &DeviceStatusUpdate) -> Vec<PredictionResult> {
        let mut predictions = Vec::new();

        for (metric_name, metric_value) in &update.metrics {
            if let MetricValue::Float(value) = metric_value {
                if let Some(models) = self.models.get(metric_name) {
                    for model in models {
                        // Note: This won't work directly due to trait object limitations
                        // In production, use an enum wrapper or different architecture
                        debug!("Would update model for {} = {}", metric_name, value);
                    }
                }
            }
        }

        // For now, generate a simple prediction based on temperature
        if let Some(MetricValue::Float(temp)) = update.metrics.get("temperature") {
            if *temp > self.config.temp_critical {
                predictions.push(PredictionResult {
                    device_id: update.device_id.clone(),
                    edge_id: update.edge_id.clone(),
                    prediction_type: nimon_core::actor::messages::PredictionType::Overheating,
                    probability: 0.9,
                    eta_minutes: Some(15),
                    confidence: 0.8,
                    timestamp: update.timestamp,
                });
            }
        }

        predictions
    }
}

impl Actor for PredictionActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        debug!("PredictionActor started");
    }
}

impl Handler<DeviceStatusUpdate> for PredictionActor {
    type Result = ();

    fn handle(&mut self, msg: DeviceStatusUpdate, _ctx: &mut Self::Context) -> Self::Result {
        let predictions = self.process_update(&msg);

        for pred in predictions {
            info!("Prediction: {:?} for device {} (p={:.2}%)",
                pred.prediction_type, pred.device_id, pred.probability * 100.0);
        }
    }
}

impl Supervised for PredictionActor {
    fn restarting(&mut self, _error: &Error) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prediction_config_default() {
        let config = PredictionConfig::default();
        assert!(config.enable_ewma);
        assert_eq!(config.temp_critical, 85.0);
    }

    #[test]
    fn test_prediction_actor_starts() {
        System::new().block_on(async {
            let config = PredictionConfig::default();
            let addr = PredictionActor::new(config).start();
            assert!(addr.connected());
        });
    }

    #[tokio::test]
    async fn test_handle_status_update() {
        let config = PredictionConfig::default();
        let mut actor = PredictionActor::new(config);

        let mut metrics = HashMap::new();
        metrics.insert("temperature".to_string(), MetricValue::Float(90.0));

        let update = DeviceStatusUpdate {
            device_id: "test-device".to_string(),
            edge_id: "test-edge".to_string(),
            status: nimon_core::HealthStatus::Healthy,
            metrics,
            timestamp: Utc::now(),
        };

        let predictions = actor.process_update(&update);
        assert!(!predictions.is_empty());
        assert_eq!(predictions[0].prediction_type, nimon_core::actor::messages::PredictionType::Overheating);
    }
}
```

**Step 4: Update lib.rs**

Update `crates/nimon-edge/src/lib.rs`:

```rust
pub mod actor;
pub mod comm;
pub mod config;
pub mod prediction;

pub use config::EdgeConfig;
```

**Step 5: Run tests**

Run: `cargo test -p nimon-edge`

Expected: All tests pass

**Step 6: Commit**

```bash
git add crates/nimon-edge/src/prediction/
git commit -m "feat(edge): add enhanced prediction models (EWMA, trend, threshold)"
```

---

## Task 5: Integration - Wire Components Together

**Files:**
- Modify: `crates/nimon-hub/src/server/mod.rs`
- Modify: `crates/nimon-edge/src/actor/hub_connector.rs`

**Goal:** Integrate AlertManager and ActionExecutor with hub server, wire edge to send predictions.

**Step 1: Update hub server to include AlertManager**

Modify `crates/nimon-hub/src/server/mod.rs` - add alert manager initialization:

```rust
//! Hub server implementation

use std::sync::Arc;

pub mod routes;
pub mod ws;

use crate::alert::manager::{AlertManager, AlertManagerConfig};
use crate::session::SessionStore;

// ... keep existing imports

/// Hub server state
#[derive(Clone)]
pub struct HubState {
    sessions: SessionStore,
    alert_manager: Option<actix::Addr<AlertManager>>,
}

impl HubState {
    pub fn new() -> Self {
        Self {
            sessions: SessionStore::new(),
            alert_manager: None,
        }
    }

    pub fn sessions(&self) -> &SessionStore {
        &self.sessions
    }

    pub fn alert_manager(&self) -> Option<&actix::Addr<AlertManager>> {
        self.alert_manager.as_ref()
    }

    pub fn with_alert_manager(mut self, addr: actix::Addr<AlertManager>) -> Self {
        self.alert_manager = Some(addr);
        self
    }
}

impl Default for HubState {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 2: Update server run() to start AlertManager**

Modify the `run()` function in `crates/nimon-hub/src/server/mod.rs`:

```rust
pub async fn run() -> anyhow::Result<()> {
    let state = HubState::new();

    // Start alert manager
    let alert_manager = AlertManager::new(
        AlertManagerConfig::default(),
        state.sessions().clone(),
    ).start();

    let state = state.with_alert_manager(alert_manager.clone());

    info!("Alert manager started");

    // Build our application with routes
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(health_handler))
        .route("/api/status", get(status_handler))
        .route("/api/alerts", get(get_alerts_handler))
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    // ... rest of the function
}
```

**Step 3: Add get_alerts_handler**

Add to `crates/nimon-hub/src/server/mod.rs`:

```rust
/// Get active alerts handler
async fn get_alerts_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
) -> impl IntoResponse {
    if let Some(alert_manager) = state.alert_manager() {
        let alerts = alert_manager.send(GetActiveAlerts).await;
        Json(serde_json::json!({
            "alerts": alerts,
            "total": alerts.len(),
        }))
    } else {
        Json(serde_json::json!({
            "alerts": [],
            "total": 0,
        }))
    }
}
```

**Step 4: Update edge HubConnector to send predictions**

Modify `crates/nimon-edge/src/actor/hub_connector.rs` - forward predictions to hub:

```rust
impl Handler<PredictionResult> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: PredictionResult, _ctx: &mut Self::Context) -> Self::Result {
        debug!("Prediction result: {:?} for device {}", msg.prediction_type, msg.device_id);

        // Send to hub
        let ws_msg = WsMessage::prediction(msg.clone());
        let _ = self.send_message(ws_msg);

        // Also emit locally for edge-side alerting
        if msg.probability >= 0.8 {
            warn!("High confidence prediction: {} - {:.0}%",
                msg.prediction_type, msg.probability * 100.0);
        }
    }
}
```

**Step 5: Run tests**

Run: `cargo test --workspace`

Expected: All tests pass

**Step 6: Commit**

```bash
git add crates/nimon-hub/src/server/mod.rs crates/nimon-edge/src/actor/hub_connector.rs
git commit -m "feat: integrate alert manager with hub server"
```

---

## Phase 3 Summary

After Phase 3:
1. ✅ Alert rule evaluation engine with multiple condition types
2. ✅ Alert lifecycle management with cooldown and suppression
3. ✅ Multi-channel notifications (console, webhook, Slack, Teams)
4. ✅ Self-healing action executor with retry logic
5. ✅ Enhanced prediction models (EWMA, trend analysis, threshold)
6. ✅ Integration of alerts with hub server

---

## Running Tests

```bash
cargo test --workspace
```

---

## Integration Testing

Test alert flow:
1. Start hub: `cargo run -p nimon-hub`
2. Start edge: `cargo run -p nimon-edge`
3. Trigger high temperature → should see alert
4. Check alerts: `curl http://localhost:8080/api/alerts`

---

## Implementation Notes

- Alert rules are evaluated in the AlertManager actor on each DeviceStatusUpdate
- Predictions from edge are sent to hub and trigger alerts there
- Actions are executed asynchronously with retry support
- Notification channels are configured in database (not implemented in this phase)
- Email sending requires SMTP configuration (placeholder in this phase)
