//! Hub server configuration

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use nimon_core::alert::rules::{AlertRule, ComparisonOp, RuleCondition};
use nimon_core::alert::Severity;
use nimon_core::types::HealthStatus;

use crate::action::{Action, ActionType, RuleActionSpec};

/// Default HTTP/WebSocket port of the hub.
pub const DEFAULT_PORT: u16 = 9090;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_db_path")]
    pub database_path: String,
    #[serde(default)]
    pub alert: AlertConfig,
    /// Desired-state configuration pushed to edges on register
    #[serde(default)]
    pub edges: EdgesConfig,
    /// Retention / maintenance settings
    #[serde(default)]
    pub maintenance: MaintenanceConfig,
    /// API / edge authentication
    #[serde(default)]
    pub auth: AuthConfig,
    /// Origins allowed to call the API cross-origin. Empty (default) means
    /// no CORS layer at all (the embedded dashboard is same-origin).
    /// `"*"` allows any origin.
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    DEFAULT_PORT
}
fn default_db_path() -> String {
    "data/nimon.db".to_string()
}

/// Authentication settings. Environment variables override the file:
/// `NIMON_API_TOKEN`, `NIMON_EDGE_TOKEN`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthConfig {
    /// When set, every non-GET `/api/*` request needs `Authorization: Bearer <token>`
    #[serde(default)]
    pub api_token: Option<String>,
    /// When set, `/ws` needs the token (`Authorization: Bearer` or `?token=`)
    #[serde(default)]
    pub edge_token: Option<String>,
}

impl AuthConfig {
    /// Apply the environment overrides (`NIMON_API_TOKEN`, `NIMON_EDGE_TOKEN`).
    /// Empty values (from file or env) mean "not set".
    pub fn resolved_with(&self, env: &dyn Fn(&str) -> Option<String>) -> AuthConfig {
        let pick = |env_key: &str, file: &Option<String>| {
            env(env_key)
                .filter(|v| !v.trim().is_empty())
                .or_else(|| file.clone().filter(|v| !v.trim().is_empty()))
        };
        AuthConfig {
            api_token: pick("NIMON_API_TOKEN", &self.api_token),
            edge_token: pick("NIMON_EDGE_TOKEN", &self.edge_token),
        }
    }

