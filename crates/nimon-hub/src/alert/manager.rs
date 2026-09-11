//! Alert manager actor for rule evaluation and alert lifecycle

use actix::prelude::*;
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use sqlx::SqlitePool;
use std::collections::HashMap;
use tracing::{debug, error, info, warn};

use nimon_core::actor::messages::{DeviceStatusUpdate, PredictionResult};
use nimon_core::alert::rules::{AlertRule, ComparisonOp, EvaluationContext, RuleCondition};
use nimon_core::alert::*;
use nimon_core::db::device_repo::DeviceRepository;
use nimon_core::db::{AlertRepository, PredictionRepository};
use nimon_core::{HealthStatus, MetricValue, Severity};

use crate::action::executor::{ActionContext, ExecuteAction};
use crate::alert::notifier::{AlertNotifier, SendNotification};
use crate::config::MaintenanceConfig;
use crate::session::SessionStore;

/// Prediction severity thresholds
const PREDICTION_CRITICAL_THRESHOLD: f64 = 0.9;

/// Interval between periodic evaluation sweeps (seconds)
const EVALUATE_INTERVAL_SECS: u64 = 30;

/// Minimum spacing between metric-history flushes per device (seconds)
const METRIC_FLUSH_INTERVAL_SECS: i64 = 60;

/// Session heartbeat staleness before the session is dropped (seconds)
const SESSION_STALENESS_SECS: i64 = 120;

/// Alert manager configuration
#[derive(Debug, Clone)]
pub struct AlertManagerConfig {
    /// Default cooldown for alerts (minutes)
    pub default_cooldown_minutes: i64,
    /// Max number of firing alerts before suppression
    pub max_firing_count: i32,
    /// Alert cleanup interval (hours)
    pub cleanup_interval_hours: i64,
    /// Probability at which predictions are persisted/alerted
    pub prediction_alert_threshold: f64,
    /// Notification channels for alert notifications
    pub notification_channels: Vec<crate::config::NotificationChannelConfig>,
    /// Alert rules (uses default_rules() if empty)
    pub rules: Vec<AlertRule>,
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
            notification_channels: Vec::new(),
            rules: Vec::new(),
            maintenance: MaintenanceConfig::default(),
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
    /// First time the rule's condition started breaching (duration windows)
    breach_started: DashMap<String, DateTime<Utc>>,
    /// Last known health status per (edge, device) for HealthStatusChange
    last_status: HashMap<(String, String), HealthStatus>,
    /// Latest predictions per device (rule evaluation context)
    latest_predictions: HashMap<String, Vec<nimon_core::alert::rules::PredictionInfo>>,
    /// Last metric-history flush per device
    last_metric_flush: HashMap<String, DateTime<Utc>>,
    notifier_addr: Option<actix::Addr<AlertNotifier>>,
    action_executor: Option<actix::Addr<crate::action::executor::ActionExecutor>>,
    /// SQLite connection pool for persisting alerts and predictions.
    /// When `Some`, alerts and predictions are written to the database.
    db_pool: Option<SqlitePool>,
    /// Connected edge sessions for periodic evaluation + auto-resolve
    sessions: Option<std::sync::Arc<SessionStore>>,
}

impl AlertManager {
    pub fn new(config: AlertManagerConfig) -> Self {
        let rules = if config.rules.is_empty() {
            Self::default_rules()
        } else {
            config.rules.clone()
        };
        Self {
            config,
            rules,
            active_alerts: DashMap::new(),
            breach_started: DashMap::new(),
            last_status: HashMap::new(),
            latest_predictions: HashMap::new(),
            last_metric_flush: HashMap::new(),
            notifier_addr: None,
            action_executor: None,
            db_pool: None,
            sessions: None,
        }
    }

