//! Alert manager actor for rule evaluation and alert lifecycle

use actix::prelude::*;
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use sqlx::SqlitePool;
use tracing::{debug, info, warn};

use nimon_core::alert::*;
use nimon_core::alert::rules::{AlertRule, ComparisonOp, EvaluationContext, RuleCondition};
use nimon_core::actor::messages::{DeviceStatusUpdate, PredictionResult};
use nimon_core::db::{AlertRepository, PredictionRepository};
use nimon_core::{HealthStatus, MetricValue};

use crate::action::executor::{ActionContext, ExecuteAction};
use crate::action::actions::Action;

/// Prediction alert thresholds
const PREDICTION_ALERT_THRESHOLD: f64 = 0.8;
const PREDICTION_CRITICAL_THRESHOLD: f64 = 0.9;

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
    notification_tx: tokio::sync::mpsc::UnboundedSender<Alert>,
    action_executor: Option<actix::Addr<crate::action::executor::ActionExecutor>>,
    /// SQLite connection pool for persisting alerts and predictions.
    /// When `Some`, alerts and predictions are written to the database.
    db_pool: Option<SqlitePool>,
}

impl AlertManager {
    pub fn new(config: AlertManagerConfig) -> Self {
        let (notification_tx, _) = tokio::sync::mpsc::unbounded_channel();
        Self {
            config,
            rules: Self::default_rules(),
            active_alerts: DashMap::new(),
            notification_tx,
            action_executor: None,
            db_pool: None,
        }
    }

    /// Set the database pool for persisting alerts and predictions.
    /// Returns Self for builder-pattern chaining.
    pub fn with_db_pool(mut self, pool: SqlitePool) -> Self {
        self.db_pool = Some(pool);
        self
    }