    /// [`Self::resolved_with`] against the process environment.
    pub fn resolved(&self) -> AuthConfig {
        self.resolved_with(&|k| std::env::var(k).ok())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    #[serde(default = "default_cooldown")]
    pub default_cooldown_minutes: i64,
    #[serde(default = "default_max_firing")]
    pub max_firing_count: i32,
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_hours: i64,
    /// Probability at which a prediction raises an alert (0.0-1.0).
    /// Every prediction is persisted regardless.
    #[serde(default = "default_prediction_threshold")]
    pub prediction_alert_threshold: f64,
    /// Prediction alerts auto-resolve when no fresh matching prediction
    /// arrives within this many minutes; also bounds the in-memory
    /// prediction cache used for rule evaluation.
    #[serde(default = "default_prediction_ttl")]
    pub prediction_ttl_minutes: i64,
    /// Edge-reported alerts auto-resolve when the device reports healthy
    /// again or after this many minutes without a re-report.
    #[serde(default = "default_edge_alert_ttl")]
    pub edge_alert_ttl_minutes: i64,
    /// Built-in "edge offline" alert fires when an edge has not been seen
    /// for longer than this (seconds). 0 disables it.
    #[serde(default = "default_edge_offline_after")]
    pub edge_offline_after_secs: u64,
    /// Periodic evaluation sweep interval (seconds)
    #[serde(default = "default_evaluate_interval")]
    pub evaluate_interval_secs: u64,
    #[serde(default)]
    pub notification_channels: Vec<NotificationChannelConfig>,
    #[serde(default)]
    pub rules: Vec<RuleConfig>,
}

fn default_cooldown() -> i64 {
    5
}
fn default_max_firing() -> i32 {
    100
}
fn default_cleanup_interval() -> i64 {
    24
}
fn default_prediction_threshold() -> f64 {
    0.8
}
fn default_prediction_ttl() -> i64 {
    15
}
fn default_edge_alert_ttl() -> i64 {
    30
}
fn default_edge_offline_after() -> u64 {
    90
}
fn default_evaluate_interval() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannelConfig {
    pub channel_type: String, // "email", "slack", "teams", "webhook", "console"
    pub name: Option<String>,
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub headers: Option<Vec<Vec<String>>>, // list of [name, value] pairs (YAML-friendly)
    pub smtp_server: Option<String>,
    #[serde(default)]
    pub smtp_port: Option<u16>,
    /// SMTP username; env `NIMON_SMTP_USER` takes precedence when set
    #[serde(default)]
    pub smtp_username: Option<String>,
    /// SMTP password; env `NIMON_SMTP_PASSWORD`, then the file named by
    /// `NIMON_SMTP_PASSWORD_FILE`, then `smtp_password_file` take precedence
    #[serde(default)]
    pub smtp_password: Option<String>,
    /// File holding the SMTP password (first line); used when no env override is set
    #[serde(default)]
    pub smtp_password_file: Option<String>,
    /// TLS mode: `implicit` (a.k.a. `wrapper`/`smtps`), `starttls`,
    /// `opportunistic` or `none`. Default by port: 465 implicit, 25
    /// opportunistic, anything else STARTTLS (required).
    #[serde(default)]
    pub smtp_tls: Option<String>,
    /// Allow sending credentials over an unencrypted connection (default false)
    #[serde(default)]
    pub smtp_allow_plaintext_auth: bool,
    pub from_addr: Option<String>,
    pub to_addrs: Option<Vec<String>>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub smtp_skip_tls_verify: Option<bool>,
}

fn default_enabled() -> bool {
    true
}

impl NotificationChannelConfig {
    /// SMTP credentials with precedence: `NIMON_SMTP_USER` over
    /// `smtp_username`; `NIMON_SMTP_PASSWORD` over the file named by
    /// `NIMON_SMTP_PASSWORD_FILE` over `smtp_password_file` over
    /// `smtp_password`.
    pub fn smtp_credentials_with(
        &self,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> (Option<String>, Option<String>) {
        let non_empty = |v: Option<String>| v.filter(|s| !s.is_empty());
        let username = non_empty(env("NIMON_SMTP_USER")).or_else(|| self.smtp_username.clone());
        let password = non_empty(env("NIMON_SMTP_PASSWORD"))
            .or_else(|| non_empty(env("NIMON_SMTP_PASSWORD_FILE")).and_then(read_secret_file))
            .or_else(|| self.smtp_password_file.clone().and_then(read_secret_file))
            .or_else(|| self.smtp_password.clone());
        (username, password)
    }

    /// [`Self::smtp_credentials_with`] against the process environment.
    pub fn smtp_credentials(&self) -> (Option<String>, Option<String>) {
        self.smtp_credentials_with(&|k| std::env::var(k).ok())
    }
}

/// Read a secret from a file (trailing newline stripped). Logs and
/// returns `None` when unreadable.
fn read_secret_file(path: String) -> Option<String> {
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let secret = content.trim_end_matches(['\r', '\n']).to_string();
            (!secret.is_empty()).then_some(secret)
        }
        Err(e) => {
            tracing::error!("Failed to read SMTP password file {}: {}", path, e);
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleConfig {
    pub name: String,
    pub condition: ConditionConfig,
    #[serde(default)]
    pub action: Option<ActionConfig>,
    /// Fallback action executed when `action` still fails after its retries
    #[serde(default)]
    pub on_failure: Option<ActionConfig>,
    #[serde(default = "default_severity")]
    pub severity: String,
    #[serde(default = "default_rule_description")]
    pub description: String,
    #[serde(default = "default_rule_enabled")]
    pub enabled: bool,
    /// Minutes between re-fires of the same rule+device; falls back to
    /// `alert.default_cooldown_minutes` when omitted
    #[serde(default)]
    pub cooldown_minutes: Option<i32>,
    /// Cap on simultaneously active alerts for this rule; falls back to
    /// `alert.max_firing_count` when omitted
    #[serde(default)]
    pub max_firing_count: Option<i32>,
    /// Channel names/ids; empty = every enabled channel
    #[serde(default)]
    pub notification_channels: Vec<String>,
    #[serde(default)]
    pub suppress_repeat: bool,
}

fn default_severity() -> String {
    "warning".to_string()
}
fn default_rule_description() -> String {
    String::new()
}
fn default_rule_enabled() -> bool {
    true
}

/// Rule condition configuration.
///
/// Accepts both the documented nested shape:
///
/// ```yaml
/// condition:
///   MetricThreshold:
///     metric: temperature
///     threshold: 75.0
///     comparison: greater_than
///     hysteresis: 2.0     # optional clear margin
/// ```
///
/// and the legacy flat shape:
///
/// ```yaml
/// condition:
///   metric: temperature
///   threshold: 75.0
///   comparison: greater_than
/// ```
///
/// Variants: `MetricThreshold`, `DeviceOffline`, `HealthStatusChange`,
/// `Prediction`, `Composite`.
#[derive(Debug, Clone, Serialize)]
pub enum ConditionConfig {
    MetricThreshold {
        metric: String,
        threshold: Option<f64>,
        comparison: Option<String>,
        #[serde(default)]
        duration_minutes: Option<i32>,
        #[serde(default)]
        hysteresis: Option<f64>,
    },
    DeviceOffline {
        max_minutes_since_poll: i32,
    },
    HealthStatusChange {
        from: Option<String>,
        to: String,
    },
    Prediction {
        prediction_type: String,
        min_probability: f64,
        #[serde(default)]
        max_eta_minutes: Option<i32>,
    },
    Composite {
        #[serde(default)]
        op: Option<String>,
        conditions: Vec<ConditionConfig>,
    },
}

fn parse_condition(value: serde_yaml::Value) -> Result<ConditionConfig, String> {
    let map = value
        .as_mapping()
        .ok_or_else(|| "condition must be a mapping".to_string())?;

    // Nested shape: a single key naming the condition type
    if map.len() == 1 {
        let (key, inner) = map.iter().next().unwrap();
        let key = key
            .as_str()
            .ok_or_else(|| "condition type key must be a string".to_string())?;
        match key {
            "MetricThreshold" | "DeviceOffline" | "HealthStatusChange" | "Prediction"
            | "Composite" => {
                return parse_condition_inner(key, inner.clone());
            }
            _ => {}
        }
    }

    // Flat shape: dispatch on characteristic keys
    let has = |k: &str| map.contains_key(k);
    if has("metric") {
        return parse_condition_inner("MetricThreshold", value);
    }
    if has("max_minutes_since_poll") {
        return parse_condition_inner("DeviceOffline", value);
    }
    if has("prediction_type") || has("min_probability") {
        return parse_condition_inner("Prediction", value);
    }
    if has("conditions") {
        return parse_condition_inner("Composite", value);
    }
    if has("to") || has("from") {
        return parse_condition_inner("HealthStatusChange", value);
    }

    Err("unrecognized condition shape: expected a nested type key or flat fields".into())
}

fn parse_condition_inner(kind: &str, inner: serde_yaml::Value) -> Result<ConditionConfig, String> {
    match kind {
        "MetricThreshold" => {
            let metric = field_str(&inner, "metric")?
                .ok_or_else(|| "MetricThreshold requires 'metric'".to_string())?;
            Ok(ConditionConfig::MetricThreshold {
                metric,
                threshold: field_f64(&inner, "threshold")?,
                comparison: field_str(&inner, "comparison")?,
                duration_minutes: field_i64(&inner, "duration_minutes")?.map(|v| v as i32),
                hysteresis: field_f64(&inner, "hysteresis")?,
            })
        }
        "DeviceOffline" => {
            let minutes = field_i64(&inner, "max_minutes_since_poll")?
                .ok_or_else(|| "DeviceOffline requires 'max_minutes_since_poll'".to_string())?;
            Ok(ConditionConfig::DeviceOffline {
                max_minutes_since_poll: minutes as i32,
            })
        }
        "HealthStatusChange" => {
            let to = field_str(&inner, "to")?
                .ok_or_else(|| "HealthStatusChange requires 'to'".to_string())?;
            Ok(ConditionConfig::HealthStatusChange {
                from: field_str(&inner, "from")?,
                to,
            })
        }
        "Prediction" => {
            let prediction_type = field_str(&inner, "prediction_type")?
                .ok_or_else(|| "Prediction requires 'prediction_type'".to_string())?;
            let min_probability = field_f64(&inner, "min_probability")?
                .ok_or_else(|| "Prediction requires 'min_probability'".to_string())?;
            Ok(ConditionConfig::Prediction {
                prediction_type,
                min_probability,
                max_eta_minutes: field_i64(&inner, "max_eta_minutes")?.map(|v| v as i32),
            })
        }
        "Composite" => {
            let op = match field_str(&inner, "op")? {
                Some(s) => Some(s),
                None => field_str(&inner, "operator")?,
            };
            let conditions_value = inner
                .as_mapping()
                .and_then(|m| m.get("conditions"))
                .cloned()
                .ok_or_else(|| "Composite requires 'conditions'".to_string())?;
            let list = conditions_value
                .as_sequence()
                .ok_or_else(|| "Composite 'conditions' must be a list".to_string())?;
            let mut conditions = Vec::new();
            for item in list {
                conditions.push(parse_condition(item.clone())?);
            }
            Ok(ConditionConfig::Composite { op, conditions })
        }
        _ => unreachable!(),
    }
}

fn get_field<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    let field = value.as_mapping()?.get(key)?;
    if matches!(field, serde_yaml::Value::Null) {
        None
    } else {
        Some(field)
    }
}

fn field_str(value: &serde_yaml::Value, key: &str) -> Result<Option<String>, String> {
    match get_field(value, key) {
        None => Ok(None),
        Some(v) => {
            Ok(Some(v.as_str().map(|s| s.to_string()).ok_or_else(
                || format!("field '{}' must be a string", key),
            )?))
        }
    }
}

fn field_f64(value: &serde_yaml::Value, key: &str) -> Result<Option<f64>, String> {
    match get_field(value, key) {
        None => Ok(None),
        Some(v) => {
            Ok(Some(v.as_f64().ok_or_else(|| {
                format!("field '{}' must be a number", key)
            })?))
        }
    }
}

fn field_i64(value: &serde_yaml::Value, key: &str) -> Result<Option<i64>, String> {
    match get_field(value, key) {
        None => Ok(None),
        Some(v) => {
            Ok(Some(v.as_i64().ok_or_else(|| {
                format!("field '{}' must be an integer", key)
            })?))
        }
    }
}

impl<'de> serde::Deserialize<'de> for ConditionConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        parse_condition(value).map_err(serde::de::Error::custom)
    }
}

