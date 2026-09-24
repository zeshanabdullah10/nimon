//! Alert manager actor for rule evaluation and alert lifecycle
//!
//! Lifecycle: an alert opens (`firing`) when a rule's condition holds,
//! re-fires at most once per cooldown (re-notifying and re-running the
//! rule action unless acknowledged), and auto-resolves when the rule's
//! [`AlertRule::is_cleared`] holds. Edge-reported alerts resolve when the
//! device reports healthy again or after `edge_alert_ttl_minutes` without
//! a re-report; prediction alerts resolve after `prediction_ttl_minutes`
//! without a fresh matching prediction; the built-in edge-offline alert
//! resolves when the edge is seen again.
//!
//! Every database write goes through one ordered [`DbWriter`], so rows
//! are written in the order events happened.

use actix::prelude::*;
use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, info, warn};

use nimon_core::actor::messages::{DeviceRemoved, DeviceStatusUpdate, PredictionResult};
use nimon_core::alert::rules::{
    AlertRule, ComparisonOp, EvaluationContext, PredictionInfo, RuleCondition,
};
use nimon_core::alert::*;
use nimon_core::db::device_repo::DeviceRepository;
use nimon_core::db::{AlertRepository, DeviceMetadata, PredictionRepository};
use nimon_core::{DeviceStatus, HealthStatus, MetricValue, NimonError, Severity};

use crate::action::executor::{ActionContext, ExecuteAction, ExecuteRuleAction};
use crate::action::RuleActionSpec;
use crate::alert::notifier::{AlertNotifier, SendNotification};
use crate::config::{HubConfig, MaintenanceConfig};
use crate::db_writer::DbWriter;
use crate::session::SessionStore;

/// Prediction severity threshold for `critical`
const PREDICTION_CRITICAL_THRESHOLD: f64 = 0.9;

/// Rule id of the built-in edge-offline alert
pub const RULE_EDGE_OFFLINE: &str = "builtin-edge-offline";
/// Rule-id prefix of alerts reported by edges (`edge:<metric>`)
pub const EDGE_ALERT_PREFIX: &str = "edge:";
/// Rule-id prefix of prediction alerts (`pred-<type>`)
pub const PREDICTION_ALERT_PREFIX: &str = "pred-";

/// Metric keys that describe the device rather than measure it: stored
/// as device metadata, not as metric history.
const METADATA_KEYS: &[&str] = &[
    "product",
    "product_type",
    "model",
    "serial_number",
    "serial",
    "slot",
    "chassis",
    "firmware_version",
    "driver_version",
];

/// A buffered metric-history point: (device, metric, value, timestamp)
type MetricRow = (String, String, f64, DateTime<Utc>);

/// Flush the metric buffer early when it grows beyond this many points
const MAX_METRIC_BUFFER: usize = 10_000;

/// Alert manager configuration
#[derive(Debug, Clone)]
pub struct AlertManagerConfig {
    /// Default cooldown for alerts without a rule (minutes)
    pub default_cooldown_minutes: i64,
    /// Max number of simultaneously active alerts per rule before suppression
    pub max_firing_count: i32,
    /// Alert cleanup interval (hours)
    pub cleanup_interval_hours: i64,
    /// Probability at which predictions raise an alert
    pub prediction_alert_threshold: f64,
    /// Prediction alerts resolve (and cached predictions expire) after this many minutes
    pub prediction_ttl_minutes: i64,
    /// Edge-reported alerts resolve after this many minutes without a re-report
    pub edge_alert_ttl_minutes: i64,
    /// Built-in edge-offline alert threshold (seconds); 0 disables it
    pub edge_offline_after_secs: u64,
    /// Periodic evaluation sweep interval (seconds)
    pub evaluate_interval_secs: u64,
    /// Notification channels for alert notifications
    pub notification_channels: Vec<crate::config::NotificationChannelConfig>,
    /// Alert rules (built-in defaults when empty, unless `disable_default_rules`)
    pub rules: Vec<AlertRule>,
    /// Executable actions per rule id (preferred over `AlertRule::action`)
    pub rule_actions: HashMap<String, RuleActionSpec>,
    /// Do not fall back to built-in rules when `rules` is empty (rules were
    /// configured but none converted)
    pub disable_default_rules: bool,
    /// Retention settings
    pub maintenance: MaintenanceConfig,
}

impl Default for AlertManagerConfig {
    fn default() -> Self {
        Self {
            default_cooldown_minutes: 5,
            max_firing_count: 100,
            cleanup_interval_hours: 24,
            prediction_alert_threshold: 0.8,
            prediction_ttl_minutes: 15,
            edge_alert_ttl_minutes: 30,
            edge_offline_after_secs: 90,
            evaluate_interval_secs: 30,
            notification_channels: Vec::new(),
            rules: Vec::new(),
            rule_actions: HashMap::new(),
            disable_default_rules: false,
            maintenance: MaintenanceConfig::default(),
        }
    }
}

impl AlertManagerConfig {
    /// Build from the hub config, converting and validating rules. Rule
    /// conversion errors are logged; when every configured rule fails,
    /// the manager runs with NO rules (never silently with the defaults).
    pub fn from_hub_config(config: &HubConfig) -> Self {
        let built = crate::config::build_rules(&config.alert);
        for (name, e) in &built.errors {
            error!("Failed to convert alert rule '{}': {}", name, e);
        }
        if built.configured && built.rules.is_empty() {
            error!(
                "All {} configured alert rules failed to convert: NO alert rules are active \
                 (built-in defaults are NOT used when rules are configured). Fix the rules in the config.",
                config.alert.rules.len()
            );
        }
        Self {
            default_cooldown_minutes: config.alert.default_cooldown_minutes,
            max_firing_count: config.alert.max_firing_count,
            cleanup_interval_hours: config.alert.cleanup_interval_hours,
            prediction_alert_threshold: config.alert.prediction_alert_threshold,
            prediction_ttl_minutes: config.alert.prediction_ttl_minutes,
            edge_alert_ttl_minutes: config.alert.edge_alert_ttl_minutes,
            edge_offline_after_secs: config.alert.edge_offline_after_secs,
            evaluate_interval_secs: config.alert.evaluate_interval_secs,
            notification_channels: config.alert.notification_channels.clone(),
            rules: built.rules,
            rule_actions: built.actions,
            disable_default_rules: built.configured,
            maintenance: config.maintenance.clone(),
        }
    }
}

/// Identity of an active alert: one per (rule, edge, device).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AlertKey {
    rule_id: String,
    edge_id: String,
    device_id: String,
}

impl AlertKey {
    fn new(rule_id: &str, edge_id: &str, device_id: &str) -> Self {
        Self {
            rule_id: rule_id.to_string(),
            edge_id: edge_id.to_string(),
            device_id: device_id.to_string(),
        }
    }

    fn of(alert: &Alert) -> Self {
        Self::new(&alert.rule_id, &alert.edge_id, &alert.device_id)
    }
}

/// Active alert tracking
#[derive(Debug, Clone)]
struct ActiveAlert {
    alert: Alert,
    /// Last (re-)fire: cooldown reference
    last_fired: DateTime<Utc>,
    /// Last time the source re-reported the problem (TTL reference)
    last_reported: DateTime<Utc>,
    acknowledged_at: Option<DateTime<Utc>>,
}

/// Last-known state of a device (drives offline detection even when its
/// edge is gone).
#[derive(Debug, Clone)]
struct KnownDevice {
    edge_id: String,
    status: HealthStatus,
    metrics: HashMap<String, MetricValue>,
    /// Hub time of the last status report
    last_seen: DateTime<Utc>,
    /// Rebuilt from the database at startup (not a live report): its
    /// state may open time-based alerts but never auto-resolves any, until
    /// the device reports again. Without this, the startup grace floor on
    /// `last_seen` would look like a fresh poll and resolve every
    /// device-offline alert of dead edges on each restart.
    restored: bool,
}

#[derive(Debug, Clone)]
struct EdgeState {
    last_seen: DateTime<Utc>,
}

/// Alert manager actor
pub struct AlertManager {
    config: AlertManagerConfig,
    rules: std::sync::Arc<Vec<AlertRule>>,
    active: HashMap<AlertKey, ActiveAlert>,
    /// First time a rule's condition started breaching (duration windows)
    breach_started: HashMap<AlertKey, DateTime<Utc>>,
    /// Manually resolved keys: no re-fire until the cooldown passed
    manual_hold: HashMap<AlertKey, DateTime<Utc>>,
    /// Edges whose offline alert was resolved manually: no re-fire until seen again
    edge_offline_latched: HashSet<String>,
    /// Last-known device state keyed by device id
    devices: HashMap<String, KnownDevice>,
    /// Latest prediction per (device, prediction type) with receive time
    predictions: HashMap<String, HashMap<String, (PredictionInfo, DateTime<Utc>)>>,
    /// Last time each edge was seen
    edges: HashMap<String, EdgeState>,
    /// Devices whose DB row is ensured, with the metadata last written
    db_devices: HashMap<String, DeviceMetadata>,
    /// Buffered metric-history points (device, metric, value, timestamp)
    metric_buffer: Vec<MetricRow>,
    /// Last metric-history sample per device
    last_metric_sample: HashMap<String, DateTime<Utc>>,
    notifier_addr: Option<actix::Addr<AlertNotifier>>,
    action_executor: Option<actix::Addr<crate::action::executor::ActionExecutor>>,
    /// SQLite pool: when `Some`, alerts, predictions and device state are persisted.
    db_pool: Option<SqlitePool>,
    db_writer: Option<DbWriter>,
    /// Connected edge sessions (edge liveness during sweeps)
    sessions: Option<std::sync::Arc<SessionStore>>,
    started_at: DateTime<Utc>,
}

impl AlertManager {
    pub fn new(config: AlertManagerConfig) -> Self {
        let rules = if config.rules.is_empty() && !config.disable_default_rules {
            Self::default_rules()
        } else {
            config.rules.clone()
        };
        Self {
            config,
            rules: std::sync::Arc::new(rules),
            active: HashMap::new(),
            breach_started: HashMap::new(),
            manual_hold: HashMap::new(),
            edge_offline_latched: HashSet::new(),
            devices: HashMap::new(),
            predictions: HashMap::new(),
            edges: HashMap::new(),
            db_devices: HashMap::new(),
            metric_buffer: Vec::new(),
            last_metric_sample: HashMap::new(),
            notifier_addr: None,
            action_executor: None,
            db_pool: None,
            db_writer: None,
            sessions: None,
            started_at: Utc::now(),
        }
    }