    /// Set the action executor for auto-remediation on critical alerts.
    /// Returns Self for builder-pattern chaining.
    pub fn with_action_executor(mut self, addr: actix::Addr<crate::action::executor::ActionExecutor>) -> Self {
        self.action_executor = Some(addr);
        self
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
                condition: RuleCondition::DeviceOffline { max_minutes_since_poll: 5 },
                cooldown_minutes: 5,
                notification_channels: vec!["console".to_string()],
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
                    duration_minutes: None,
                },
                cooldown_minutes: 5,
                notification_channels: vec!["console".to_string()],
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
                    duration_minutes: None,
                },
                cooldown_minutes: 2,
                notification_channels: vec!["console".to_string()],
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
                cooldown_minutes: 5,
                notification_channels: vec!["console".to_string()],
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
            triggered_at: ctx.last_poll,
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
            let cooldown_minutes = if let Some(rule) = rule {
                rule.cooldown_minutes
            } else {
                self.config.default_cooldown_minutes as i32
            };
            let cooldown_duration = Duration::minutes(cooldown_minutes as i64);
            if elapsed < cooldown_duration {
                return true;
            }
        }
        false
    }

    /// Common logic for processing rule evaluation: checks cooldown, creates alerts,
    /// stores them in active_alerts, and returns the newly created alerts.
    fn process_evaluation(&mut self, ctx: &EvaluationContext) -> Vec<Alert> {
        let mut new_alerts = Vec::new();

        for rule in self.evaluate_rules(ctx) {
            // Check cooldown
            if self.check_cooldown(&rule.id, &ctx.device_id) {
                debug!("Rule {} for device {} in cooldown", rule.id, ctx.device_id);
                continue;
            }

            let alert = self.create_alert(&rule, ctx);
            let key = format!("{}:{}", rule.id, ctx.device_id);

            // Update or insert alert
            if let Some(mut active) = self.active_alerts.get_mut(&key) {
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

            // Auto-execute remediation for critical alerts when an action executor is available
            if alert.severity == AlertSeverity::Critical {
                if let Some(ref executor) = self.action_executor {
                    let action = Action::restart_service(
                        format!("auto-remediate-{}", alert.id),
                        format!("Auto-remediate: {}", alert.title),
                        format!("Automatic restart triggered by critical alert {}", alert.id),
                        "nimon-agent",
                    );
                    let ctx = ActionContext::new(
                        &alert.edge_id,
                        &alert.device_id,
                        &alert.id,
                    );
                    executor.do_send(ExecuteAction { action, context: ctx });
                    info!(
                        "Auto-remediation action dispatched for critical alert {} on device {}",
                        alert.id, alert.device_id
                    );
                }
            }

            new_alerts.push(alert);
        }

        new_alerts
    }

    /// Persist alerts to the database (fire-and-forget).
    fn persist_alerts(&self, alerts: &[Alert], ctx: &mut <Self as Actor>::Context) {
        if alerts.is_empty() {
            return;
        }
        if let Some(pool) = &self.db_pool {
            let pool = pool.clone();
            let alerts_to_persist: Vec<_> = alerts.iter().map(|a| {
                (
                    a.device_id.clone(),
                    a.edge_id.clone(),
                    a.rule_id.clone(),
                    a.severity.to_string(),
                    a.message.clone(),
                )
            }).collect();
            let fut = async move {
                let repo = AlertRepository::new(&pool);
                for (device_id, edge_id, rule_name, severity, message) in alerts_to_persist {
                    if let Err(e) = repo.insert(
                        Some(device_id.as_str()),
                        Some(edge_id.as_str()),
                        &rule_name,
                        &severity,
                        &message,
                    ).await {
                        warn!("Failed to persist alert: {}", e);
                    }
                }
            };
            ctx.spawn(actix::fut::wrap_future(fut));
        }
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

    fn handle(&mut self, msg: EvaluateRules, ctx: &mut Self::Context) -> Self::Result {
        let alerts = self.process_evaluation(&msg.context);
        self.persist_alerts(&alerts, ctx);
        alerts
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
#[rtype(result = "Result<(), nimon_core::NimonError>")]
pub struct ResolveAlert {
    pub alert_id: String,
}

impl Handler<ResolveAlert> for AlertManager {
    type Result = Result<(), nimon_core::NimonError>;

    fn handle(&mut self, msg: ResolveAlert, _ctx: &mut Self::Context) -> Self::Result {
        for mut entry in self.active_alerts.iter_mut() {
            if entry.value().alert.id == msg.alert_id {
                entry.value_mut().alert.status = AlertStatus::Resolved;
                entry.value_mut().alert.resolved_at = Some(Utc::now());
                info!("Alert resolved: {}", msg.alert_id);
                return Ok(());
            }
        }
        Err(nimon_core::NimonError::AlertNotFound(msg.alert_id))
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
            previous_status: None,
            metrics: msg.metrics.clone(),
            last_poll: msg.timestamp,
            predictions: vec![],
        };

        let alerts = self.process_evaluation(&eval_ctx);
        self.persist_alerts(&alerts, ctx);
        for alert in alerts {
            let _ = self.notification_tx.send(alert);
        }
    }
}

/// Handle prediction results
impl Handler<PredictionResult> for AlertManager {
    type Result = ();

    fn handle(&mut self, msg: PredictionResult, ctx: &mut Self::Context) -> Self::Result {
        if msg.probability < PREDICTION_ALERT_THRESHOLD {
            return;
        }

        let rule_id = format!("pred-{:?}", msg.prediction_type).to_lowercase();

        // Check cooldown
        if self.check_cooldown(&rule_id, &msg.device_id) {
            debug!("Prediction {} for device {} in cooldown", rule_id, msg.device_id);
            return;
        }

        let alert = Alert {
            id: ulid::Ulid::new().to_string(),
            rule_id: rule_id.clone(),
            edge_id: msg.edge_id.clone(),
            device_id: msg.device_id.clone(),
            severity: if msg.probability >= PREDICTION_CRITICAL_THRESHOLD { AlertSeverity::Critical } else { AlertSeverity::Warning },
            status: AlertStatus::Firing,
            title: format!("{:?} Prediction for {}", msg.prediction_type, msg.device_id),
            message: format!("Predicted {:?} with {:.0}% confidence", msg.prediction_type, msg.probability * 100.0),
            metric_name: None,
            metric_value: Some(msg.probability),
            threshold: Some(0.8),
            triggered_at: msg.timestamp,
            resolved_at: None,
            fired_count: 1,
            notification_sent: false,
        };

        info!("Prediction alert: {}", alert.title);

        // Update or insert alert (same pattern as process_evaluation)
        let key = format!("{}:{}", rule_id, msg.device_id);
        if let Some(mut active) = self.active_alerts.get_mut(&key) {
            active.last_fired = Utc::now();
            active.fired_count += 1;
            active.alert.fired_count = active.fired_count;
        } else {
            self.active_alerts.insert(key, ActiveAlert {
                alert: alert.clone(),
                last_fired: Utc::now(),
                fired_count: 1,
            });
        }

        // Persist the prediction to the database
        if let Some(pool) = &self.db_pool {
            let pool = pool.clone();
            let device_id = msg.device_id.clone();
            let edge_id = msg.edge_id.clone();
            let prediction_type = format!("{:?}", msg.prediction_type);
            let probability = msg.probability;
            let eta_minutes = msg.eta_minutes.map(|m| m as i64);
            let fut = async move {
                let repo = PredictionRepository::new(&pool);
                if let Err(e) = repo.insert(
                    &device_id,
                    &edge_id,
                    &prediction_type,
                    probability,
                    eta_minutes,
                    None::<&str>,
                ).await {
                    warn!("Failed to persist prediction: {}", e);
                }
            };
            ctx.spawn(actix::fut::wrap_future(fut));
        }

        // Also persist the alert generated from this prediction
        self.persist_alerts(&[alert.clone()], ctx);

        let _ = self.notification_tx.send(alert);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = AlertManagerConfig::default();
        assert_eq!(config.default_cooldown_minutes, 5);
        assert_eq!(config.max_firing_count, 100);
    }
}