/// Self-healing action configuration attached to a rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionConfig {
    /// Run a script on the hub host (argv, no shell). `${ALERT_ID}`,
    /// `${RULE_ID}`, `${DEVICE_ID}`, `${EDGE_ID}`, `${SEVERITY}`,
    /// `${METRIC}`, `${VALUE}`, `${THRESHOLD}` are substituted in the
    /// script path, args and env values.
    Script {
        script: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        timeout_secs: Option<u64>,
        #[serde(default)]
        env: HashMap<String, String>,
        #[serde(default)]
        working_dir: Option<String>,
        /// Not supported: rejected at config load
        #[serde(default)]
        run_as: Option<String>,
    },
    /// Restart an OS service on the hub host
    RestartService { service_name: String },
    /// Power-cycle the device (executed on the edge: DAQmx reset / fallback script)
    PowerCycle {
        #[serde(default)]
        delay_secs: Option<u64>,
    },
    /// Send a command to the edge node attached to the device.
    /// `power_cycle`, `reset_driver`, `restart_services` (parameter
    /// `services`) map to the matching edge action; anything else runs as
    /// an allowlisted edge script named `command`.
    EdgeCommand {
        command: String,
        #[serde(default)]
        parameters: HashMap<String, String>,
    },
    /// Run an allowlisted script on the edge node attached to the device
    CustomScript { script: String },
}

impl ActionConfig {
    /// Validate settings the executor cannot honor.
    pub fn validate(&self) -> Result<(), String> {
        if let ActionConfig::Script {
            run_as: Some(user), ..
        } = self
        {
            return Err(format!(
                "script action 'run_as: {}' is not supported; run the hub as the desired user instead",
                user
            ));
        }
        Ok(())
    }

    /// Build the executable action.
    pub fn to_action(&self, id: &str) -> Action {
        match self {
            ActionConfig::Script {
                script,
                args,
                timeout_secs,
                env,
                working_dir,
                run_as,
            } => {
                let mut action = Action::script(
                    id,
                    "Rule script",
                    "Script action attached to an alert rule",
                    script.clone(),
                );
                if let ActionType::Script(ref mut cfg) = action.action_type {
                    cfg.args = args.clone();
                    cfg.env = env.clone();
                    cfg.working_dir = working_dir.clone();
                    cfg.run_as = run_as.clone();
                }
                if let Some(timeout) = timeout_secs {
                    action.timeout_secs = *timeout;
                }
                action
            }
            ActionConfig::RestartService { service_name } => Action::restart_service(
                id,
                "Rule service restart",
                "Service restart action attached to an alert rule",
                service_name.clone(),
            ),
            ActionConfig::PowerCycle { delay_secs } => {
                let mut action = Action::power_cycle(
                    id,
                    "Rule power cycle",
                    "Power cycle action attached to an alert rule",
                );
                action.action_type = ActionType::PowerCycle {
                    delay_secs: delay_secs.unwrap_or(0),
                };
                action
            }
            ActionConfig::EdgeCommand {
                command,
                parameters,
            } => {
                let mut action = Action::edge_command(
                    id,
                    "Rule edge command",
                    "Edge command action attached to an alert rule",
                );
                action.action_type = ActionType::EdgeCommand {
                    command: command.clone(),
                    parameters: parameters.clone(),
                };
                action
            }
            ActionConfig::CustomScript { script } => {
                let mut action = Action::edge_command(
                    id,
                    "Rule custom script",
                    "Allowlisted edge script attached to an alert rule",
                );
                let mut parameters = HashMap::new();
                parameters.insert("script".to_string(), script.clone());
                action.action_type = ActionType::EdgeCommand {
                    command: "custom_script".to_string(),
                    parameters,
                };
                action
            }
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            ActionConfig::Script { .. } => "script",
            ActionConfig::RestartService { .. } => "restart-service",
            ActionConfig::PowerCycle { .. } => "power-cycle",
            ActionConfig::EdgeCommand { .. } => "edge-command",
            ActionConfig::CustomScript { .. } => "custom-script",
        }
    }
}

/// Desired-state configuration for edge nodes, pushed on registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgesConfig {
    /// Desired state applied to every edge
    #[serde(default)]
    pub defaults: EdgeDesiredConfig,
    /// Per-edge overrides, keyed by edge id
    #[serde(default)]
    pub overrides: Vec<EdgeOverride>,
    /// A connected edge that sends nothing for this long (seconds) is
    /// disconnected and marked offline
    #[serde(default = "default_session_timeout")]
    pub session_timeout_secs: i64,
}