    /// Set the database pool (a private ordered writer starts with the actor).
    pub fn with_db_pool(mut self, pool: SqlitePool) -> Self {
        self.db_pool = Some(pool);
        self
    }

    /// Use a shared ordered writer (and its pool) for persistence.
    pub fn with_db_writer(mut self, writer: DbWriter) -> Self {
        self.db_pool = Some(writer.pool().clone());
        self.db_writer = Some(writer);
        self
    }

    /// Set the action executor for rule-driven auto-remediation.
    pub fn with_action_executor(
        mut self,
        addr: actix::Addr<crate::action::executor::ActionExecutor>,
    ) -> Self {
        self.action_executor = Some(addr);
        self
    }

    /// Set the session store (edge liveness for the edge-offline alert).
    pub fn with_sessions(mut self, sessions: std::sync::Arc<SessionStore>) -> Self {
        self.sessions = Some(sessions);
        self
    }

    /// Built-in rules used when no rules are configured.
    pub fn default_rules() -> Vec<AlertRule> {
        vec![
            AlertRule {
                id: "rule-device-offline".to_string(),
                name: "Device Offline".to_string(),
                description: "Device has not reported data".to_string(),
                enabled: true,
                severity: Severity::Critical,
                condition: RuleCondition::DeviceOffline {
                    max_minutes_since_poll: 5,
                },
                cooldown_minutes: 5,
                notification_channels: vec![],
                suppress_repeat: true,
                max_firing_count: None,
                action: None,
            },
            AlertRule {
                id: "rule-high-temp".to_string(),
                name: "High Temperature".to_string(),
                description: "Device temperature exceeds threshold".to_string(),
                enabled: true,
                severity: Severity::Warning,
                condition: RuleCondition::MetricThreshold {
                    metric_name: "temperature".to_string(),
                    operator: ComparisonOp::GreaterOrEqual,
                    threshold: nimon_core::DEFAULT_TEMP_WARNING_C,
                    duration_minutes: None,
                    hysteresis: Some(2.0),
                },
                cooldown_minutes: 5,
                notification_channels: vec![],
                suppress_repeat: false,
                max_firing_count: None,
                action: None,
            },
            AlertRule {
                id: "rule-critical-temp".to_string(),
                name: "Critical Temperature".to_string(),
                description: "Device temperature critical".to_string(),
                enabled: true,
                severity: Severity::Critical,
                condition: RuleCondition::MetricThreshold {
                    metric_name: "temperature".to_string(),
                    operator: ComparisonOp::GreaterOrEqual,
                    threshold: nimon_core::DEFAULT_TEMP_CRITICAL_C,
                    duration_minutes: None,
                    hysteresis: Some(2.0),
                },
                cooldown_minutes: 2,
                notification_channels: vec![],
                suppress_repeat: false,
                max_firing_count: None,
                action: None,
            },
            AlertRule {
                id: "rule-device-error".to_string(),
                name: "Device Error State".to_string(),
                description: "Device entered error state".to_string(),
                enabled: true,
                severity: Severity::Critical,
                condition: RuleCondition::HealthStatusChange {
                    from: Some(HealthStatus::Healthy),
                    to: HealthStatus::Error,
                },
                cooldown_minutes: 5,
                notification_channels: vec![],
                suppress_repeat: true,
                max_firing_count: None,
                action: None,
            },
        ]
    }

    // ------------------------------------------------------------------
    // Persistence
    // ------------------------------------------------------------------