    /// Set the database pool for persisting alerts and predictions.
    pub fn with_db_pool(mut self, pool: SqlitePool) -> Self {
        self.db_pool = Some(pool);
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

    /// Set the session store used by the periodic evaluator.
    pub fn with_sessions(mut self, sessions: std::sync::Arc<SessionStore>) -> Self {
        self.sessions = Some(sessions);
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
                severity: Severity::Critical,
                condition: RuleCondition::DeviceOffline {
                    max_minutes_since_poll: 5,
                },
                cooldown_minutes: 5,
                notification_channels: vec!["console".to_string()],
                suppress_repeat: true,
                max_firing_count: None,
                action: None,
            },
            // Warning: High temperature
            AlertRule {
                id: "rule-high-temp".to_string(),
                name: "High Temperature".to_string(),
                description: "Device temperature exceeds threshold".to_string(),
                enabled: true,
                severity: Severity::Warning,
                condition: RuleCondition::MetricThreshold {
                    metric_name: "temperature".to_string(),
                    operator: ComparisonOp::GreaterThan,
                    threshold: 70.0,
                    duration_minutes: None,
                },
                cooldown_minutes: 5,
                notification_channels: vec!["console".to_string()],
                suppress_repeat: false,
                max_firing_count: None,
                action: None,
            },
            // Critical: Critical temperature
            AlertRule {
                id: "rule-critical-temp".to_string(),
                name: "Critical Temperature".to_string(),
                description: "Device temperature critical".to_string(),
                enabled: true,
                severity: Severity::Critical,
                condition: RuleCondition::MetricThreshold {
                    metric_name: "temperature".to_string(),
                    operator: ComparisonOp::GreaterThan,
                    threshold: 85.0,
                    duration_minutes: None,
                },
                cooldown_minutes: 2,
                notification_channels: vec!["console".to_string()],
                suppress_repeat: false,
                max_firing_count: None,
                action: None,
            },
            // Critical: Device error state
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
                notification_channels: vec!["console".to_string()],
                suppress_repeat: true,
                max_firing_count: None,
                action: None,
            },
        ]
    }

    fn evaluate_rules(&self, ctx: &EvaluationContext) -> Vec<&AlertRule> {
        self.rules
            .iter()
            .filter(|rule| rule.evaluate(ctx))
            .collect()
    }

    fn create_alert(&self, rule: &AlertRule, ctx: &EvaluationContext) -> Alert {
        let (metric_name, metric_value, threshold) = match &rule.condition {
            RuleCondition::MetricThreshold {
                metric_name,
                threshold,
                ..
            } => (
                Some(metric_name.clone()),
                ctx.metrics.get(metric_name).and_then(|v| match v {
                    MetricValue::Float(f) => Some(*f),
                    MetricValue::Integer(i) => Some(*i as f64),
                    _ => None,
                }),
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

    /// Number of currently active (open) alerts for a rule
    fn firing_count_for_rule(&self, rule_id: &str) -> i32 {
        self.active_alerts
            .iter()
            .filter(|entry| entry.alert.rule_id == rule_id)
            .count() as i32
    }

    /// Common logic for processing rule evaluation: checks duration windows,
    /// cooldown, suppression; creates alerts, stores them in active_alerts,
    /// dispatches rule actions and notifications, persists to the database.
    fn process_evaluation(&mut self, ctx: &EvaluationContext) -> Vec<Alert> {
        let mut new_alerts = Vec::new();
        let matched: Vec<AlertRule> = self.evaluate_rules(ctx).into_iter().cloned().collect();

        // Clear breach trackers for rules that are no longer matching
        let matched_ids: Vec<String> = matched.iter().map(|r| r.id.clone()).collect();
        let device_keys: Vec<String> = self
            .breach_started
            .iter()
            .filter(|entry| entry.key().ends_with(&format!(":{}", ctx.device_id)))
            .map(|entry| entry.key().clone())
            .collect();
        for key in device_keys {
            let rule_id = key.split(':').next().unwrap_or_default().to_string();
            if !matched_ids.contains(&rule_id) {
                self.breach_started.remove(&key);
            }
        }

        for rule in matched {
            let key = format!("{}:{}", rule.id, ctx.device_id);

            // duration window: only fire after sustained breach
            if let RuleCondition::MetricThreshold {
                duration_minutes: Some(window),
                ..
            } = rule.condition
            {
                let started = match self.breach_started.get(&key) {
                    Some(t) => *t,
                    None => {
                        let now = Utc::now();
                        self.breach_started.insert(key.clone(), now);
                        debug!(
                            "Rule {} for device {} started breaching, waiting {}min window",
                            rule.id, ctx.device_id, window
                        );
                        continue;
                    }
                };
                let elapsed_min = Utc::now().signed_duration_since(started).num_minutes();
                if elapsed_min < window as i64 {
                    debug!(
                        "Rule {} for device {} breaching for {}min of {}min window",
                        rule.id, ctx.device_id, elapsed_min, window
                    );
                    continue;
                }
            }

            // Check cooldown
            if self.check_cooldown(&rule.id, &ctx.device_id) {
                debug!("Rule {} for device {} in cooldown", rule.id, ctx.device_id);
                continue;
            }

            // Check per-rule and global suppression caps
            let rule_cap = rule
                .max_firing_count
                .unwrap_or(self.config.max_firing_count);
            if self.firing_count_for_rule(&rule.id) >= rule_cap {
                warn!(
                    "Rule {} suppressed: {} active alerts already firing (cap {})",
                    rule.id,
                    self.firing_count_for_rule(&rule.id),
                    rule_cap
                );
                continue;
            }

            let mut alert = self.create_alert(&rule, ctx);

            // suppress_repeat: if this rule+device is already active, skip re-fire
            if rule.suppress_repeat && self.active_alerts.contains_key(&key) {
                debug!(
                    "Rule {} for device {} suppress_repeat: already active",
                    rule.id, ctx.device_id
                );
                continue;
            }

            // Update or insert alert tracking
            if let Some(mut active) = self.active_alerts.get_mut(&key) {
                active.last_fired = Utc::now();
                active.fired_count += 1;
                alert.fired_count = active.fired_count;
                alert.id = active.alert.id.clone(); // keep DB identity stable
                active.alert = alert.clone();
            } else {
                self.active_alerts.insert(
                    key.clone(),
                    ActiveAlert {
                        alert: alert.clone(),
                        last_fired: Utc::now(),
                        fired_count: 1,
                    },
                );
            }

            info!("Alert triggered: {} - {}", alert.id, alert.title);

            // Rule-driven self-healing action
            if let Some(action_ref) = &rule.action {
                if let Some(ref executor) = self.action_executor {
                    let action = crate::action::actions::action_from_ref(action_ref);
                    let action_name = action.name.to_string();
                    let action_ctx =
                        ActionContext::new(&alert.edge_id, &alert.device_id, &alert.id);
                    executor.do_send(ExecuteAction {
                        action,
                        context: action_ctx,
                    });
                    info!(
                        "Rule action '{}' dispatched for alert {} on device {}",
                        action_name, alert.id, alert.device_id
                    );
                }
            }

            new_alerts.push(alert);
        }

        new_alerts
    }

    /// Dispatch notifications for alerts and mark them sent.
    fn dispatch_notifications(&self, alerts: &[Alert], ctx: &mut <Self as Actor>::Context) {
        for alert in alerts {
            let channels = self
                .rules
                .iter()
                .find(|r| r.id == alert.rule_id)
                .map(|r| r.notification_channels.clone())
                .unwrap_or_default();

            if let Some(ref addr) = self.notifier_addr {
                addr.do_send(SendNotification {
                    alert: alert.clone(),
                    channels,
                });
            }

            // Mark notification sent in the database
            if let Some(pool) = &self.db_pool {
                let pool = pool.clone();
                let alert_id = alert.id.clone();
                ctx.spawn(actix::fut::wrap_future(async move {
                    let repo = AlertRepository::new(&pool);
                    if let Err(e) = repo.mark_notified(&alert_id).await {
                        warn!("Failed to mark alert notified: {}", e);
                    }
                }));
            }
        }
    }

    /// Persist alerts to the database (fire-and-forget).
    fn persist_alerts(&self, alerts: &[Alert], ctx: &mut <Self as Actor>::Context) {
        if alerts.is_empty() {
            return;
        }
        if let Some(pool) = &self.db_pool {
            let pool = pool.clone();
            let alerts_to_persist: Vec<Alert> = alerts.to_vec();
            let fut = async move {
                let repo = AlertRepository::new(&pool);
                for alert in alerts_to_persist {
                    if let Err(e) = repo.insert(&alert).await {
                        warn!("Failed to persist alert: {}", e);
                    }
                }
            };
            ctx.spawn(actix::fut::wrap_future(fut));
        }
    }

    /// Persist device status and (throttled) metric history.
    fn persist_device_state(
        &mut self,
        msg: &DeviceStatusUpdate,
        ctx: &mut <Self as Actor>::Context,
    ) {
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        let edge_id = msg.edge_id.clone();
        let device_status = nimon_core::DeviceStatus {
            device_id: msg.device_id.clone(),
            status: msg.status,
            last_poll: msg.timestamp,
            metrics: msg.metrics.clone(),
            error_message: None,
            error_count: 0,
            uptime_seconds: 0,
        };

        // Throttled metric history: numeric metrics, at most once per interval
        let now = Utc::now();
        let should_flush = self
            .last_metric_flush
            .get(&msg.device_id)
            .map(|last| {
                now.signed_duration_since(*last).num_seconds() >= METRIC_FLUSH_INTERVAL_SECS
            })
            .unwrap_or(true);
        let metric_points: Vec<nimon_core::MetricPoint> = if should_flush {
            self.last_metric_flush.insert(msg.device_id.clone(), now);
            msg.metrics
                .iter()
                .filter_map(|(name, value)| match value {
                    MetricValue::Float(f) => Some((*f, name.clone())),
                    MetricValue::Integer(i) => Some((*i as f64, name.clone())),
                    _ => None,
                })
                .map(|(value, metric_name)| nimon_core::MetricPoint {
                    device_id: msg.device_id.clone(),
                    timestamp: msg.timestamp,
                    metric_name,
                    metric_value: value,
                })
                .collect()
        } else {
            Vec::new()
        };

        ctx.spawn(actix::fut::wrap_future(async move {
            let repo = DeviceRepository::new(&pool);
            // Ensure the device row exists so alert/status FKs resolve
            // (first sighting arrives via status updates, not discovery)
            if let Err(e) = repo
                .upsert_snapshot(&device_status.device_id, &edge_id)
                .await
            {
                warn!("Failed to register device snapshot: {}", e);
            }
            if let Err(e) = repo.upsert_status(&device_status).await {
                error!("Failed to persist device status: {}", e);
            }
            for metric in metric_points {
                if let Err(e) = repo.insert_metric(&metric).await {
                    warn!("Failed to persist metric history: {}", e);
                }
            }
        }));
    }

    /// Resolve an active alert: update memory + database, notify.
    fn resolve_active_alert(&self, alert: &Alert, ctx: &mut <Self as Actor>::Context) {
        info!("Alert resolved: {} ({})", alert.id, alert.title);
        if let Some(pool) = &self.db_pool {
            let pool = pool.clone();
            let alert_id = alert.id.clone();
            ctx.spawn(actix::fut::wrap_future(async move {
                let repo = AlertRepository::new(&pool);
                if let Err(e) = repo.resolve(&alert_id).await {
                    warn!("Failed to persist alert resolution: {}", e);
                }
            }));
        }
    }

    /// Periodic sweep: evaluate all session snapshots for staleness/offline,
    /// auto-resolve alerts whose conditions cleared, and expire predictions.
    /// Contexts are gathered asynchronously and processed via
    /// [`ProcessSweepContexts`] back on the actor loop.
    fn run_periodic_evaluation(&self, ctx: &mut <Self as Actor>::Context) {
        let Some(sessions) = self.sessions.clone() else {
            return;
        };
        let pool = self.db_pool.clone();
        let addr = ctx.address();
        let last_status = self.last_status.clone();
        let latest_predictions = self.latest_predictions.clone();

        ctx.spawn(actix::fut::wrap_future(async move {
            // Drop stale sessions and mark them offline in the DB
            let stale = sessions.remove_stale(SESSION_STALENESS_SECS).await;
            if !stale.is_empty() {
                if let Some(pool) = &pool {
                    let repo = nimon_core::db::edge_repo::EdgeRepository::new(pool);
                    for edge_id in &stale {
                        if let Err(e) = repo
                            .update_status(edge_id, nimon_core::EdgeStatus::Offline)
                            .await
                        {
                            warn!("Failed to mark edge {} offline: {}", edge_id, e);
                        }
                    }
                }
            }

            // Build evaluation contexts from live session snapshots
            let mut contexts = Vec::new();
            for entry in sessions.iter() {
                let (_edge_id, session) = entry.pair();
                let edge_id = session.edge_id().to_string();
                for device in session.devices().await {
                    let current = HealthStatus::parse(&device.status);
                    let previous = last_status
                        .get(&(edge_id.clone(), device.device_id.clone()))
                        .copied();
                    let predictions = latest_predictions
                        .get(&device.device_id)
                        .cloned()
                        .unwrap_or_default();
                    contexts.push(EvaluationContext {
                        device_id: device.device_id.clone(),
                        edge_id: edge_id.clone(),
                        current_status: current,
                        previous_status: previous,
                        metrics: device.metrics.clone(),
                        last_poll: device.last_seen,
                        predictions,
                    });
                }
            }

            addr.do_send(ProcessSweepContexts(contexts));
        }));
    }

    /// Retention cleanup: prune resolved alerts, old metrics, old predictions.
    fn run_cleanup(&self, ctx: &mut <Self as Actor>::Context) {
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        let maintenance = self.config.maintenance.clone();
        ctx.spawn(actix::fut::wrap_future(async move {
            let alerts = AlertRepository::new(&pool);
            let devices = DeviceRepository::new(&pool);
            let predictions = PredictionRepository::new(&pool);
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
        }));
    }

    /// Restore firing alerts from the database after a hub restart.
    fn restore_firing_alerts(&mut self, ctx: &mut <Self as Actor>::Context) {
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        ctx.spawn(actix::fut::wrap_future(async move {
            let repo = AlertRepository::new(&pool);
            match repo.list_active().await {
                Ok(records) => {
                    let count = records.len();
                    for record in records {
                        info!(
                            "Restored firing alert {} (rule {})",
                            record.id, record.rule_name
                        );
                    }
                    if count > 0 {
                        info!("Restored {} firing alerts from database", count);
                    }
                }
                Err(e) => warn!("Failed to restore firing alerts: {}", e),
            }
        }));
    }
}

impl Actor for AlertManager {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("AlertManager actor started");

        // Start the AlertNotifier. The console channel is implied when no
        // channels are configured so default deployments still see alerts.
        let mut channels: Vec<NotificationChannel> = self
            .config
            .notification_channels
            .iter()
            .map(|c| {
                let skip_tls_verify = c.smtp_skip_tls_verify.unwrap_or(false);
                let smtp_password = c
                    .smtp_password
                    .clone()
                    .or_else(|| std::env::var("NIMON_SMTP_PASSWORD").ok());
                let smtp_username = c
                    .smtp_username
                    .clone()
                    .or_else(|| std::env::var("NIMON_SMTP_USER").ok());
                let channel_config: serde_json::Value = match c.channel_type.as_str() {
                    "email" => serde_json::json!({
                        "smtp_skip_tls_verify": skip_tls_verify,
                        "smtp_password": smtp_password,
                        "smtp_username_env": smtp_username,
                    }),
                    "webhook" => serde_json::json!({
                        "headers": c.headers.as_ref().map(|pairs| {
                            pairs
                                .iter()
                                .filter_map(|p| {
                                    if p.len() == 2 {
                                        Some((p[0].clone(), p[1].clone()))
                                    } else {
                                        None
                                    }
                                })
                                .collect::<std::collections::HashMap<String, String>>()
                        }),
                    }),
                    _ => serde_json::json!({}),
                };
                NotificationChannel {
                    id: c.name.clone().unwrap_or_else(|| c.channel_type.clone()),
                    name: c.name.clone().unwrap_or_else(|| c.channel_type.clone()),
                    enabled: c.enabled,
                    channel_type: match c.channel_type.as_str() {
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
                            headers: c.headers.as_ref().and_then(|pairs| {
                                let map: std::collections::HashMap<String, String> = pairs
                                    .iter()
                                    .filter_map(|p| {
                                        if p.len() == 2 {
                                            Some((p[0].clone(), p[1].clone()))
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                                if map.is_empty() {
                                    None
                                } else {
                                    Some(map)
                                }
                            }),
                        },
                        "console" => ChannelType::Console,
                        other => {
                            debug!(
                                "Unsupported notification channel type: {}, using Console",
                                other
                            );
                            ChannelType::Console
                        }
                    },
                    config: channel_config,
                }
            })
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

        let notifier = AlertNotifier::new(channels).start();
        self.notifier_addr = Some(notifier);
        info!("AlertNotifier started");

        // Restore firing alerts from a previous run
        self.restore_firing_alerts(ctx);

        // Periodic evaluator: staleness/offline detection + auto-resolve
        ctx.run_interval(
            std::time::Duration::from_secs(EVALUATE_INTERVAL_SECS),
            |actor, ctx| actor.run_periodic_evaluation(ctx),
        );

        // Retention cleanup on a slower cadence
        ctx.run_interval(
            std::time::Duration::from_secs(
                (self.config.cleanup_interval_hours.max(1) as u64) * 3600,
            ),
            |actor, ctx| actor.run_cleanup(ctx),
        );
    }
}

/// Snapshot contexts gathered by the periodic sweep, processed back on
/// the actor loop for rule evaluation and auto-resolve.
#[derive(Message)]
#[rtype(result = "()")]
struct ProcessSweepContexts(Vec<EvaluationContext>);

impl Handler<ProcessSweepContexts> for AlertManager {
    type Result = ();

    fn handle(&mut self, msg: ProcessSweepContexts, ctx: &mut Self::Context) -> Self::Result {
        for eval_ctx in msg.0 {
            // Evaluate rules (catches DeviceOffline via stale last_poll)
            let alerts = self.process_evaluation(&eval_ctx);
            self.persist_alerts(&alerts, ctx);
            self.dispatch_notifications(&alerts, ctx);

            // Auto-resolve: conditions that no longer hold for active alerts
            let device_key_prefix = format!(":{}", eval_ctx.device_id);
            let keys_to_check: Vec<String> = self
                .active_alerts
                .iter()
                .filter(|entry| entry.key().ends_with(&device_key_prefix))
                .map(|entry| entry.key().clone())
                .collect();

            for key in keys_to_check {
                let Some(active) = self.active_alerts.get(&key) else {
                    continue;
                };
                let rule_id = active.alert.rule_id.clone();
                let Some(rule) = self.rules.iter().find(|r| r.id == rule_id) else {
                    continue;
                };
                if !rule.condition.evaluate(&eval_ctx) {
                    let resolved = Alert {
                        status: AlertStatus::Resolved,
                        resolved_at: Some(Utc::now()),
                        ..active.alert.clone()
                    };
                    drop(active);
                    self.resolve_active_alert(&resolved, ctx);
                    self.active_alerts.remove(&key);
                }
            }
        }

        // Expire stale active predictions
        if let Some(pool) = self.db_pool.clone() {
            let expiry = self.config.maintenance.prediction_expiry_minutes;
            ctx.spawn(actix::fut::wrap_future(async move {
                let repo = PredictionRepository::new(&pool);
                if let Err(e) = repo.expire_stale(expiry).await {
                    warn!("Failed to expire stale predictions: {}", e);
                }
            }));
        }
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
        self.dispatch_notifications(&alerts, ctx);
        alerts
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

    fn handle(&mut self, msg: IngestDeviceAlert, ctx: &mut Self::Context) -> Self::Result {
        let device_alert = msg.alert;
        let alert = Alert {
            id: ulid::Ulid::new().to_string(),
            rule_id: format!(
                "edge:{}",
                device_alert
                    .metric_name
                    .as_deref()
                    .unwrap_or("device-alert")
            ),
            edge_id: device_alert.edge_id.clone(),
            device_id: device_alert.device_id.clone(),
            severity: device_alert.severity,
            status: AlertStatus::Firing,
            title: format!("Edge alert: {}", device_alert.message),
            message: device_alert.message.clone(),
            metric_name: device_alert.metric_name.clone(),
            metric_value: device_alert.metric_value,
            threshold: None,
            triggered_at: device_alert.timestamp,
            resolved_at: None,
            fired_count: 1,
            notification_sent: false,
        };

        let key = format!("{}:{}", alert.rule_id, alert.device_id);
        if self.check_cooldown(&alert.rule_id, &alert.device_id) {
            debug!("Edge alert in cooldown: {}", key);
            return;
        }

        info!("Edge alert ingested: {} - {}", alert.id, alert.title);
        self.active_alerts.insert(
            key,
            ActiveAlert {
                alert: alert.clone(),
                last_fired: Utc::now(),
                fired_count: 1,
            },
        );

        self.persist_alerts(&[alert.clone()], ctx);
        self.dispatch_notifications(&[alert], ctx);
    }
}

/// Message to get active predictions from the database
#[derive(Message)]
#[rtype(result = "Result<Vec<serde_json::Value>, String>")]
pub struct GetRecentPredictions {
    pub limit: usize,
}

/// Message to get alert history from the database
#[derive(Message)]
#[rtype(result = "Result<Vec<serde_json::Value>, String>")]
pub struct GetAlertHistory {
    pub limit: usize,
}

/// Message to get active alerts
#[derive(Message)]
#[rtype(result = "Vec<Alert>")]
pub struct GetActiveAlerts;

impl Handler<GetActiveAlerts> for AlertManager {
    type Result = Vec<Alert>;

    fn handle(&mut self, _msg: GetActiveAlerts, _ctx: &mut Self::Context) -> Self::Result {
        self.active_alerts
            .iter()
            .map(|entry| entry.value().alert.clone())
            .collect()
    }
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
            // Convert to JSON, limited to requested count
            let json: Vec<serde_json::Value> = predictions
                .into_iter()
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

impl Handler<GetAlertHistory> for AlertManager {
    type Result = ResponseActFuture<Self, Result<Vec<serde_json::Value>, String>>;

    fn handle(&mut self, msg: GetAlertHistory, _ctx: &mut Self::Context) -> Self::Result {
        let pool = self.db_pool.clone();
        let limit = msg.limit;
        let fut = async move {
            let pool = pool.ok_or("No database pool")?;
            let repo = AlertRepository::new(&pool);
            let alerts = repo
                .list_recent(limit as i64)
                .await
                .map_err(|e| e.to_string())?;
            let json: Vec<serde_json::Value> = alerts
                .into_iter()
                .map(|a| {
                    serde_json::json!({
                        "id": a.id,
                        "device_id": a.device_id,
                        "edge_id": a.edge_id,
                        "rule_name": a.rule_name,
                        "severity": a.severity,
                        "message": a.message,
                        "status": a.status,
                        "created_at": a.created_at,
                        "resolved_at": a.resolved_at,
                    })
                })
                .collect();
            Ok(json)
        }
        .into_actor(self);
        Box::pin(fut)
    }
}

/// Message to resolve (acknowledge) an alert
#[derive(Message)]
#[rtype(result = "Result<(), nimon_core::NimonError>")]
pub struct ResolveAlert {
    pub alert_id: String,
}

impl Handler<ResolveAlert> for AlertManager {
    type Result = ResponseActFuture<Self, Result<(), nimon_core::NimonError>>;

    fn handle(&mut self, msg: ResolveAlert, _ctx: &mut Self::Context) -> Self::Result {
        let pool = self.db_pool.clone();
        let alert_id = msg.alert_id;

        // Remove from in-memory active set if present
        let memory_key = self
            .active_alerts
            .iter()
            .find(|entry| entry.alert.id == alert_id)
            .map(|entry| entry.key().clone());
        if let Some(key) = &memory_key {
            info!("Alert acknowledged: {}", alert_id);
            self.active_alerts.remove(key);
        }
        let found_in_memory = memory_key.is_some();

        let fut = async move {
            if let Some(pool) = pool {
                let repo = AlertRepository::new(&pool);
                let affected = repo.resolve(&alert_id).await?;
                if found_in_memory || affected > 0 {
                    return Ok(());
                }
                // Neither tracked in memory nor present in the database
                Err(nimon_core::NimonError::AlertNotFound(alert_id))
            } else if found_in_memory {
                Ok(())
            } else {
                // Not tracked in memory: without a DB we cannot confirm it exists
                Err(nimon_core::NimonError::AlertNotFound(alert_id))
            }
        }
        .into_actor(self);
        Box::pin(fut)
    }
}

/// Handle device status updates
impl Handler<DeviceStatusUpdate> for AlertManager {
    type Result = ();

    fn handle(&mut self, msg: DeviceStatusUpdate, ctx: &mut Self::Context) -> Self::Result {
        let status_key = (msg.edge_id.clone(), msg.device_id.clone());
        let previous_status = self.last_status.get(&status_key).copied();
        self.last_status.insert(status_key, msg.status);

        let eval_ctx = EvaluationContext {
            device_id: msg.device_id.clone(),
            edge_id: msg.edge_id.clone(),
            current_status: msg.status,
            previous_status,
            metrics: msg.metrics.clone(),
            last_poll: msg.timestamp,
            predictions: self
                .latest_predictions
                .get(&msg.device_id)
                .cloned()
                .unwrap_or_default(),
        };

        let alerts = self.process_evaluation(&eval_ctx);

        // Persist device status + throttled metric history
        self.persist_device_state(&msg, ctx);

        self.persist_alerts(&alerts, ctx);
        self.dispatch_notifications(&alerts, ctx);
    }
}

/// Handle prediction results
impl Handler<PredictionResult> for AlertManager {
    type Result = ();

    fn handle(&mut self, msg: PredictionResult, ctx: &mut Self::Context) -> Self::Result {
        // Track latest predictions per device for rule evaluation
        self.latest_predictions
            .entry(msg.device_id.clone())
            .or_default()
            .push(nimon_core::alert::rules::PredictionInfo {
                prediction_type: msg.prediction_type.to_string(),
                probability: msg.probability,
                eta_minutes: msg.eta_minutes,
            });

        if msg.probability < self.config.prediction_alert_threshold {
            return;
        }

        let rule_id = format!("pred-{}", msg.prediction_type);

        // Check cooldown
        if self.check_cooldown(&rule_id, &msg.device_id) {
            debug!(
                "Prediction {} for device {} in cooldown",
                rule_id, msg.device_id
            );
            return;
        }

        let mut alert = Alert {
            id: ulid::Ulid::new().to_string(),
            rule_id: rule_id.clone(),
            edge_id: msg.edge_id.clone(),
            device_id: msg.device_id.clone(),
            severity: if msg.probability >= PREDICTION_CRITICAL_THRESHOLD {
                Severity::Critical
            } else {
                Severity::Warning
            },
            status: AlertStatus::Firing,
            title: format!("{} prediction for {}", msg.prediction_type, msg.device_id),
            message: msg.reason.clone().unwrap_or_else(|| {
                format!(
                    "Predicted {} with {:.0}% confidence",
                    msg.prediction_type,
                    msg.probability * 100.0
                )
            }),
            metric_name: None,
            metric_value: Some(msg.probability),
            threshold: Some(self.config.prediction_alert_threshold),
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
            alert.fired_count = active.fired_count;
            alert.id = active.alert.id.clone();
            active.alert = alert.clone();
        } else {
            self.active_alerts.insert(
                key,
                ActiveAlert {
                    alert: alert.clone(),
                    last_fired: Utc::now(),
                    fired_count: 1,
                },
            );
        }

        // Persist the prediction to the database
        if let Some(pool) = &self.db_pool {
            let pool = pool.clone();
            let device_id = msg.device_id.clone();
            let edge_id = msg.edge_id.clone();
            let prediction_type = msg.prediction_type.to_string();
            let probability = msg.probability;
            let eta_minutes = msg.eta_minutes.map(|m| m as i64);
            let model_version = msg.model_version.clone();
            let fut = async move {
                let repo = PredictionRepository::new(&pool);
                if let Err(e) = repo
                    .insert(
                        &device_id,
                        &edge_id,
                        &prediction_type,
                        probability,
                        eta_minutes,
                        model_version.as_deref(),
                    )
                    .await
                {
                    warn!("Failed to persist prediction: {}", e);
                }
            };
            ctx.spawn(actix::fut::wrap_future(fut));
        }

        // Also persist the alert generated from this prediction
        let persisted_alerts = std::slice::from_ref(&alert);
        self.persist_alerts(persisted_alerts, ctx);
        self.dispatch_notifications(persisted_alerts, ctx);
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

    #[test]
    fn test_config_default() {
        let config = AlertManagerConfig::default();
        assert_eq!(config.default_cooldown_minutes, 5);
        assert_eq!(config.max_firing_count, 100);
        assert_eq!(config.prediction_alert_threshold, 0.8);
    }

    #[actix::test]
    async fn test_status_update_fires_temperature_alert() {
        let manager = AlertManager::new(test_config());
        let addr = manager.start();

        // Healthy temperature: no alerts
        addr.send(DeviceStatusUpdate {
            device_id: "dev-1".to_string(),
            edge_id: "edge-1".to_string(),
            status: HealthStatus::Healthy,
            metrics: [("temperature".to_string(), MetricValue::Float(50.0))].into(),
            timestamp: Utc::now(),
            is_simulated: false,
        })
        .await
        .unwrap();

        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert!(active.is_empty());

        // High temperature (>70): warning alert
        addr.send(DeviceStatusUpdate {
            device_id: "dev-1".to_string(),
            edge_id: "edge-1".to_string(),
            status: HealthStatus::Healthy,
            metrics: [("temperature".to_string(), MetricValue::Float(75.0))].into(),
            timestamp: Utc::now(),
            is_simulated: false,
        })
        .await
        .unwrap();

        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].rule_id, "rule-high-temp");
    }

    #[actix::test]
    async fn test_previous_status_enables_status_change_rule() {
        let manager = AlertManager::new(test_config());
        let addr = manager.start();

        // Healthy first: establishes previous_status
        addr.send(DeviceStatusUpdate {
            device_id: "dev-2".to_string(),
            edge_id: "edge-1".to_string(),
            status: HealthStatus::Healthy,
            metrics: Default::default(),
            timestamp: Utc::now(),
            is_simulated: false,
        })
        .await
        .unwrap();

        // Then error: HealthStatusChange{from: Healthy, to: Error} fires
        addr.send(DeviceStatusUpdate {
            device_id: "dev-2".to_string(),
            edge_id: "edge-1".to_string(),
            status: HealthStatus::Error,
            metrics: Default::default(),
            timestamp: Utc::now(),
            is_simulated: false,
        })
        .await
        .unwrap();

        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert!(active.iter().any(|a| a.rule_id == "rule-device-error"));
    }

    #[actix::test]
    async fn test_acknowledge_removes_active_alert() {
        let mut config = test_config();
        // Keep exactly one rule firing for this device so ack is unambiguous
        config.rules = vec![AlertRule {
            id: "rule-critical-only".to_string(),
            name: "Critical Temp".to_string(),
            description: String::new(),
            enabled: true,
            severity: Severity::Critical,
            condition: RuleCondition::MetricThreshold {
                metric_name: "temperature".to_string(),
                operator: ComparisonOp::GreaterThan,
                threshold: 85.0,
                duration_minutes: None,
            },
            cooldown_minutes: 0,
            notification_channels: vec![],
            suppress_repeat: false,
            max_firing_count: None,
            action: None,
        }];
        let manager = AlertManager::new(config);
        let addr = manager.start();

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

        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert!(active.is_empty());
    }

    #[actix::test]
    async fn test_duration_window_delays_firing() {
        let mut config = test_config();
        config.rules = vec![AlertRule {
            id: "rule-sustained".to_string(),
            name: "Sustained High Temp".to_string(),
            description: String::new(),
            enabled: true,
            severity: Severity::Warning,
            condition: RuleCondition::MetricThreshold {
                metric_name: "temperature".to_string(),
                operator: ComparisonOp::GreaterThan,
                threshold: 70.0,
                duration_minutes: Some(10),
            },
            cooldown_minutes: 0,
            notification_channels: vec![],
            suppress_repeat: false,
            max_firing_count: None,
            action: None,
        }];
        let manager = AlertManager::new(config);
        let addr = manager.start();

        // First breach: window starts, no alert yet
        addr.send(EvaluateRules {
            context: eval_ctx("dev-4", 80.0, HealthStatus::Healthy),
        })
        .await
        .unwrap();
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert!(active.is_empty(), "duration window should delay firing");

        // Immediately re-evaluating still inside the window
        addr.send(EvaluateRules {
            context: eval_ctx("dev-4", 80.0, HealthStatus::Healthy),
        })
        .await
        .unwrap();
        let active = addr.send(GetActiveAlerts).await.unwrap();
        assert!(active.is_empty(), "still inside duration window");
    }

    #[actix::test]
    async fn test_max_firing_count_suppression() {
        let mut config = test_config();
        config.max_firing_count = 2;
        config.rules = vec![AlertRule {
            id: "rule-hot".to_string(),
            name: "Hot".to_string(),
            description: String::new(),
            enabled: true,
            severity: Severity::Warning,
            condition: RuleCondition::MetricThreshold {
                metric_name: "temperature".to_string(),
                operator: ComparisonOp::GreaterThan,
                threshold: 70.0,
                duration_minutes: None,
            },
            cooldown_minutes: 0,
            notification_channels: vec![],
            suppress_repeat: false,
            max_firing_count: None,
            action: None,
        }];
        let manager = AlertManager::new(config);
        let addr = manager.start();

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
}