fn default_session_timeout() -> i64 {
    120
}

impl Default for EdgesConfig {
    fn default() -> Self {
        Self {
            defaults: EdgeDesiredConfig::default(),
            overrides: Vec::new(),
            session_timeout_secs: default_session_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeOverride {
    pub edge_id: String,
    #[serde(flatten)]
    pub config: EdgeDesiredConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EdgeDesiredConfig {
    /// Sweep/poll interval in seconds
    pub poll_interval_secs: Option<u64>,
    /// Temperature warning threshold (C)
    pub temperature_warning: Option<f64>,
    /// Temperature critical threshold (C)
    pub temperature_critical: Option<f64>,
}

impl EdgeDesiredConfig {
    /// Overlay the `Some` fields of `other` onto `self`.
    pub fn overlay(&mut self, other: &EdgeDesiredConfig) {
        if other.poll_interval_secs.is_some() {
            self.poll_interval_secs = other.poll_interval_secs;
        }
        if other.temperature_warning.is_some() {
            self.temperature_warning = other.temperature_warning;
        }
        if other.temperature_critical.is_some() {
            self.temperature_critical = other.temperature_critical;
        }
    }

    /// True when nothing is set.
    pub fn is_empty(&self) -> bool {
        self.poll_interval_secs.is_none()
            && self.temperature_warning.is_none()
            && self.temperature_critical.is_none()
    }
}

impl EdgesConfig {
    /// Resolve the desired config for a given edge (defaults + override).
    pub fn resolve(&self, edge_id: &str) -> EdgeDesiredConfig {
        let mut config = self.defaults.clone();
        if let Some(ov) = self.overrides.iter().find(|o| o.edge_id == edge_id) {
            config.overlay(&ov.config);
        }
        config
    }
}

/// Retention / maintenance settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceConfig {
    /// Days resolved alerts are kept
    #[serde(default = "default_alert_retention_days")]
    pub alert_retention_days: i64,
    /// Days metric history is kept
    #[serde(default = "default_metric_retention_days")]
    pub metric_retention_days: i64,
    /// Days predictions are kept
    #[serde(default = "default_prediction_retention_days")]
    pub prediction_retention_days: i64,
    /// Minutes after which an unrefreshed active prediction is expired
    #[serde(default = "default_prediction_expiry_minutes")]
    pub prediction_expiry_minutes: i64,
    /// Days action history is kept
    #[serde(default = "default_action_retention_days")]
    pub action_retention_days: i64,
    /// Minimum spacing between metric-history samples per device (seconds)
    #[serde(default = "default_metric_sample_interval")]
    pub metric_sample_interval_secs: i64,
    /// Interval at which buffered metric samples are written in one batch (seconds)
    #[serde(default = "default_metric_flush_interval")]
    pub metric_flush_interval_secs: u64,
    /// In-memory per-device state is dropped after this many hours without a report
    #[serde(default = "default_device_state_retention_hours")]
    pub device_state_retention_hours: i64,
}

fn default_alert_retention_days() -> i64 {
    30
}
fn default_metric_retention_days() -> i64 {
    30
}
fn default_prediction_retention_days() -> i64 {
    30
}
fn default_prediction_expiry_minutes() -> i64 {
    60
}
fn default_action_retention_days() -> i64 {
    30
}
fn default_metric_sample_interval() -> i64 {
    60
}
fn default_metric_flush_interval() -> u64 {
    10
}
fn default_device_state_retention_hours() -> i64 {
    24
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            alert_retention_days: default_alert_retention_days(),
            metric_retention_days: default_metric_retention_days(),
            prediction_retention_days: default_prediction_retention_days(),
            prediction_expiry_minutes: default_prediction_expiry_minutes(),
            action_retention_days: default_action_retention_days(),
            metric_sample_interval_secs: default_metric_sample_interval(),
            metric_flush_interval_secs: default_metric_flush_interval(),
            device_state_retention_hours: default_device_state_retention_hours(),
        }
    }
}

fn parse_comparison(s: &str) -> Result<ComparisonOp, String> {
    ComparisonOp::parse(s).ok_or_else(|| format!("Unknown comparison operator: {}", s))
}

fn parse_health_status(s: &str) -> Result<HealthStatus, String> {
    match s {
        "healthy" => Ok(HealthStatus::Healthy),
        "warning" => Ok(HealthStatus::Warning),
        "error" => Ok(HealthStatus::Error),
        "offline" => Ok(HealthStatus::Offline),
        other => Err(format!("Unknown health status: {}", other)),
    }
}

impl TryFrom<ConditionConfig> for RuleCondition {
    type Error = String;

    fn try_from(config: ConditionConfig) -> Result<Self, Self::Error> {
        match config {
            ConditionConfig::MetricThreshold {
                metric,
                threshold,
                comparison,
                duration_minutes,
                hysteresis,
            } => {
                let operator = match comparison.as_deref() {
                    Some(c) => parse_comparison(c)?,
                    None => ComparisonOp::GreaterThan,
                };
                let threshold = threshold.ok_or_else(|| {
                    format!("MetricThreshold on '{}' requires 'threshold'", metric)
                })?;
                if let Some(h) = hysteresis {
                    if !h.is_finite() || h < 0.0 {
                        return Err(format!("hysteresis must be >= 0 (got {})", h));
                    }
                }
                Ok(RuleCondition::MetricThreshold {
                    metric_name: metric,
                    operator,
                    threshold,
                    duration_minutes,
                    hysteresis,
                })
            }
            ConditionConfig::DeviceOffline {
                max_minutes_since_poll,
            } => Ok(RuleCondition::DeviceOffline {
                max_minutes_since_poll,
            }),
            ConditionConfig::HealthStatusChange { from, to } => {
                let from_status = match from.as_deref() {
                    Some(s) => Some(parse_health_status(s)?),
                    None => None,
                };
                let to_status = parse_health_status(&to)?;
                Ok(RuleCondition::HealthStatusChange {
                    from: from_status,
                    to: to_status,
                })
            }
            ConditionConfig::Prediction {
                prediction_type,
                min_probability,
                max_eta_minutes,
            } => Ok(RuleCondition::Prediction {
                prediction_type,
                min_probability,
                max_eta_minutes,
            }),
            ConditionConfig::Composite { op, conditions } => {
                let operator = match op.as_deref() {
                    None => Ok(nimon_core::alert::rules::LogicalOp::And),
                    Some("and") => Ok(nimon_core::alert::rules::LogicalOp::And),
                    Some("or") => Ok(nimon_core::alert::rules::LogicalOp::Or),
                    Some(other) => Err(format!(
                        "Unknown logical operator: {} (use 'and'/'or')",
                        other
                    )),
                }?;
                let mut converted = Vec::new();
                for condition in conditions {
                    converted.push(RuleCondition::try_from(condition)?);
                }
                Ok(RuleCondition::Composite {
                    operator,
                    conditions: converted,
                })
            }
        }
    }
}

/// Slug used as rule id base: lowercase, non-alphanumerics collapsed to `-`.
pub fn rule_slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "rule".to_string()
    } else {
        slug
    }
}