    fn write<F, Fut>(&self, op: F)
    where
        F: FnOnce(SqlitePool) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        if let Some(writer) = &self.db_writer {
            writer.submit(op);
        } else if let Some(pool) = &self.db_pool {
            tokio::spawn(op(pool.clone()));
        }
    }

    /// Sheddable write (status snapshots, history, maintenance): see
    /// [`DbWriter::submit_low`]. Nothing a priority write depends on may
    /// go through here.
    fn write_low<F, Fut>(&self, op: F)
    where
        F: FnOnce(SqlitePool) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        if let Some(writer) = &self.db_writer {
            writer.submit_low(op);
        } else if let Some(pool) = &self.db_pool {
            tokio::spawn(op(pool.clone()));
        }
    }

    fn has_db(&self) -> bool {
        self.db_writer.is_some() || self.db_pool.is_some()
    }

    /// Make sure a device row exists before rows referencing it are written.
    fn ensure_device_row(&mut self, device_id: &str, edge_id: &str) {
        if device_id.is_empty() || !self.has_db() || self.db_devices.contains_key(device_id) {
            return;
        }
        self.db_devices
            .insert(device_id.to_string(), DeviceMetadata::default());
        let (device_id, edge_id) = (device_id.to_string(), edge_id.to_string());
        self.write(move |pool| async move {
            if let Err(e) = DeviceRepository::new(&pool)
                .upsert_snapshot(&device_id, &edge_id)
                .await
            {
                warn!("Failed to register device {}: {}", device_id, e);
            }
        });
    }

    fn persist_new_alert(&mut self, alert: &Alert) {
        if !self.has_db() {
            return;
        }
        self.ensure_device_row(&alert.device_id, &alert.edge_id);
        let alert = alert.clone();
        self.write(move |pool| async move {
            if let Err(e) = AlertRepository::new(&pool).upsert(&alert).await {
                warn!("Failed to persist alert {}: {}", alert.id, e);
            }
        });
    }

    fn persist_refire(&self, alert: &Alert, at: DateTime<Utc>) {
        let alert = alert.clone();
        self.write(move |pool| async move {
            let repo = AlertRepository::new(&pool);
            match repo
                .record_refire(
                    &alert.id,
                    alert.fired_count,
                    at,
                    alert.metric_value,
                    &alert.message,
                )
                .await
            {
                Ok(0) => {
                    if let Err(e) = repo.upsert(&alert).await {
                        warn!("Failed to persist re-fired alert {}: {}", alert.id, e);
                    }
                }
                Ok(_) => {}
                Err(e) => warn!("Failed to record re-fire of {}: {}", alert.id, e),
            }
        });
    }

    fn persist_status(&self, alert_id: &str, status: AlertStatus, at: DateTime<Utc>) {
        let alert_id = alert_id.to_string();
        self.write(move |pool| async move {
            if let Err(e) = AlertRepository::new(&pool)
                .set_status(&alert_id, status, at)
                .await
            {
                warn!("Failed to set alert {} to {}: {}", alert_id, status, e);
            }
        });
    }

    /// Persist device status, metadata (when new/changed) and sample metrics.
    fn persist_device_state(&mut self, msg: &DeviceStatusUpdate, now: DateTime<Utc>) {
        if !self.has_db() {
            return;
        }
        let meta = extract_metadata(msg);
        if self.db_devices.get(&msg.device_id) != Some(&meta) {
            self.db_devices.insert(msg.device_id.clone(), meta.clone());
            let (device_id, edge_id) = (msg.device_id.clone(), msg.edge_id.clone());
            self.write(move |pool| async move {
                if let Err(e) = DeviceRepository::new(&pool)
                    .upsert_metadata(&device_id, &edge_id, &meta)
                    .await
                {
                    warn!("Failed to store device {} metadata: {}", device_id, e);
                }
            });
        }

        let status = DeviceStatus {
            device_id: msg.device_id.clone(),
            status: msg.status,
            last_poll: msg.timestamp,
            metrics: msg.metrics.clone(),
            error_message: None,
            error_count: 0,
            uptime_seconds: 0,
        };
        self.write_low(move |pool| async move {
            if let Err(e) = DeviceRepository::new(&pool).upsert_status(&status).await {
                error!("Failed to persist device status: {}", e);
            }
        });

        // Throttled metric sampling into the batch buffer
        let interval =
            Duration::seconds(self.config.maintenance.metric_sample_interval_secs.max(0));
        let due = self
            .last_metric_sample
            .get(&msg.device_id)
            .map(|last| now.signed_duration_since(*last) >= interval)
            .unwrap_or(true);
        if due {
            self.last_metric_sample.insert(msg.device_id.clone(), now);
            for (name, value) in &msg.metrics {
                if METADATA_KEYS.contains(&name.as_str()) {
                    continue;
                }
                if let Some(v) = value.as_f64_finite() {
                    self.metric_buffer.push((
                        msg.device_id.clone(),
                        name.clone(),
                        v,
                        msg.timestamp,
                    ));
                }
            }
            if self.metric_buffer.len() >= MAX_METRIC_BUFFER {
                self.flush_metrics();
            }
        }
    }

    /// Write buffered metric points (one transaction per device, so one
    /// bad device cannot drop everyone's samples).
    fn flush_metrics(&mut self) {
        if self.metric_buffer.is_empty() {
            return;
        }
        let points = std::mem::take(&mut self.metric_buffer);
        self.write_low(move |pool| async move {
            let mut by_device: HashMap<String, Vec<MetricRow>> = HashMap::new();
            for p in points {
                by_device.entry(p.0.clone()).or_default().push(p);
            }
            let repo = DeviceRepository::new(&pool);
            for (device_id, rows) in by_device {
                if let Err(e) = repo.insert_metrics_batch(&rows).await {
                    warn!("Failed to persist metric history for {}: {}", device_id, e);
                }
            }
        });
    }

    // ------------------------------------------------------------------
    // Alert lifecycle
    // ------------------------------------------------------------------

    fn rule(&self, rule_id: &str) -> Option<&AlertRule> {
        self.rules.iter().find(|r| r.id == rule_id)
    }

    /// True for rule ids something can still evaluate/resolve: an active
    /// configured (or built-in default) rule, the built-in edge-offline
    /// alert, edge-reported (`edge:`) and prediction (`pred-`) alerts.
    fn is_known_rule(&self, rule_id: &str) -> bool {
        rule_id == RULE_EDGE_OFFLINE
            || rule_id.starts_with(EDGE_ALERT_PREFIX)
            || rule_id.starts_with(PREDICTION_ALERT_PREFIX)
            || self.rule(rule_id).is_some()
    }

    fn cooldown_for(&self, rule_id: &str) -> Duration {
        let minutes = self
            .rule(rule_id)
            .map(|r| r.cooldown_minutes as i64)
            .unwrap_or(self.config.default_cooldown_minutes);
        Duration::minutes(minutes.max(0))
    }

    fn channels_for(&self, rule_id: &str) -> Vec<String> {
        self.rule(rule_id)
            .map(|r| r.notification_channels.clone())
            .unwrap_or_default()
    }

    fn notify(&self, alert: &Alert) {
        if let Some(addr) = &self.notifier_addr {
            addr.do_send(SendNotification {
                alert: alert.clone(),
                channels: self.channels_for(&alert.rule_id),
            });
        }
    }

    fn dispatch_action(&self, alert: &Alert) {
        let Some(executor) = &self.action_executor else {
            return;
        };
        let context = ActionContext::for_alert(alert);
        if let Some(spec) = self.config.rule_actions.get(&alert.rule_id) {
            info!(
                "Rule action '{}' dispatched for alert {} on device {}",
                spec.action.id, alert.id, alert.device_id
            );
            executor.do_send(ExecuteRuleAction {
                action: spec.action.clone(),
                on_failure: spec.on_failure.clone(),
                context,
            });
        } else if let Some(action_ref) = self.rule(&alert.rule_id).and_then(|r| r.action.as_ref()) {
            let action = crate::action::actions::action_from_ref(action_ref);
            info!(
                "Rule action '{}' dispatched for alert {} on device {}",
                action.id, alert.id, alert.device_id
            );
            executor.do_send(ExecuteAction { action, context });
        }
    }

    /// Number of currently active alerts for a rule
    fn firing_count_for_rule(&self, rule_id: &str) -> i32 {
        self.active.keys().filter(|k| k.rule_id == rule_id).count() as i32
    }

    /// Open a new alert: persist, notify, run the rule action.
    fn open_alert(&mut self, mut alert: Alert, now: DateTime<Utc>) -> Alert {
        alert.notification_sent = self.notifier_addr.is_some();
        info!("Alert triggered: {} - {}", alert.id, alert.title);
        self.persist_new_alert(&alert);
        self.notify(&alert);
        self.dispatch_action(&alert);
        self.active.insert(
            AlertKey::of(&alert),
            ActiveAlert {
                alert: alert.clone(),
                last_fired: now,
                last_reported: now,
                acknowledged_at: None,
            },
        );
        alert
    }

    /// Re-fire an active alert after its cooldown: bump count, persist,
    /// and (unless acknowledged) re-notify and re-run the rule action.
    fn refire(
        &mut self,
        key: &AlertKey,
        metric_value: Option<f64>,
        message: Option<String>,
        now: DateTime<Utc>,
    ) -> Option<Alert> {
        let active = self.active.get_mut(key)?;
        active.last_fired = now;
        active.last_reported = now;
        active.alert.fired_count += 1;
        if metric_value.is_some() {
            active.alert.metric_value = metric_value;
        }
        if let Some(message) = message {
            active.alert.message = message;
        }
        let alert = active.alert.clone();
        self.persist_refire(&alert, now);
        if alert.status.should_notify() {
            debug!("Alert re-fired: {} ({}x)", alert.id, alert.fired_count);
            self.notify(&alert);
            self.dispatch_action(&alert);
        } else {
            debug!(
                "Alert {} re-fired while {} (no notification/action)",
                alert.id, alert.status
            );
        }
        Some(alert)
    }

    /// Resolve an active alert: memory, database, resolution notice.
    fn resolve_key(&mut self, key: &AlertKey, reason: &str) -> Option<Alert> {
        let active = self.active.remove(key)?;
        let now = Utc::now();
        let alert = Alert {
            status: AlertStatus::Resolved,
            resolved_at: Some(now),
            ..active.alert
        };
        info!(
            "Alert resolved ({}): {} ({})",
            reason, alert.id, alert.title
        );
        self.persist_status(&alert.id, AlertStatus::Resolved, now);
        self.notify(&alert);
        Some(alert)
    }

    fn create_alert(&self, rule: &AlertRule, ctx: &EvaluationContext) -> Alert {
        let (metric_name, metric_value, threshold) = match &rule.condition {
            RuleCondition::MetricThreshold {
                metric_name,
                threshold,
                ..
            } => (
                Some(metric_name.clone()),
                ctx.metrics.get(metric_name).and_then(|v| v.as_f64_finite()),
                Some(*threshold),
            ),
            _ => (None, None, None),
        };

        let title = format!("{}: {}", rule.name, ctx.device_id);
        let message = if rule.description.is_empty() {
            format!(
                "Rule '{}' triggered for device {}",
                rule.name, ctx.device_id
            )
        } else {
            rule.description.clone()
        };

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
            triggered_at: Utc::now(),
            resolved_at: None,
            fired_count: 1,
            notification_sent: false,
        }
    }

    /// Handle a rule whose condition currently holds.
    fn fire_rule(&mut self, rule: &AlertRule, ctx: &EvaluationContext) -> Option<Alert> {
        let now = Utc::now();
        let key = AlertKey::new(&rule.id, &ctx.edge_id, &ctx.device_id);

        // Duration window: only fire after a sustained breach
        if let RuleCondition::MetricThreshold {
            duration_minutes: Some(window),
            ..
        } = rule.condition
        {
            let started = *self.breach_started.entry(key.clone()).or_insert(now);
            let elapsed = now.signed_duration_since(started);
            if elapsed < Duration::minutes(window as i64) {
                debug!(
                    "Rule {} for device {} breaching for {}s of {}min window",
                    rule.id,
                    ctx.device_id,
                    elapsed.num_seconds(),
                    window
                );
                return None;
            }
        }

        let cooldown = Duration::minutes(rule.cooldown_minutes.max(0) as i64);

        if self.active.contains_key(&key) {
            let active = self.active.get_mut(&key)?;
            active.last_reported = now;
            if rule.suppress_repeat {
                return None;
            }
            if now.signed_duration_since(active.last_fired) < cooldown {
                return None;
            }
            let value = match &rule.condition {
                RuleCondition::MetricThreshold { metric_name, .. } => {
                    ctx.metrics.get(metric_name).and_then(|v| v.as_f64_finite())
                }
                _ => None,
            };
            return self.refire(&key, value, None, now);
        }

        if let Some(held) = self.manual_hold.get(&key) {
            if now.signed_duration_since(*held) < cooldown {
                return None;
            }
            self.manual_hold.remove(&key);
        }

        let cap = rule
            .max_firing_count
            .unwrap_or(self.config.max_firing_count);
        if self.firing_count_for_rule(&rule.id) >= cap {
            debug!(
                "Rule {} suppressed: {} active alerts already (cap {})",
                rule.id,
                self.firing_count_for_rule(&rule.id),
                cap
            );
            return None;
        }

        let alert = self.create_alert(rule, ctx);
        Some(self.open_alert(alert, now))
    }

    /// Evaluate every rule for one device. `sweep` evaluations (periodic,
    /// on last-known state) only open time-based rules (offline /
    /// prediction) but auto-resolve any rule whose condition cleared —
    /// unless `restored` (state loaded from the database, not reported
    /// live), which never resolves anything.
    fn evaluate_device(
        &mut self,
        ctx: &EvaluationContext,
        sweep: bool,
        restored: bool,
    ) -> Vec<Alert> {
        let mut fired = Vec::new();
        let rules = std::sync::Arc::clone(&self.rules);
        for rule in rules.iter() {
            let key = AlertKey::new(&rule.id, &ctx.edge_id, &ctx.device_id);
            if rule.is_cleared(ctx) {
                self.breach_started.remove(&key);
                if restored {
                    if self.active.contains_key(&key) {
                        debug!(
                            "Alert for rule {} on device {} kept: device state restored from \
                             the database, waiting for a live report",
                            rule.id, ctx.device_id
                        );
                    }
                } else if self.active.contains_key(&key) {
                    self.resolve_key(&key, "condition cleared");
                }
                continue;
            }
            if sweep && !is_time_based(&rule.condition) {
                continue;
            }
            if rule.evaluate(ctx) {
                if let Some(alert) = self.fire_rule(rule, ctx) {
                    fired.push(alert);
                }
            } else {
                self.breach_started.remove(&key);
            }
        }
        fired
    }

    fn predictions_for(&self, device_id: &str) -> Vec<PredictionInfo> {
        self.predictions
            .get(device_id)
            .map(|m| m.values().map(|(p, _)| p.clone()).collect())
            .unwrap_or_default()
    }

    /// Resolve every active alert of (`edge_id`, `device_id`) whose rule id
    /// matches `pred`. Alerts of the same device id on another edge are
    /// never touched.
    fn resolve_device_alerts(
        &mut self,
        edge_id: &str,
        device_id: &str,
        pred: impl Fn(&str) -> bool,
        reason: &str,
    ) -> usize {
        let keys: Vec<AlertKey> = self
            .active
            .keys()
            .filter(|k| k.edge_id == edge_id && k.device_id == device_id && pred(&k.rule_id))
            .cloned()
            .collect();
        for key in &keys {
            self.resolve_key(key, reason);
        }
        keys.len()
    }

    /// Record that an edge was seen; resolves its offline alert.
    fn mark_edge_seen(&mut self, edge_id: &str, at: DateTime<Utc>) {
        let state = self
            .edges
            .entry(edge_id.to_string())
            .or_insert(EdgeState { last_seen: at });
        if at > state.last_seen {
            state.last_seen = at;
        }
        self.edge_offline_latched.remove(edge_id);
        let key = AlertKey::new(RULE_EDGE_OFFLINE, edge_id, "");
        if self.active.contains_key(&key) {
            self.resolve_key(&key, "edge seen again");
        }
    }

    // ------------------------------------------------------------------
    // Periodic work
    // ------------------------------------------------------------------

    /// Periodic sweep over last-known state: offline detection (devices
    /// and edges), auto-resolve, TTL expiry, pruning.
    fn run_sweep(&mut self) {
        let now = Utc::now();

        // Live sessions refresh edge liveness (heartbeats or any message).
        // A live session heard from within the offline threshold counts as
        // "seen": it resolves an edge-offline alert raised while the same
        // connection was silent (also for edges without devices).
        if let Some(sessions) = self.sessions.clone() {
            let limit = Duration::seconds(self.config.edge_offline_after_secs as i64);
            for session in sessions.snapshot() {
                if let Some(last) = session.try_last_heartbeat() {
                    let edge_id = session.edge_id().to_string();
                    let fresh = self.config.edge_offline_after_secs > 0
                        && now.signed_duration_since(last) <= limit;
                    if fresh {
                        self.mark_edge_seen(&edge_id, last);
                    } else {
                        let state = self
                            .edges
                            .entry(edge_id)
                            .or_insert(EdgeState { last_seen: last });
                        if last > state.last_seen {
                            state.last_seen = last;
                        }
                    }
                }
            }
        }

        // Devices (including those of dead edges)
        let device_ids: Vec<String> = self.devices.keys().cloned().collect();
        for device_id in device_ids {
            let Some(device) = self.devices.get(&device_id) else {
                continue;
            };
            let ctx = EvaluationContext {
                device_id: device_id.clone(),
                edge_id: device.edge_id.clone(),
                current_status: device.status,
                previous_status: Some(device.status),
                metrics: device.metrics.clone(),
                last_poll: device.last_seen,
                predictions: self.predictions_for(&device_id),
            };
            let restored = device.restored;
            self.evaluate_device(&ctx, true, restored);
        }

        // Expire cached predictions
        let prediction_ttl = Duration::minutes(self.config.prediction_ttl_minutes.max(1));
        for per_device in self.predictions.values_mut() {
            per_device.retain(|_, (_, at)| now.signed_duration_since(*at) <= prediction_ttl);
        }
        self.predictions.retain(|_, m| !m.is_empty());

        // TTL-based resolution of edge-reported and prediction alerts
        let edge_ttl = Duration::minutes(self.config.edge_alert_ttl_minutes.max(1));
        let expired: Vec<(AlertKey, &'static str)> = self
            .active
            .iter()
            .filter_map(|(key, active)| {
                let age = now.signed_duration_since(active.last_reported);
                if key.rule_id.starts_with(EDGE_ALERT_PREFIX) && age > edge_ttl {
                    Some((key.clone(), "edge alert not re-reported"))
                } else if key.rule_id.starts_with(PREDICTION_ALERT_PREFIX) && age > prediction_ttl {
                    Some((key.clone(), "no fresh prediction"))
                } else {
                    None
                }
            })
            .collect();
        for (key, reason) in expired {
            self.resolve_key(&key, reason);
        }

        // Built-in edge-offline alert
        if self.config.edge_offline_after_secs > 0 {
            let limit = Duration::seconds(self.config.edge_offline_after_secs as i64);
            let offline: Vec<(String, DateTime<Utc>)> = self
                .edges
                .iter()
                .filter(|(edge_id, state)| {
                    now.signed_duration_since(state.last_seen) > limit
                        && !self.edge_offline_latched.contains(*edge_id)
                        && !self
                            .active
                            .contains_key(&AlertKey::new(RULE_EDGE_OFFLINE, edge_id, ""))
                })
                .map(|(edge_id, state)| (edge_id.clone(), state.last_seen))
                .collect();
            for (edge_id, last_seen) in offline {
                let alert = Alert {
                    id: ulid::Ulid::new().to_string(),
                    rule_id: RULE_EDGE_OFFLINE.to_string(),
                    edge_id: edge_id.clone(),
                    device_id: String::new(),
                    severity: Severity::Critical,
                    status: AlertStatus::Firing,
                    title: format!("Edge offline: {}", edge_id),
                    message: format!(
                        "Edge {} has not been seen for more than {}s (last seen {})",
                        edge_id,
                        self.config.edge_offline_after_secs,
                        last_seen.to_rfc3339()
                    ),
                    metric_name: None,
                    metric_value: None,
                    threshold: None,
                    triggered_at: now,
                    resolved_at: None,
                    fired_count: 1,
                    notification_sent: false,
                };
                self.open_alert(alert, now);
            }
        }

        // Drop per-device state not refreshed within the retention window
        // (kept while the device has an active alert, so that alert stays
        // evaluable, e.g. by the offline rule)
        let retention =
            Duration::hours(self.config.maintenance.device_state_retention_hours.max(1));
        let alerted: HashSet<(&str, &str)> = self
            .active
            .keys()
            .map(|k| (k.edge_id.as_str(), k.device_id.as_str()))
            .collect();
        let stale: Vec<String> = self
            .devices
            .iter()
            .filter(|(id, d)| {
                now.signed_duration_since(d.last_seen) > retention
                    && !alerted.contains(&(d.edge_id.as_str(), id.as_str()))
            })
            .map(|(id, _)| id.clone())
            .collect();
        for device_id in stale {
            debug!("Dropping in-memory state of stale device {}", device_id);
            self.forget_device(&device_id);
        }
        self.edges
            .retain(|_, e| now.signed_duration_since(e.last_seen) <= retention);
        self.manual_hold
            .retain(|_, at| now.signed_duration_since(*at) <= retention);
        let active_keys: HashSet<&AlertKey> = self.active.keys().collect();
        self.breach_started
            .retain(|k, _| active_keys.contains(k) || self.devices.contains_key(&k.device_id));

        // Expire stale active predictions in the database
        if self.has_db() {
            let expiry = self.config.maintenance.prediction_expiry_minutes;
            self.write_low(move |pool| async move {
                if let Err(e) = PredictionRepository::new(&pool).expire_stale(expiry).await {
                    warn!("Failed to expire stale predictions: {}", e);
                }
            });
        }
    }

    /// Drop every per-device map entry for `device_id`.
    fn forget_device(&mut self, device_id: &str) {
        self.devices.remove(device_id);
        self.predictions.remove(device_id);
        self.last_metric_sample.remove(device_id);
        self.db_devices.remove(device_id);
        self.breach_started.retain(|k, _| k.device_id != device_id);
        self.manual_hold.retain(|k, _| k.device_id != device_id);
    }

    /// Retention cleanup: prune resolved alerts, old metrics, predictions
    /// and action history.
    fn run_cleanup(&self) {
        let maintenance = self.config.maintenance.clone();
        self.write_low(move |pool| async move {
            let alerts = AlertRepository::new(&pool);
            let devices = DeviceRepository::new(&pool);
            let predictions = PredictionRepository::new(&pool);
            let actions = nimon_core::db::ActionRepository::new(&pool);
            if let Err(e) = alerts
                .prune_resolved(maintenance.alert_retention_days)
                .await
            {
                warn!("Alert retention cleanup failed: {}", e);
            }
            if let Err(e) = devices
                .prune_metrics(maintenance.metric_retention_days)
                .await
            {
                warn!("Metric retention cleanup failed: {}", e);
            }
            if let Err(e) = predictions
                .prune(maintenance.prediction_retention_days)
                .await
            {
                warn!("Prediction retention cleanup failed: {}", e);
            }
            let cutoff = Utc::now() - Duration::days(maintenance.action_retention_days);
            if let Err(e) = actions.prune_older_than(cutoff).await {
                warn!("Action history retention cleanup failed: {}", e);
            }
        });
    }

    /// Rebuild state from the database after a restart: active alerts
    /// (including acknowledged), recently seen devices and edges.
    fn restore_state(&mut self, ctx: &mut <Self as Actor>::Context) {
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        let retention =
            Duration::hours(self.config.maintenance.device_state_retention_hours.max(1));
        let fut = async move {
            let alerts = AlertRepository::new(&pool).list_active().await;
            let devices = crate::queries::list_devices(&pool, None).await;
            let edges = nimon_core::db::edge_repo::EdgeRepository::new(&pool)
                .list()
                .await;
            (alerts, devices, edges)
        };
        ctx.wait(
            fut.into_actor(self)
                .map(move |(alerts, devices, edges), act, _ctx| {
                    let now = Utc::now();
                    let grace_floor = act.started_at;
                    let row_status = |row: &crate::queries::DeviceStatusRow| {
                        row.status
                            .as_deref()
                            .map(HealthStatus::parse)
                            .unwrap_or(HealthStatus::Offline)
                    };
                    // Rows not restored (never polled / past retention), used
                    // to seed devices that still have active alerts
                    let mut unrestored: HashMap<String, crate::queries::DeviceStatusRow> =
                        HashMap::new();

                    match devices {
                        Ok(rows) => {
                            for row in rows {
                                let Some(last_poll) = row.last_poll() else {
                                    unrestored.insert(row.id.clone(), row);
                                    continue;
                                };
                                if now.signed_duration_since(last_poll) > retention {
                                    unrestored.insert(row.id.clone(), row);
                                    continue;
                                }
                                act.db_devices
                                    .insert(row.id.clone(), DeviceMetadata::default());
                                act.devices.insert(
                                    row.id.clone(),
                                    KnownDevice {
                                        edge_id: row.edge_id.clone(),
                                        status: row_status(&row),
                                        metrics: row.metrics(),
                                        // Offline clocks restart at hub start
                                        last_seen: last_poll.max(grace_floor),
                                        restored: true,
                                    },
                                );
                            }
                        }
                        Err(e) => warn!("Failed to restore device state: {}", e),
                    }

                    match edges {
                        Ok(list) => {
                            for edge in list {
                                if let Some(last_seen) = edge.last_seen {
                                    if now.signed_duration_since(last_seen) <= retention {
                                        act.edges.insert(
                                            edge.id.clone(),
                                            EdgeState {
                                                last_seen: last_seen.max(grace_floor),
                                            },
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => warn!("Failed to restore edge state: {}", e),
                    }

                    match alerts {
                        Ok(records) => {
                            let mut restored = 0;
                            let mut orphaned = 0;
                            // list_active is newest first: keep the newest per key
                            for record in records {
                                let status = record.status();
                                if !status.is_active() {
                                    continue;
                                }
                                let alert = record.to_alert();
                                let key = AlertKey::of(&alert);
                                if act.active.contains_key(&key) {
                                    info!(
                                        "Resolving duplicate active alert {} (rule {}, device {})",
                                        alert.id, alert.rule_id, alert.device_id
                                    );
                                    act.persist_status(&alert.id, AlertStatus::Resolved, now);
                                    continue;
                                }
                                if !act.is_known_rule(&alert.rule_id) {
                                    // Nothing could ever evaluate (and so
                                    // resolve) it; resolved quietly, no notice
                                    info!(
                                        "Resolving restored alert {} (device {}): rule '{}' is no \
                                         longer configured",
                                        alert.id, alert.device_id, alert.rule_id
                                    );
                                    act.persist_status(&alert.id, AlertStatus::Resolved, now);
                                    orphaned += 1;
                                    continue;
                                }
                                if !alert.device_id.is_empty() {
                                    act.db_devices.entry(alert.device_id.clone()).or_default();
                                }
                                // A configured-rule alert of a device whose
                                // state was not restored (past retention):
                                // seed it so sweeps keep evaluating it
                                if act.rule(&alert.rule_id).is_some()
                                    && !alert.device_id.is_empty()
                                    && alert.device_id != alert.edge_id
                                    && !act.devices.contains_key(&alert.device_id)
                                {
                                    let row = unrestored.get(&alert.device_id);
                                    act.devices.insert(
                                        alert.device_id.clone(),
                                        KnownDevice {
                                            edge_id: alert.edge_id.clone(),
                                            status: row
                                                .map(row_status)
                                                .unwrap_or(HealthStatus::Offline),
                                            metrics: row.map(|r| r.metrics()).unwrap_or_default(),
                                            last_seen: grace_floor,
                                            restored: true,
                                        },
                                    );
                                    debug!(
                                        "Seeded state of device {} for restored alert {}",
                                        alert.device_id, alert.id
                                    );
                                }
                                let last_fired = record
                                    .last_fired_at
                                    .as_deref()
                                    .and_then(crate::queries::parse_ts)
                                    .unwrap_or(alert.triggered_at);
                                let acknowledged_at = record
                                    .acknowledged_at
                                    .as_deref()
                                    .and_then(crate::queries::parse_ts);
                                act.active.insert(
                                    key,
                                    ActiveAlert {
                                        alert,
                                        last_fired,
                                        last_reported: now,
                                        acknowledged_at,
                                    },
                                );
                                restored += 1;
                            }
                            if restored > 0 {
                                info!("Restored {} active alerts from database", restored);
                            }
                            if orphaned > 0 {
                                warn!(
                                    "Resolved {} restored alerts whose rule no longer exists \
                                     (renamed or removed rules)",
                                    orphaned
                                );
                            }
                        }
                        Err(e) => warn!("Failed to restore active alerts: {}", e),
                    }
                }),
        );
    }

    fn start_notifier(&mut self) {
        // The console channel is implied when no console channel is
        // configured so default deployments still see alerts.
        let mut channels: Vec<NotificationChannel> = self
            .config
            .notification_channels
            .iter()
            .map(notification_channel_from_config)
            .collect();

        if !channels
            .iter()
            .any(|c| matches!(c.channel_type, ChannelType::Console))
        {
            channels.push(NotificationChannel {
                id: "console".to_string(),
                name: "console".to_string(),
                enabled: true,
                channel_type: ChannelType::Console,
                config: serde_json::json!({}),
            });
        }

        self.notifier_addr = Some(AlertNotifier::new(channels).start());
    }
}

/// Build the notifier channel for one configured channel. SMTP
/// credentials follow the documented precedence (env over config).
pub fn notification_channel_from_config(
    c: &crate::config::NotificationChannelConfig,
) -> NotificationChannel {
    let headers = c.headers.as_ref().and_then(|pairs| {
        let map: HashMap<String, String> = pairs
            .iter()
            .filter(|p| p.len() == 2)
            .map(|p| (p[0].clone(), p[1].clone()))
            .collect();
        (!map.is_empty()).then_some(map)
    });
    let (smtp_username, smtp_password) = if c.channel_type == "email" {
        c.smtp_credentials()
    } else {
        (None, None)
    };
    let channel_config = match c.channel_type.as_str() {
        "email" => serde_json::json!({
            "smtp_skip_tls_verify": c.smtp_skip_tls_verify.unwrap_or(false),
            "smtp_password": smtp_password,
            "smtp_tls": c.smtp_tls,
            "smtp_allow_plaintext_auth": c.smtp_allow_plaintext_auth,
        }),
        _ => serde_json::json!({}),
    };
    let channel_type = match c.channel_type.as_str() {
        "email" => ChannelType::Email {
            smtp_server: c
                .smtp_server
                .clone()
                .unwrap_or_else(|| "localhost".to_string()),
            smtp_port: c.smtp_port.unwrap_or(587),
            username: smtp_username,
            from_addr: c
                .from_addr
                .clone()
                .unwrap_or_else(|| "nimon@localhost".to_string()),
            to_addrs: c
                .to_addrs
                .clone()
                .unwrap_or_else(|| vec!["admin@localhost".to_string()]),
        },
        "slack" => ChannelType::Slack {
            webhook_url: c.webhook_url.clone().unwrap_or_default(),
        },
        "teams" => ChannelType::Teams {
            webhook_url: c.webhook_url.clone().unwrap_or_default(),
        },
        "webhook" => ChannelType::Webhook {
            url: c.webhook_url.clone().unwrap_or_default(),
            headers,
        },
        "console" => ChannelType::Console,
        other => {
            warn!(
                "Unsupported notification channel type '{}', using console",
                other
            );
            ChannelType::Console
        }
    };
    let id = c.name.clone().unwrap_or_else(|| c.channel_type.clone());
    NotificationChannel {
        id: id.clone(),
        name: id,
        enabled: c.enabled,
        channel_type,
        config: channel_config,
    }
}

/// Descriptive fields carried in the metrics map of a status update.
fn extract_metadata(msg: &DeviceStatusUpdate) -> DeviceMetadata {
    let text = |keys: &[&str]| {
        keys.iter().find_map(|k| match msg.metrics.get(*k) {
            Some(MetricValue::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        })
    };
    DeviceMetadata {
        model: text(&["product", "model", "product_type"]),
        serial_number: text(&["serial_number", "serial"]),
        slot: msg
            .metrics
            .get("slot")
            .and_then(|v| v.as_f64_finite())
            .map(|v| v as i32),
        chassis: text(&["chassis"]),
        firmware_version: text(&["firmware_version"]),
        driver_version: text(&["driver_version"]),
        is_simulated: Some(msg.is_simulated),
        ..Default::default()
    }
}

/// Conditions whose truth changes with time alone (evaluated by sweeps).
fn is_time_based(condition: &RuleCondition) -> bool {
    match condition {
        RuleCondition::DeviceOffline { .. } | RuleCondition::Prediction { .. } => true,
        RuleCondition::Composite { conditions, .. } => conditions.iter().any(is_time_based),
        _ => false,
    }
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Critical => 2,
        Severity::Warning => 1,
        Severity::Info => 0,
    }
}

impl Actor for AlertManager {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("AlertManager actor started with {} rules", self.rules.len());
        self.started_at = Utc::now();
        if self.db_writer.is_none() {
            if let Some(pool) = &self.db_pool {
                self.db_writer = Some(DbWriter::spawn(pool.clone()));
            }
        }

        self.start_notifier();
        self.restore_state(ctx);

        ctx.run_interval(
            std::time::Duration::from_secs(self.config.evaluate_interval_secs.max(1)),
            |actor, _ctx| actor.run_sweep(),
        );
        ctx.run_interval(
            std::time::Duration::from_secs(
                self.config.maintenance.metric_flush_interval_secs.max(1),
            ),
            |actor, _ctx| actor.flush_metrics(),
        );
        ctx.run_interval(
            std::time::Duration::from_secs(
                (self.config.cleanup_interval_hours.max(1) as u64) * 3600,
            ),
            |actor, _ctx| actor.run_cleanup(),
        );
    }

    fn stopping(&mut self, _ctx: &mut Self::Context) -> Running {
        self.flush_metrics();
        Running::Stop
    }
}

// ----------------------------------------------------------------------
// Messages
// ----------------------------------------------------------------------

/// Evaluate rules against a context (returns alerts opened or re-fired)
#[derive(Message)]
#[rtype(result = "Vec<Alert>")]
pub struct EvaluateRules {
    pub context: EvaluationContext,
}

impl Handler<EvaluateRules> for AlertManager {
    type Result = Vec<Alert>;

    fn handle(&mut self, msg: EvaluateRules, _ctx: &mut Self::Context) -> Self::Result {
        self.evaluate_device(&msg.context, false, false)
    }
}

/// Run one periodic sweep now (tests / diagnostics)
#[derive(Message)]
#[rtype(result = "()")]
pub struct RunSweep;

impl Handler<RunSweep> for AlertManager {
    type Result = ();

    fn handle(&mut self, _msg: RunSweep, _ctx: &mut Self::Context) -> Self::Result {
        self.run_sweep();
    }
}

/// Flush buffered metric history now (tests / shutdown)
#[derive(Message)]
#[rtype(result = "()")]
pub struct FlushMetrics;

impl Handler<FlushMetrics> for AlertManager {
    type Result = ();

    fn handle(&mut self, _msg: FlushMetrics, _ctx: &mut Self::Context) -> Self::Result {
        self.flush_metrics();
    }
}

/// Liveness probe (health endpoint)
#[derive(Message)]
#[rtype(result = "()")]
pub struct Ping;

impl Handler<Ping> for AlertManager {
    type Result = ();

    fn handle(&mut self, _msg: Ping, _ctx: &mut Self::Context) -> Self::Result {}
}

/// Edge connection lifecycle events from the WebSocket layer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeActivityKind {
    Connected,
    Seen,
    Disconnected,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct EdgeActivity {
    pub edge_id: String,
    pub kind: EdgeActivityKind,
}

impl Handler<EdgeActivity> for AlertManager {
    type Result = ();

    fn handle(&mut self, msg: EdgeActivity, _ctx: &mut Self::Context) -> Self::Result {
        let now = Utc::now();
        match msg.kind {
            EdgeActivityKind::Connected | EdgeActivityKind::Seen => {
                self.mark_edge_seen(&msg.edge_id, now)
            }
            EdgeActivityKind::Disconnected => {
                // The offline clock runs from the last time the edge was
                // heard from (now, when never tracked); the alert fires after
                // `edge_offline_after_secs` unless the edge reconnects.
                self.edges
                    .entry(msg.edge_id)
                    .or_insert(EdgeState { last_seen: now });
            }
        }
    }
}

/// Ingest a device alert reported by an edge node
#[derive(Message)]
#[rtype(result = "()")]
pub struct IngestDeviceAlert {
    pub alert: nimon_core::actor::messages::DeviceAlert,
}

impl Handler<IngestDeviceAlert> for AlertManager {
    type Result = ();

    fn handle(&mut self, msg: IngestDeviceAlert, _ctx: &mut Self::Context) -> Self::Result {
        let device_alert = msg.alert;
        let now = Utc::now();
        let rule_id = format!(
            "{}{}",
            EDGE_ALERT_PREFIX,
            device_alert
                .metric_name
                .as_deref()
                .unwrap_or("device-alert")
        );
        let key = AlertKey::new(&rule_id, &device_alert.edge_id, &device_alert.device_id);

        if let Some(active) = self.active.get_mut(&key) {
            // Same problem re-reported: keep the id, refresh TTL, re-fire
            // only after the cooldown.
            active.last_reported = now;
            if now.signed_duration_since(active.last_fired) >= self.cooldown_for(&rule_id) {
                self.refire(
                    &key,
                    device_alert.metric_value,
                    Some(device_alert.message.clone()),
                    now,
                );
            }
            return;
        }
        if let Some(held) = self.manual_hold.get(&key) {
            if now.signed_duration_since(*held) < self.cooldown_for(&rule_id) {
                return;
            }
            self.manual_hold.remove(&key);
        }

        let alert = Alert {
            id: ulid::Ulid::new().to_string(),
            rule_id,
            edge_id: device_alert.edge_id.clone(),
            device_id: device_alert.device_id.clone(),
            severity: device_alert.severity,
            status: AlertStatus::Firing,
            title: format!("Edge alert: {}", device_alert.message),
            message: device_alert.message.clone(),
            metric_name: device_alert.metric_name.clone(),
            metric_value: device_alert.metric_value,
            threshold: None,
            triggered_at: now,
            resolved_at: None,
            fired_count: 1,
            notification_sent: false,
        };
        self.open_alert(alert, now);
    }
}

/// A device disappeared from discovery: drop its state, resolve its alerts.
impl Handler<DeviceRemoved> for AlertManager {
    type Result = ();

    fn handle(&mut self, msg: DeviceRemoved, _ctx: &mut Self::Context) -> Self::Result {
        info!("Device {} removed by edge {}", msg.device_id, msg.edge_id);
        match self.devices.get(&msg.device_id) {
            Some(known) if known.edge_id != msg.edge_id => warn!(
                "Edge {} reported removal of device {} owned by edge {}; keeping its state",
                msg.edge_id, msg.device_id, known.edge_id
            ),
            _ => self.forget_device(&msg.device_id),
        }
        self.resolve_device_alerts(&msg.edge_id, &msg.device_id, |_| true, "device removed");
    }
}

/// Message to get active predictions from the database
#[derive(Message)]
#[rtype(result = "Result<Vec<serde_json::Value>, String>")]
pub struct GetRecentPredictions {
    pub limit: usize,
}

impl Handler<GetRecentPredictions> for AlertManager {
    type Result = ResponseActFuture<Self, Result<Vec<serde_json::Value>, String>>;

    fn handle(&mut self, msg: GetRecentPredictions, _ctx: &mut Self::Context) -> Self::Result {
        let pool = self.db_pool.clone();
        let limit = msg.limit;
        let fut = async move {
            let pool = pool.ok_or("No database pool")?;
            let repo = PredictionRepository::new(&pool);
            let predictions = repo.list_active().await.map_err(|e| e.to_string())?;
            // Newest first: keep the latest per (device, type)
            let mut seen = HashSet::new();
            let json: Vec<serde_json::Value> = predictions
                .into_iter()
                .filter(|p| seen.insert((p.device_id.clone(), p.prediction_type.clone())))
                .take(limit)
                .map(|p| {
                    serde_json::json!({
                        "id": p.id,
                        "device_id": p.device_id,
                        "edge_id": p.edge_id,
                        "prediction_type": p.prediction_type,
                        "probability": p.probability,
                        "eta_minutes": p.eta_minutes,
                        "status": p.status,
                        "created_at": p.created_at,
                    })
                })
                .collect();
            Ok(json)
        }
        .into_actor(self);
        Box::pin(fut)
    }
}

/// Message to get active alerts (firing, pending, acknowledged), sorted
/// by severity (critical first) then newest first.
#[derive(Message)]
#[rtype(result = "Vec<Alert>")]
pub struct GetActiveAlerts;

impl Handler<GetActiveAlerts> for AlertManager {
    type Result = Vec<Alert>;

    fn handle(&mut self, _msg: GetActiveAlerts, _ctx: &mut Self::Context) -> Self::Result {
        self.active_views().into_iter().map(|v| v.alert).collect()
    }
}

/// An active alert plus its lifecycle timestamps (API view)
#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveAlertView {
    #[serde(flatten)]
    pub alert: Alert,
    pub last_fired_at: DateTime<Utc>,
    pub acknowledged_at: Option<DateTime<Utc>>,
}

/// Active alerts with lifecycle timestamps, in `GetActiveAlerts` order
#[derive(Message)]
#[rtype(result = "Vec<ActiveAlertView>")]
pub struct GetActiveAlertViews;

impl Handler<GetActiveAlertViews> for AlertManager {
    type Result = Vec<ActiveAlertView>;

    fn handle(&mut self, _msg: GetActiveAlertViews, _ctx: &mut Self::Context) -> Self::Result {
        self.active_views()
    }
}

impl AlertManager {
    /// Active alerts sorted by severity (critical first), then newest
    fn active_views(&self) -> Vec<ActiveAlertView> {
        let mut views: Vec<ActiveAlertView> = self
            .active
            .values()
            .filter(|a| a.alert.status.is_active())
            .map(|a| ActiveAlertView {
                alert: a.alert.clone(),
                last_fired_at: a.last_fired,
                acknowledged_at: a.acknowledged_at,
            })
            .collect();
        views.sort_by(|a, b| {
            severity_rank(b.alert.severity)
                .cmp(&severity_rank(a.alert.severity))
                .then(b.alert.triggered_at.cmp(&a.alert.triggered_at))
                .then(b.alert.id.cmp(&a.alert.id))
        });
        views
    }
}

/// Acknowledge an alert: it stays active (visible, auto-resolvable) but
/// is no longer re-notified and its rule action is not re-run. Persisted.
/// Errors: `AlertNotFound`, `InvalidState` (already resolved).
#[derive(Message)]
#[rtype(result = "Result<Alert, nimon_core::NimonError>")]
pub struct AcknowledgeAlert {
    pub alert_id: String,
}

impl Handler<AcknowledgeAlert> for AlertManager {
    type Result = ResponseActFuture<Self, Result<Alert, NimonError>>;

    fn handle(&mut self, msg: AcknowledgeAlert, _ctx: &mut Self::Context) -> Self::Result {
        let now = Utc::now();
        let alert_id = msg.alert_id;
        let in_memory = self
            .active
            .values_mut()
            .find(|a| a.alert.id == alert_id)
            .map(|a| {
                a.alert.status = AlertStatus::Acknowledged;
                a.acknowledged_at.get_or_insert(now);
                a.alert.clone()
            });
        if let Some(alert) = &in_memory {
            info!("Alert acknowledged: {}", alert.id);
        }
        let writer = self.db_writer.clone();
        let fut = async move {
            let Some(writer) = writer else {
                return in_memory.ok_or(NimonError::AlertNotFound(alert_id));
            };
            let id = alert_id.clone();
            let outcome = writer
                .call(move |pool| async move {
                    let repo = AlertRepository::new(&pool);
                    let rows = repo.set_status(&id, AlertStatus::Acknowledged, now).await?;
                    if rows > 0 {
                        return repo.get(&id).await.map(|r| Some(r.to_alert()));
                    }
                    // Unknown, or refused because already resolved
                    repo.get(&id).await.map(|_| None)
                })
                .await
                .map_err(|_| NimonError::InvalidState("database writer stopped".into()))?;
            match (outcome, in_memory) {
                (Ok(_), Some(alert)) => Ok(alert),
                (Ok(Some(alert)), None) => Ok(alert),
                (Ok(None), _) => Err(NimonError::InvalidState(format!(
                    "alert {} is already resolved",
                    alert_id
                ))),
                (Err(NimonError::AlertNotFound(_)), Some(alert)) => Ok(alert),
                (Err(e), _) => Err(e),
            }
        }
        .into_actor(self);
        Box::pin(fut)
    }
}

/// Resolve an alert manually. Persisted; a resolution notice is sent.
/// The same rule/device does not re-fire until its cooldown elapsed (the
/// edge-offline alert: until the edge is seen again).
#[derive(Message)]
#[rtype(result = "Result<(), nimon_core::NimonError>")]
pub struct ResolveAlert {
    pub alert_id: String,
}

impl Handler<ResolveAlert> for AlertManager {
    type Result = ResponseActFuture<Self, Result<(), NimonError>>;

    fn handle(&mut self, msg: ResolveAlert, _ctx: &mut Self::Context) -> Self::Result {
        let alert_id = msg.alert_id;
        let key = self
            .active
            .iter()
            .find(|(_, a)| a.alert.id == alert_id)
            .map(|(k, _)| k.clone());
        let found_in_memory = key.is_some();
        if let Some(key) = key {
            if key.rule_id == RULE_EDGE_OFFLINE {
                self.edge_offline_latched.insert(key.edge_id.clone());
            } else {
                self.manual_hold.insert(key.clone(), Utc::now());
            }
            self.resolve_key(&key, "resolved manually");
        }

        let writer = self.db_writer.clone();
        let fut = async move {
            let Some(writer) = writer else {
                return if found_in_memory {
                    Ok(())
                } else {
                    Err(NimonError::AlertNotFound(alert_id))
                };
            };
            let id = alert_id.clone();
            // Runs after the resolve write queued above (ordered writer)
            let outcome = writer
                .call(move |pool| async move {
                    let repo = AlertRepository::new(&pool);
                    let record = repo.get(&id).await?;
                    if record.status() != AlertStatus::Resolved {
                        repo.set_status(&id, AlertStatus::Resolved, Utc::now())
                            .await?;
                    }
                    Ok::<(), NimonError>(())
                })
                .await
                .map_err(|_| NimonError::InvalidState("database writer stopped".into()))?;
            match outcome {
                Ok(()) => Ok(()),
                Err(NimonError::AlertNotFound(_)) if found_in_memory => Ok(()),
                Err(e) => Err(e),
            }
        }
        .into_actor(self);
        Box::pin(fut)
    }
}

/// Handle device status updates
impl Handler<DeviceStatusUpdate> for AlertManager {
    type Result = ();

    fn handle(&mut self, msg: DeviceStatusUpdate, _ctx: &mut Self::Context) -> Self::Result {
        let now = Utc::now();
        self.mark_edge_seen(&msg.edge_id, now);

        let previous_status = self.devices.get(&msg.device_id).map(|d| d.status);
        self.devices.insert(
            msg.device_id.clone(),
            KnownDevice {
                edge_id: msg.edge_id.clone(),
                status: msg.status,
                metrics: msg.metrics.clone(),
                last_seen: now,
                restored: false,
            },
        );

        // Device rows first so alert rows referencing them resolve
        self.persist_device_state(&msg, now);

        let eval_ctx = EvaluationContext {
            device_id: msg.device_id.clone(),
            edge_id: msg.edge_id.clone(),
            current_status: msg.status,
            previous_status,
            metrics: msg.metrics,
            last_poll: now,
            predictions: self.predictions_for(&msg.device_id),
        };
        self.evaluate_device(&eval_ctx, false, false);

        if msg.status == HealthStatus::Healthy {
            self.resolve_device_alerts(
                &msg.edge_id,
                &msg.device_id,
                |rule_id| rule_id.starts_with(EDGE_ALERT_PREFIX),
                "device healthy again",
            );
        }
    }
}

/// Handle prediction results: every prediction is persisted; those at or
/// above `prediction_alert_threshold` raise (or refresh) an alert.
impl Handler<PredictionResult> for AlertManager {
    type Result = ();

    fn handle(&mut self, msg: PredictionResult, _ctx: &mut Self::Context) -> Self::Result {
        let now = Utc::now();
        let prediction_type = msg.prediction_type.to_string();
        self.predictions
            .entry(msg.device_id.clone())
            .or_default()
            .insert(
                prediction_type.clone(),
                (
                    PredictionInfo {
                        prediction_type: prediction_type.clone(),
                        probability: msg.probability,
                        eta_minutes: msg.eta_minutes,
                    },
                    now,
                ),
            );

        if self.has_db() {
            let (device_id, edge_id, ptype) = (
                msg.device_id.clone(),
                msg.edge_id.clone(),
                prediction_type.clone(),
            );
            let probability = msg.probability;
            let eta_minutes = msg.eta_minutes.map(|m| m as i64);
            let model_version = msg.model_version.clone();
            self.write_low(move |pool| async move {
                if let Err(e) = PredictionRepository::new(&pool)
                    .insert(
                        &device_id,
                        &edge_id,
                        &ptype,
                        probability,
                        eta_minutes,
                        model_version.as_deref(),
                    )
                    .await
                {
                    warn!("Failed to persist prediction: {}", e);
                }
            });
        }

        if !msg.probability.is_finite() || msg.probability < self.config.prediction_alert_threshold
        {
            return;
        }

        let rule_id = format!("{}{}", PREDICTION_ALERT_PREFIX, prediction_type);
        let key = AlertKey::new(&rule_id, &msg.edge_id, &msg.device_id);
        let message = msg.reason.clone().unwrap_or_else(|| {
            format!(
                "Predicted {} with {:.0}% confidence",
                prediction_type,
                msg.probability * 100.0
            )
        });

        if let Some(active) = self.active.get_mut(&key) {
            active.last_reported = now;
            if now.signed_duration_since(active.last_fired) >= self.cooldown_for(&rule_id) {
                self.refire(&key, Some(msg.probability), Some(message), now);
            }
            return;
        }
        if let Some(held) = self.manual_hold.get(&key) {
            if now.signed_duration_since(*held) < self.cooldown_for(&rule_id) {
                return;
            }
            self.manual_hold.remove(&key);
        }

        let alert = Alert {
            id: ulid::Ulid::new().to_string(),
            rule_id,
            edge_id: msg.edge_id.clone(),
            device_id: msg.device_id.clone(),
            severity: if msg.probability >= PREDICTION_CRITICAL_THRESHOLD {
                Severity::Critical
            } else {
                Severity::Warning
            },
            status: AlertStatus::Firing,
            title: format!("{} prediction for {}", prediction_type, msg.device_id),
            message,
            metric_name: None,
            metric_value: Some(msg.probability),
            threshold: Some(self.config.prediction_alert_threshold),
            triggered_at: now,
            resolved_at: None,
            fired_count: 1,
            notification_sent: false,
        };
        info!("Prediction alert: {}", alert.title);
        self.open_alert(alert, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> AlertManagerConfig {
        AlertManagerConfig::default()
    }

    fn eval_ctx(device_id: &str, temp: f64, status: HealthStatus) -> EvaluationContext {
        let mut metrics = std::collections::HashMap::new();
        metrics.insert("temperature".to_string(), MetricValue::Float(temp));
        EvaluationContext {
            device_id: device_id.to_string(),
            edge_id: "edge-1".to_string(),
            current_status: status,
            previous_status: None,
            metrics,
            last_poll: Utc::now(),
            predictions: vec![],
        }
    }

    fn status(device: &str, temp: f64, status: HealthStatus) -> DeviceStatusUpdate {
        DeviceStatusUpdate {
            device_id: device.to_string(),
            edge_id: "edge-1".to_string(),
            status,
            metrics: [("temperature".to_string(), MetricValue::Float(temp))].into(),
            timestamp: Utc::now(),
            is_simulated: false,
        }
    }

    fn threshold_rule(id: &str, threshold: f64, cooldown: i32) -> AlertRule {
        AlertRule {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            enabled: true,
            severity: Severity::Critical,
            condition: RuleCondition::MetricThreshold {
                metric_name: "temperature".to_string(),
                operator: ComparisonOp::GreaterThan,
                threshold,
                duration_minutes: None,
                hysteresis: None,
            },
            cooldown_minutes: cooldown,
            notification_channels: vec![],
            suppress_repeat: false,
            max_firing_count: None,
            action: None,
        }
    }

    #[test]
    fn test_config_default() {
        let config = AlertManagerConfig::default();
        assert_eq!(config.default_cooldown_minutes, 5);
        assert_eq!(config.max_firing_count, 100);
        assert_eq!(config.prediction_alert_threshold, 0.8);
    }

    #[test]
    fn test_all_rules_failed_disables_defaults() {
        let config: HubConfig = serde_yaml::from_str(
            "alert:\n  rules:\n  - name: bad\n    severity: extreme\n    condition: { metric: t, threshold: 1 }\n",
        )
        .unwrap();
        let manager_config = AlertManagerConfig::from_hub_config(&config);
        assert!(manager_config.disable_default_rules);
        assert!(AlertManager::new(manager_config).rules.is_empty());
        assert_eq!(
            AlertManager::new(AlertManagerConfig::default()).rules.len(),
            4
        );
    }

    #[actix::test]
    async fn test_status_update_fires_temperature_alert() {
        let addr = AlertManager::new(test_config()).start();

        addr.send(status("dev-1", 50.0, HealthStatus::Healthy))
            .await
            .unwrap();
        assert!(addr.send(GetActiveAlerts).await.unwrap().is_empty());

        addr.send(status("dev-1", 70.0, HealthStatus::Healthy))
            .await
            .unwrap();
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].rule_id, "rule-high-temp");
    }

    #[actix::test]
    async fn test_previous_status_enables_status_change_rule() {
        let addr = AlertManager::new(test_config()).start();

        addr.send(status("dev-2", 20.0, HealthStatus::Healthy))
            .await
            .unwrap();
        addr.send(status("dev-2", 20.0, HealthStatus::Error))
            .await
            .unwrap();

        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert!(active.iter().any(|a| a.rule_id == "rule-device-error"));
    }

    #[actix::test]
    async fn test_status_change_alert_stays_while_in_error_and_resolves_on_recovery() {
        let addr = AlertManager::new(test_config()).start();
        addr.send(status("dev-e", 20.0, HealthStatus::Healthy))
            .await
            .unwrap();
        addr.send(status("dev-e", 20.0, HealthStatus::Error))
            .await
            .unwrap();
        // Still in Error: stays active through status updates and sweeps
        addr.send(status("dev-e", 20.0, HealthStatus::Error))
            .await
            .unwrap();
        addr.send(RunSweep).await.unwrap();
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert!(active.iter().any(|a| a.rule_id == "rule-device-error"));

        // Recovered: resolved
        addr.send(status("dev-e", 20.0, HealthStatus::Healthy))
            .await
            .unwrap();
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert!(!active.iter().any(|a| a.rule_id == "rule-device-error"));
    }

    #[actix::test]
    async fn test_hysteresis_keeps_alert_inside_band() {
        let mut config = test_config();
        let mut rule = threshold_rule("hot", 75.0, 0);
        rule.condition = RuleCondition::MetricThreshold {
            metric_name: "temperature".to_string(),
            operator: ComparisonOp::GreaterThan,
            threshold: 75.0,
            duration_minutes: None,
            hysteresis: Some(2.0),
        };
        config.rules = vec![rule];
        let addr = AlertManager::new(config).start();

        addr.send(status("d", 80.0, HealthStatus::Healthy))
            .await
            .unwrap();
        addr.send(status("d", 74.0, HealthStatus::Healthy))
            .await
            .unwrap();
        assert_eq!(
            addr.send(GetActiveAlerts).await.unwrap().len(),
            1,
            "inside band"
        );
        addr.send(status("d", 72.5, HealthStatus::Healthy))
            .await
            .unwrap();
        assert!(
            addr.send(GetActiveAlerts).await.unwrap().is_empty(),
            "past band"
        );
    }

    #[actix::test]
    async fn test_refire_keeps_id_and_respects_cooldown() {
        let mut config = test_config();
        config.rules = vec![threshold_rule("hot", 80.0, 5)];
        let addr = AlertManager::new(config).start();

        let first = addr
            .send(EvaluateRules {
                context: eval_ctx("d", 90.0, HealthStatus::Healthy),
            })
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        // Within cooldown: nothing re-fires
        let second = addr
            .send(EvaluateRules {
                context: eval_ctx("d", 91.0, HealthStatus::Healthy),
            })
            .await
            .unwrap();
        assert!(second.is_empty());
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, first[0].id);
        assert_eq!(active[0].fired_count, 1);
    }

    #[actix::test]
    async fn test_acknowledge_keeps_alert_active() {
        let mut config = test_config();
        config.rules = vec![threshold_rule("hot", 85.0, 0)];
        let addr = AlertManager::new(config).start();

        addr.send(EvaluateRules {
            context: eval_ctx("dev-3", 90.0, HealthStatus::Healthy),
        })
        .await
        .unwrap();
        let alert_id = addr.send(GetActiveAlerts).await.unwrap()[0].id.clone();

        let acked = addr
            .send(AcknowledgeAlert {
                alert_id: alert_id.clone(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(acked.status, AlertStatus::Acknowledged);

        // Still active, still acknowledged after a re-fire (cooldown 0)
        let refired = addr
            .send(EvaluateRules {
                context: eval_ctx("dev-3", 95.0, HealthStatus::Healthy),
            })
            .await
            .unwrap();
        assert_eq!(refired[0].status, AlertStatus::Acknowledged);
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].status, AlertStatus::Acknowledged);

        // Clears: auto-resolved even though acknowledged
        addr.send(EvaluateRules {
            context: eval_ctx("dev-3", 50.0, HealthStatus::Healthy),
        })
        .await
        .unwrap();
        assert!(addr.send(GetActiveAlerts).await.unwrap().is_empty());

        // Unknown alert
        assert!(addr
            .send(AcknowledgeAlert {
                alert_id: "nope".into()
            })
            .await
            .unwrap()
            .is_err());
    }

    #[actix::test]
    async fn test_resolve_removes_active_alert() {
        let mut config = test_config();
        config.rules = vec![threshold_rule("rule-critical-only", 85.0, 0)];
        let addr = AlertManager::new(config).start();

        addr.send(EvaluateRules {
            context: eval_ctx("dev-3", 90.0, HealthStatus::Healthy),
        })
        .await
        .unwrap();

        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert!(!active.is_empty());
        let alert_id = active[0].id.clone();

        let result = addr.send(ResolveAlert { alert_id }).await.unwrap();
        assert!(result.is_ok());
        assert!(addr.send(GetActiveAlerts).await.unwrap().is_empty());
    }

    #[actix::test]
    async fn test_duration_window_delays_firing() {
        let mut config = test_config();
        let mut rule = threshold_rule("rule-sustained", 70.0, 0);
        rule.condition = RuleCondition::MetricThreshold {
            metric_name: "temperature".to_string(),
            operator: ComparisonOp::GreaterThan,
            threshold: 70.0,
            duration_minutes: Some(10),
            hysteresis: None,
        };
        config.rules = vec![rule];
        let addr = AlertManager::new(config).start();

        for _ in 0..2 {
            addr.send(EvaluateRules {
                context: eval_ctx("dev-4", 80.0, HealthStatus::Healthy),
            })
            .await
            .unwrap();
            let active = addr.send(GetActiveAlerts).await.unwrap();
            assert!(active.is_empty(), "duration window should delay firing");
        }
    }

    #[actix::test]
    async fn test_max_firing_count_suppression() {
        let mut config = test_config();
        config.max_firing_count = 2;
        config.rules = vec![threshold_rule("rule-hot", 70.0, 0)];
        let addr = AlertManager::new(config).start();

        for device in ["d1", "d2", "d3", "d4"] {
            addr.send(EvaluateRules {
                context: eval_ctx(device, 80.0, HealthStatus::Healthy),
            })
            .await
            .unwrap();
        }

        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert_eq!(active.len(), 2, "suppression beyond max_firing_count");
    }

    #[actix::test]
    async fn test_edge_alert_keeps_id_and_resolves_when_healthy() {
        let addr = AlertManager::new(test_config()).start();
        let report = || IngestDeviceAlert {
            alert: nimon_core::actor::messages::DeviceAlert {
                device_id: "dx".into(),
                edge_id: "edge-1".into(),
                severity: Severity::Warning,
                message: "fan stalled".into(),
                metric_name: Some("fan".into()),
                metric_value: Some(0.0),
                timestamp: Utc::now(),
            },
        };
        addr.send(report()).await.unwrap();
        let first = addr.send(GetActiveAlerts).await.unwrap();
        addr.send(report()).await.unwrap();
        let second = addr.send(GetActiveAlerts).await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(first[0].id, second[0].id, "re-report keeps the same alert");

        addr.send(status("dx", 30.0, HealthStatus::Healthy))
            .await
            .unwrap();
        assert!(addr.send(GetActiveAlerts).await.unwrap().is_empty());
    }

    #[actix::test]
    async fn test_prediction_alert_threshold_and_ttl() {
        let mut config = test_config();
        config.prediction_ttl_minutes = 15;
        let addr = AlertManager::new(config).start();
        let prediction = |p: f64| PredictionResult {
            device_id: "dp".into(),
            edge_id: "edge-1".into(),
            prediction_type: nimon_core::PredictionType::Overheating,
            probability: p,
            eta_minutes: Some(10),
            confidence: 0.9,
            reason: None,
            model_version: None,
            timestamp: Utc::now(),
        };
        addr.send(prediction(0.5)).await.unwrap();
        assert!(addr.send(GetActiveAlerts).await.unwrap().is_empty());
        addr.send(prediction(0.95)).await.unwrap();
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].rule_id, "pred-overheating");
        assert_eq!(active[0].severity, Severity::Critical);
    }

    #[actix::test]
    async fn test_edge_offline_alert_fires_and_resolves_on_reconnect() {
        let mut config = test_config();
        config.edge_offline_after_secs = 1;
        let addr = AlertManager::new(config).start();
        addr.send(EdgeActivity {
            edge_id: "edge-x".into(),
            kind: EdgeActivityKind::Disconnected,
        })
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        addr.send(RunSweep).await.unwrap();
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].rule_id, RULE_EDGE_OFFLINE);
        assert_eq!(active[0].edge_id, "edge-x");

        addr.send(EdgeActivity {
            edge_id: "edge-x".into(),
            kind: EdgeActivityKind::Connected,
        })
        .await
        .unwrap();
        assert!(addr.send(GetActiveAlerts).await.unwrap().is_empty());
    }

    #[actix::test]
    async fn test_device_offline_fires_for_devices_of_dead_edges() {
        let mut config = test_config();
        config.rules = vec![AlertRule {
            condition: RuleCondition::DeviceOffline {
                max_minutes_since_poll: 0,
            },
            ..threshold_rule("offline", 0.0, 5)
        }];
        let mut manager = AlertManager::new(config);
        // Last report 10 minutes ago; no session exists for its edge at
        // all, the sweep still evaluates the last-known device state.
        manager.devices.insert(
            "dz".into(),
            KnownDevice {
                edge_id: "edge-1".into(),
                status: HealthStatus::Healthy,
                metrics: HashMap::new(),
                last_seen: Utc::now() - Duration::minutes(10),
                restored: false,
            },
        );
        let addr = manager.start();
        assert!(addr.send(GetActiveAlerts).await.unwrap().is_empty());
        addr.send(RunSweep).await.unwrap();
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].rule_id, "offline");
    }

    #[actix::test]
    async fn test_device_removed_resolves_alerts() {
        let mut config = test_config();
        config.rules = vec![threshold_rule("hot", 80.0, 0)];
        let addr = AlertManager::new(config).start();
        addr.send(status("dr", 90.0, HealthStatus::Healthy))
            .await
            .unwrap();
        assert_eq!(addr.send(GetActiveAlerts).await.unwrap().len(), 1);
        addr.send(DeviceRemoved {
            edge_id: "edge-1".into(),
            device_id: "dr".into(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
        assert!(addr.send(GetActiveAlerts).await.unwrap().is_empty());
    }

    #[actix::test]
    async fn test_other_edge_cannot_resolve_or_remove_alerts_of_same_device_id() {
        let mut config = test_config();
        config.rules = vec![threshold_rule("hot", 80.0, 0)];
        let addr = AlertManager::new(config).start();
        addr.send(status("shared", 90.0, HealthStatus::Healthy))
            .await
            .unwrap();
        addr.send(IngestDeviceAlert {
            alert: nimon_core::actor::messages::DeviceAlert {
                device_id: "shared".into(),
                edge_id: "edge-1".into(),
                severity: Severity::Warning,
                message: "fan stalled".into(),
                metric_name: Some("fan".into()),
                metric_value: None,
                timestamp: Utc::now(),
            },
        })
        .await
        .unwrap();
        assert_eq!(addr.send(GetActiveAlerts).await.unwrap().len(), 2);

        // Edge 2 claims the device is healthy, then removed: edge 1's
        // alerts stay
        let mut healthy = status("shared", 20.0, HealthStatus::Healthy);
        healthy.edge_id = "edge-2".into();
        addr.send(healthy).await.unwrap();
        addr.send(DeviceRemoved {
            edge_id: "edge-2".into(),
            device_id: "shared".into(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert_eq!(active.len(), 2, "{:?}", active);
        assert!(active.iter().all(|a| a.edge_id == "edge-1"));

        // The owning edge removes it: resolved
        addr.send(DeviceRemoved {
            edge_id: "edge-1".into(),
            device_id: "shared".into(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
        assert!(addr.send(GetActiveAlerts).await.unwrap().is_empty());
    }

    #[actix::test]
    async fn test_edge_offline_alert_resolves_when_same_connection_resumes() {
        let mut config = test_config();
        config.edge_offline_after_secs = 1;
        config.rules = vec![];
        config.disable_default_rules = true;
        let sessions = std::sync::Arc::new(SessionStore::new());
        let session = crate::session::EdgeSession::new("edge-z".into(), "Z".into(), None, None);
        sessions.add(session.clone());
        let addr = AlertManager::new(config)
            .with_sessions(sessions.clone())
            .start();

        // Connected but silent past the threshold (no devices, no reconnect)
        tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
        addr.send(RunSweep).await.unwrap();
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].rule_id, RULE_EDGE_OFFLINE);

        // Heartbeats resume on the same socket: the next sweep resolves it
        session.touch().await;
        addr.send(RunSweep).await.unwrap();
        assert!(addr.send(GetActiveAlerts).await.unwrap().is_empty());

        // Silent again: fires again; a heartbeat (Seen) resolves at once
        tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
        addr.send(RunSweep).await.unwrap();
        assert_eq!(addr.send(GetActiveAlerts).await.unwrap().len(), 1);
        addr.send(EdgeActivity {
            edge_id: "edge-z".into(),
            kind: EdgeActivityKind::Seen,
        })
        .await
        .unwrap();
        assert!(addr.send(GetActiveAlerts).await.unwrap().is_empty());
    }

    #[test]
    fn test_extract_metadata() {
        let mut msg = status("d", 1.0, HealthStatus::Healthy);
        msg.metrics
            .insert("product".into(), MetricValue::String("PXIe-6368".into()));
        msg.metrics.insert("slot".into(), MetricValue::Integer(3));
        msg.is_simulated = true;
        let meta = extract_metadata(&msg);
        assert_eq!(meta.model.as_deref(), Some("PXIe-6368"));
        assert_eq!(meta.slot, Some(3));
        assert_eq!(meta.is_simulated, Some(true));
    }
}