/// Convert a RuleConfig to an AlertRule. `cooldown_minutes` falls back
/// to `default_cooldown_minutes` when omitted.
pub fn try_rule_config_to_alert_rule_with_defaults(
    config: &RuleConfig,
    default_cooldown_minutes: i64,
) -> Result<AlertRule, String> {
    let condition = RuleCondition::try_from(config.condition.clone())?;
    let severity = Severity::parse(&config.severity.to_lowercase())?;
    if let Some(action) = &config.action {
        action.validate()?;
    }
    if let Some(action) = &config.on_failure {
        action.validate()?;
    }
    let cooldown_minutes = config
        .cooldown_minutes
        .unwrap_or(default_cooldown_minutes.clamp(0, i32::MAX as i64) as i32);
    if cooldown_minutes < 0 {
        return Err(format!(
            "cooldown_minutes must be >= 0 (got {})",
            cooldown_minutes
        ));
    }
    Ok(AlertRule {
        id: rule_slug(&config.name),
        name: config.name.clone(),
        description: config.description.clone(),
        enabled: config.enabled,
        severity,
        condition,
        cooldown_minutes,
        // Empty = every enabled channel (resolved by the notifier)
        notification_channels: config.notification_channels.clone(),
        suppress_repeat: config.suppress_repeat,
        max_firing_count: config.max_firing_count,
        action: config.action.as_ref().map(config_action_to_ref),
    })
}

/// Convert a RuleConfig to an AlertRule using the global default
/// cooldown (5 minutes) when the rule omits `cooldown_minutes`.
pub fn try_rule_config_to_alert_rule(config: &RuleConfig) -> Result<AlertRule, String> {
    try_rule_config_to_alert_rule_with_defaults(config, default_cooldown())
}

/// Convert a config action into the core rule action reference
/// (informational; the executor runs the richer [`ActionConfig::to_action`]).
pub fn config_action_to_ref(action: &ActionConfig) -> nimon_core::alert::rules::ActionRef {
    use nimon_core::alert::rules::ActionRef;
    match action {
        ActionConfig::Script {
            script,
            args,
            timeout_secs,
            ..
        } => ActionRef::Script {
            script: script.clone(),
            args: args.clone(),
            timeout_secs: *timeout_secs,
        },
        ActionConfig::RestartService { service_name } => ActionRef::RestartService {
            service_name: service_name.clone(),
        },
        ActionConfig::PowerCycle { delay_secs } => ActionRef::PowerCycle {
            delay_secs: *delay_secs,
        },
        ActionConfig::EdgeCommand {
            command,
            parameters,
        } => ActionRef::EdgeCommand {
            command: command.clone(),
            parameters: parameters.clone(),
        },
        ActionConfig::CustomScript { script } => ActionRef::CustomScript {
            script: script.clone(),
        },
    }
}

/// Result of converting the configured rules.
#[derive(Debug, Clone, Default)]
pub struct BuiltRules {
    pub rules: Vec<AlertRule>,
    /// Executable actions per rule id (primary + optional on_failure)
    pub actions: HashMap<String, RuleActionSpec>,
    /// Rules that failed to convert: (rule name, error)
    pub errors: Vec<(String, String)>,
    /// True when rules were configured, so built-in defaults must not be used
    pub configured: bool,
}

/// Convert every configured rule, deduplicating ids (`name`, `name-2`, ...).
pub fn build_rules(alert: &AlertConfig) -> BuiltRules {
    let mut built = BuiltRules {
        configured: !alert.rules.is_empty(),
        ..Default::default()
    };
    let mut used: HashSet<String> = HashSet::new();
    for rule_config in &alert.rules {
        match try_rule_config_to_alert_rule_with_defaults(
            rule_config,
            alert.default_cooldown_minutes,
        ) {
            Ok(mut rule) => {
                let base = rule.id.clone();
                let mut id = base.clone();
                let mut n = 2;
                while used.contains(&id) {
                    id = format!("{}-{}", base, n);
                    n += 1;
                }
                if id != base {
                    tracing::warn!(
                        "Rule '{}' id '{}' already used; using '{}'",
                        rule_config.name,
                        base,
                        id
                    );
                }
                used.insert(id.clone());
                rule.id = id.clone();
                if let Some(action) = &rule_config.action {
                    let on_failure = rule_config.on_failure.as_ref().map(|fallback| {
                        fallback.to_action(&format!("rule-{}-fallback-{}", id, fallback.kind()))
                    });
                    built.actions.insert(
                        id.clone(),
                        RuleActionSpec {
                            action: action.to_action(&format!("rule-{}-{}", id, action.kind())),
                            on_failure,
                        },
                    );
                } else if rule_config.on_failure.is_some() {
                    tracing::warn!(
                        "Rule '{}' has on_failure but no action; on_failure ignored",
                        rule_config.name
                    );
                }
                built.rules.push(rule);
            }
            Err(e) => built.errors.push((rule_config.name.clone(), e)),
        }
    }
    built
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            default_cooldown_minutes: default_cooldown(),
            max_firing_count: default_max_firing(),
            cleanup_interval_hours: default_cleanup_interval(),
            prediction_alert_threshold: default_prediction_threshold(),
            prediction_ttl_minutes: default_prediction_ttl(),
            edge_alert_ttl_minutes: default_edge_alert_ttl(),
            edge_offline_after_secs: default_edge_offline_after(),
            evaluate_interval_secs: default_evaluate_interval(),
            notification_channels: Vec::new(),
            rules: Vec::new(),
        }
    }
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            database_path: default_db_path(),
            alert: AlertConfig::default(),
            edges: EdgesConfig::default(),
            maintenance: MaintenanceConfig::default(),
            auth: AuthConfig::default(),
            cors_allowed_origins: Vec::new(),
        }
    }
}

impl HubConfig {
    /// Load and validate a config file.
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: HubConfig = serde_yaml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    pub fn load_or_default(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            Self::from_file(path)
        } else {
            Ok(Self::default())
        }
    }

    /// Reject settings the hub cannot honor (clear errors at load time).
    pub fn validate(&self) -> anyhow::Result<()> {
        for rule in &self.alert.rules {
            for action in [&rule.action, &rule.on_failure].into_iter().flatten() {
                action
                    .validate()
                    .map_err(|e| anyhow::anyhow!("rule '{}': {}", rule.name, e))?;
            }
        }
        if !(0.0..=1.0).contains(&self.alert.prediction_alert_threshold) {
            anyhow::bail!(
                "alert.prediction_alert_threshold must be within 0.0..=1.0 (got {})",
                self.alert.prediction_alert_threshold
            );
        }
        if self.edges.session_timeout_secs <= 0 {
            anyhow::bail!(
                "edges.session_timeout_secs must be >= 1 (got {}): a non-positive timeout \
                 disconnects every edge on the next session sweep",
                self.edges.session_timeout_secs
            );
        }
        for warning in self.warnings() {
            tracing::warn!("{}", warning);
        }
        Ok(())
    }

    /// Settings that work but are probably not what was intended.
    ///
    /// `edge_offline_after_secs >= edges.session_timeout_secs`: a connected
    /// edge that goes silent is disconnected (session timeout) before the
    /// offline alert would fire; an edge that then immediately reconnects
    /// (edges auto-reconnect) resets the offline clock, so a silent,
    /// flapping edge may never raise the offline alert. Keeping the alert
    /// threshold below the session timeout flags silence while the socket
    /// is still up. Not an error: both orderings are functional.
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        let offline = self.alert.edge_offline_after_secs;
        let timeout = self.edges.session_timeout_secs;
        if offline > 0 && timeout > 0 && offline >= timeout as u64 {
            warnings.push(format!(
                "alert.edge_offline_after_secs ({}) >= edges.session_timeout_secs ({}): silent edges \
                 are disconnected before the edge-offline alert fires, and an edge that reconnects \
                 right away never raises it; keep edge_offline_after_secs below session_timeout_secs",
                offline, timeout
            ));
        }
        warnings
    }

    /// Resolve relative file paths in the config against `base_dir`
    /// (the config file's directory in service mode).
    pub fn resolve_relative_paths(&mut self, base_dir: &Path) {
        let db = Path::new(&self.database_path);
        let is_special =
            self.database_path == ":memory:" || self.database_path.starts_with("sqlite:");
        if !is_special && db.is_relative() {
            self.database_path = base_dir.join(db).display().to_string();
        }
        for channel in &mut self.alert.notification_channels {
            if let Some(file) = &channel.smtp_password_file {
                if Path::new(file).is_relative() {
                    channel.smtp_password_file = Some(base_dir.join(file).display().to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HubConfig::default();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 9090);
        assert_eq!(config.database_path, "data/nimon.db");
        assert_eq!(config.alert.cleanup_interval_hours, 24);
        assert_eq!(config.alert.prediction_alert_threshold, 0.8);
        assert_eq!(config.alert.edge_offline_after_secs, 90);
        assert_eq!(config.alert.prediction_ttl_minutes, 15);
        assert!(config.cors_allowed_origins.is_empty());
        assert!(config.auth.api_token.is_none());
        assert_eq!(config.edges.session_timeout_secs, 120);
    }

    #[test]
    fn test_validate_session_timeout_and_offline_ordering() {
        assert!(HubConfig::default().validate().is_ok());
        assert!(HubConfig::default().warnings().is_empty());
        for bad in [0, -5] {
            let mut config = HubConfig::default();
            config.edges.session_timeout_secs = bad;
            let err = config.validate().unwrap_err().to_string();
            assert!(err.contains("session_timeout_secs"), "{}", err);
        }
        let mut config = HubConfig::default();
        config.alert.edge_offline_after_secs = 120; // == session timeout
        assert!(config.validate().is_ok(), "ordering is a warning only");
        assert_eq!(config.warnings().len(), 1);
        config.alert.edge_offline_after_secs = 0; // disabled
        assert!(config.warnings().is_empty());
    }

    #[test]
    fn test_from_yaml() {
        let yaml = "port: 9191\ndatabase_path: /tmp/nimon.db";
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.port, 9191);
        assert_eq!(config.database_path, "/tmp/nimon.db");
        let config: HubConfig = serde_yaml::from_str("host: 127.0.0.1").unwrap();
        assert_eq!(config.port, 9090);
    }

    // README example, verbatim (nested shape)
    const README_RULES: &str = r#"
alert:
  rules:
  - name: "High Temperature"
    severity: "critical"            # info | warning | critical
    condition:
      MetricThreshold:
        metric: "temperature"
        threshold: 75.0
        comparison: "greater_than"  # gt lt eq ne ge le
    cooldown_minutes: 5
    notification_channels: ["slack-ops"]

  - name: "Device Offline"
    severity: "warning"
    condition:
      DeviceOffline:
        max_minutes_since_poll: 10
"#;

    #[test]
    fn test_readme_rule_examples_parse() {
        let config: HubConfig = serde_yaml::from_str(README_RULES).unwrap();
        assert_eq!(config.alert.rules.len(), 2);

        let high_temp = &config.alert.rules[0];
        match &high_temp.condition {
            ConditionConfig::MetricThreshold {
                metric,
                threshold,
                comparison,
                duration_minutes,
                hysteresis,
            } => {
                assert_eq!(metric, "temperature");
                assert_eq!(*threshold, Some(75.0));
                assert_eq!(comparison.as_deref(), Some("greater_than"));
                assert!(duration_minutes.is_none());
                assert!(hysteresis.is_none());
            }
            other => panic!("expected MetricThreshold, got {:?}", other),
        }

        // Conversion to the core rule succeeds
        let rule = try_rule_config_to_alert_rule(high_temp).unwrap();
        assert_eq!(rule.severity, Severity::Critical);
        assert_eq!(rule.notification_channels, vec!["slack-ops".to_string()]);
        assert_eq!(rule.cooldown_minutes, 5);

        match &config.alert.rules[1].condition {
            ConditionConfig::DeviceOffline {
                max_minutes_since_poll,
            } => {
                assert_eq!(*max_minutes_since_poll, 10);
            }
            other => panic!("expected DeviceOffline, got {:?}", other),
        }
        // No channels configured: every enabled channel (empty list)
        let offline = try_rule_config_to_alert_rule(&config.alert.rules[1]).unwrap();
        assert!(offline.notification_channels.is_empty());
    }

    #[test]
    fn test_cooldown_falls_back_to_default() {
        let yaml = r#"
alert:
  default_cooldown_minutes: 12
  rules:
  - name: "No cooldown given"
    condition:
      metric: "temperature"
      threshold: 70.0
  - name: "Explicit zero"
    cooldown_minutes: 0
    condition:
      metric: "temperature"
      threshold: 70.0
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.alert.rules[0].cooldown_minutes, None);
        let built = build_rules(&config.alert);
        assert!(built.errors.is_empty());
        assert_eq!(built.rules[0].cooldown_minutes, 12, "falls back to default");
        assert_eq!(built.rules[1].cooldown_minutes, 0, "explicit 0 honored");

        // Without an explicit global default the built-in 5 minutes apply
        let config: HubConfig = serde_yaml::from_str(
            "alert:\n  rules:\n  - name: x\n    condition:\n      metric: t\n      threshold: 1\n",
        )
        .unwrap();
        assert_eq!(build_rules(&config.alert).rules[0].cooldown_minutes, 5);
    }

    #[test]
    fn test_rule_slug_dedupe_and_all_failed() {
        let yaml = r#"
alert:
  rules:
  - name: "High Temp"
    condition: { metric: "temperature", threshold: 70.0 }
  - name: "high temp"
    condition: { metric: "temperature", threshold: 80.0 }
  - name: "High  Temp!"
    condition: { metric: "temperature", threshold: 90.0 }
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        let built = build_rules(&config.alert);
        let ids: Vec<&str> = built.rules.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["high-temp", "high-temp-2", "high-temp-3"]);

        // All rules invalid: configured stays true (no silent defaults)
        let yaml = r#"
alert:
  rules:
  - name: "bad"
    severity: "extreme"
    condition: { metric: "temperature", threshold: 70.0 }
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        let built = build_rules(&config.alert);
        assert!(built.rules.is_empty());
        assert_eq!(built.errors.len(), 1);
        assert!(built.configured);
        assert!(!build_rules(&AlertConfig::default()).configured);
    }

    #[test]
    fn test_rule_max_firing_and_on_failure() {
        let yaml = r#"
alert:
  rules:
  - name: "Hot"
    max_firing_count: 3
    condition: { metric: "temperature", threshold: 70.0, hysteresis: 2.5 }
    action:
      type: power_cycle
    on_failure:
      type: custom_script
      script: "escalate.ps1"
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        let built = build_rules(&config.alert);
        assert_eq!(built.rules[0].max_firing_count, Some(3));
        assert!(matches!(
            built.rules[0].condition,
            RuleCondition::MetricThreshold {
                hysteresis: Some(h),
                ..
            } if h == 2.5
        ));
        let spec = built.actions.get("hot").unwrap();
        assert!(matches!(
            spec.action.action_type,
            ActionType::PowerCycle { .. }
        ));
        let fallback = spec.on_failure.as_ref().unwrap();
        match &fallback.action_type {
            ActionType::EdgeCommand {
                command,
                parameters,
            } => {
                assert_eq!(command, "custom_script");
                assert_eq!(parameters.get("script").unwrap(), "escalate.ps1");
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn test_run_as_rejected_at_load() {
        let yaml = r#"
alert:
  rules:
  - name: "Script as root"
    condition: { metric: "temperature", threshold: 70.0 }
    action:
      type: script
      script: "/opt/fix.sh"
      run_as: "root"
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("run_as"), "{}", err);
    }

    #[test]
    fn test_script_env_and_working_dir() {
        let yaml = r#"
alert:
  rules:
  - name: "Script"
    condition: { metric: "temperature", threshold: 70.0 }
    action:
      type: script
      script: "fix"
      args: ["${DEVICE_ID}"]
      env: { LEVEL: "${SEVERITY}" }
      working_dir: "/tmp"
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        let built = build_rules(&config.alert);
        let spec = built.actions.get("script").unwrap();
        match &spec.action.action_type {
            ActionType::Script(cfg) => {
                assert_eq!(cfg.env.get("LEVEL").unwrap(), "${SEVERITY}");
                assert_eq!(cfg.working_dir.as_deref(), Some("/tmp"));
                assert_eq!(cfg.args, vec!["${DEVICE_ID}".to_string()]);
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn test_flat_legacy_shape_parses() {
        let yaml = r#"
alert:
  rules:
  - name: "High Temperature"
    condition:
      metric: "temperature"
      threshold: 70.0
      comparison: "gt"
  - name: "Offline flat"
    condition:
      max_minutes_since_poll: 5
  - name: "Status change flat"
    condition:
      from: "healthy"
      to: "error"
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.alert.rules.len(), 3);
        assert!(matches!(
            config.alert.rules[0].condition,
            ConditionConfig::MetricThreshold { .. }
        ));
        assert!(matches!(
            config.alert.rules[1].condition,
            ConditionConfig::DeviceOffline { .. }
        ));
        assert!(matches!(
            config.alert.rules[2].condition,
            ConditionConfig::HealthStatusChange { .. }
        ));
        try_rule_config_to_alert_rule(&config.alert.rules[0]).unwrap();
        try_rule_config_to_alert_rule(&config.alert.rules[1]).unwrap();
        try_rule_config_to_alert_rule(&config.alert.rules[2]).unwrap();
    }

    #[test]
    fn test_prediction_and_composite_conditions_parse() {
        let yaml = r#"
alert:
  rules:
  - name: "Overheat predicted"
    condition:
      Prediction:
        prediction_type: "overheating"
        min_probability: 0.7
        max_eta_minutes: 30
  - name: "Hot and trending"
    condition:
      Composite:
        op: "and"
        conditions:
          - MetricThreshold:
              metric: "temperature"
              threshold: 60.0
          - DeviceOffline:
              max_minutes_since_poll: 3
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(
            config.alert.rules[0].condition,
            ConditionConfig::Prediction { .. }
        ));
        assert!(matches!(
            config.alert.rules[1].condition,
            ConditionConfig::Composite { .. }
        ));
        try_rule_config_to_alert_rule(&config.alert.rules[0]).unwrap();
        try_rule_config_to_alert_rule(&config.alert.rules[1]).unwrap();
    }

    #[test]
    fn test_nested_health_status_change_parses() {
        let yaml = r#"
alert:
  rules:
  - name: "Device error"
    condition:
      HealthStatusChange:
        from: "healthy"
        to: "error"
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        match &config.alert.rules[0].condition {
            ConditionConfig::HealthStatusChange { from, to } => {
                assert_eq!(from.as_deref(), Some("healthy"));
                assert_eq!(to, "error");
            }
            other => panic!("expected HealthStatusChange, got {:?}", other),
        }
    }

    #[test]
    fn test_rule_action_parses() {
        let yaml = r#"
alert:
  rules:
  - name: "Critical temp with action"
    severity: critical
    condition:
      metric: "temperature"
      threshold: 85.0
    action:
      type: power_cycle
      delay_secs: 5
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.alert.rules[0].action.is_some());
    }

    #[test]
    fn test_invalid_condition_shape_errors() {
        let yaml = r#"
alert:
  rules:
  - name: "Broken"
    condition:
      nonsense_field: true
"#;
        let result: Result<HubConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_full_notification_channel_config() {
        let yaml = r#"
alert:
  notification_channels:
    - channel_type: "email"
      smtp_server: "smtp.example.com"
      smtp_port: 465
      smtp_username: "nimon@example.com"
      from_addr: "nimon@example.com"
      to_addrs: ["ops@example.com"]
    - channel_type: "webhook"
      webhook_url: "https://hooks.example.com/x"
      headers:
        - ["X-Token", "abc"]
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.alert.notification_channels.len(), 2);
        let email = &config.alert.notification_channels[0];
        assert_eq!(email.smtp_port, Some(465));
        assert_eq!(email.smtp_username.as_deref(), Some("nimon@example.com"));
        assert!(!email.smtp_allow_plaintext_auth);
        let webhook = &config.alert.notification_channels[1];
        assert_eq!(
            webhook.headers.as_ref().unwrap()[0],
            vec!["X-Token".to_string(), "abc".to_string()]
        );
    }

    #[test]
    fn test_smtp_env_overrides_config() {
        let yaml = r#"
channel_type: "email"
name: "mail"
smtp_username: "file-user"
smtp_password: "file-pass"
"#;
        let channel: NotificationChannelConfig = serde_yaml::from_str(yaml).unwrap();

        // No env: config values
        let none = |_: &str| None;
        assert_eq!(
            channel.smtp_credentials_with(&none),
            (Some("file-user".into()), Some("file-pass".into()))
        );

        // Env wins over config
        let env = |k: &str| match k {
            "NIMON_SMTP_USER" => Some("env-user".to_string()),
            "NIMON_SMTP_PASSWORD" => Some("env-pass".to_string()),
            _ => None,
        };
        assert_eq!(
            channel.smtp_credentials_with(&env),
            (Some("env-user".into()), Some("env-pass".into()))
        );

        // Password file via env
        let dir = std::env::temp_dir().join(format!("nimon-hub-cfg-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("smtp-password");
        std::fs::write(&file, "from-file\n").unwrap();
        let file_str = file.display().to_string();
        let env = move |k: &str| match k {
            "NIMON_SMTP_PASSWORD_FILE" => Some(file_str.clone()),
            _ => None,
        };
        assert_eq!(
            channel.smtp_credentials_with(&env).1.as_deref(),
            Some("from-file")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_auth_env_overrides_config() {
        let auth = AuthConfig {
            api_token: Some("file-token".into()),
            edge_token: None,
        };
        let none = |_: &str| None;
        let resolved = auth.resolved_with(&none);
        assert_eq!(resolved.api_token.as_deref(), Some("file-token"));
        assert!(resolved.edge_token.is_none());

        let env = |k: &str| match k {
            "NIMON_API_TOKEN" => Some("env-token".to_string()),
            "NIMON_EDGE_TOKEN" => Some("edge-env".to_string()),
            _ => None,
        };
        let resolved = auth.resolved_with(&env);
        assert_eq!(resolved.api_token.as_deref(), Some("env-token"));
        assert_eq!(resolved.edge_token.as_deref(), Some("edge-env"));
    }

    #[test]
    fn test_edges_desired_state() {
        let yaml = r#"
edges:
  defaults:
    poll_interval_secs: 30
    temperature_warning: 65
    temperature_critical: 75
  overrides:
    - edge_id: "edge-02"
      temperature_warning: 60
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        let d = config.edges.resolve("edge-01");
        assert_eq!(d.poll_interval_secs, Some(30));
        assert_eq!(d.temperature_warning, Some(65.0));
        let o = config.edges.resolve("edge-02");
        assert_eq!(o.temperature_warning, Some(60.0));
        assert_eq!(o.temperature_critical, Some(75.0));
    }

    #[test]
    fn test_maintenance_config() {
        let yaml = r#"
maintenance:
  alert_retention_days: 7
"#;
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.maintenance.alert_retention_days, 7);
        assert_eq!(config.maintenance.metric_retention_days, 30);
        assert_eq!(config.maintenance.action_retention_days, 30);
        assert_eq!(config.maintenance.metric_flush_interval_secs, 10);
    }

    #[test]
    fn test_resolve_relative_paths() {
        let mut config = HubConfig::default();
        let base = std::path::PathBuf::from("base-dir");
        config.resolve_relative_paths(&base);
        assert_eq!(
            std::path::PathBuf::from(&config.database_path),
            base.join("data/nimon.db")
        );
        let mut memory = HubConfig {
            database_path: ":memory:".into(),
            ..Default::default()
        };
        memory.resolve_relative_paths(&base);
        assert_eq!(memory.database_path, ":memory:");
    }

    #[test]
    fn test_condition_config_roundtrip() {
        // Serialize a ConditionConfig and parse it back through both shapes
        let condition = ConditionConfig::MetricThreshold {
            metric: "temperature".to_string(),
            threshold: Some(75.0),
            comparison: Some("gt".to_string()),
            duration_minutes: Some(5),
            hysteresis: None,
        };
        let yaml = serde_yaml::to_string(&condition).unwrap();
        let parsed: ConditionConfig = serde_yaml::from_str(&yaml).unwrap();
        assert!(matches!(parsed, ConditionConfig::MetricThreshold { .. }));
    }
}
