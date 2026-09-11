//! Hub server configuration

use serde::{Deserialize, Serialize};
use std::path::Path;

use nimon_core::alert::rules::{AlertRule, ComparisonOp, RuleCondition};
use nimon_core::alert::Severity;
use nimon_core::types::HealthStatus;

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
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    8080
}
fn default_db_path() -> String {
    "data/nimon.db".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    #[serde(default = "default_cooldown")]
    pub default_cooldown_minutes: i64,
    #[serde(default = "default_max_firing")]
    pub max_firing_count: i32,
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_hours: i64,
    /// Probability at which a prediction is persisted/alerted (0.0-1.0)
    #[serde(default = "default_prediction_threshold")]
    pub prediction_alert_threshold: f64,
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
    /// SMTP username; env NIMON_SMTP_USER overrides when set
    #[serde(default)]
    pub smtp_username: Option<String>,
    /// SMTP password; env NIMON_SMTP_PASSWORD overrides when set
    #[serde(default)]
    pub smtp_password: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleConfig {
    pub name: String,
    pub condition: ConditionConfig,
    #[serde(default)]
    pub action: Option<ActionConfig>,
    #[serde(default = "default_severity")]
    pub severity: String,
    #[serde(default = "default_rule_description")]
    pub description: String,
    #[serde(default = "default_rule_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub cooldown_minutes: i32,
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
    /// Run a script on the hub host
    Script {
        script: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        timeout_secs: Option<u64>,
    },
    /// Restart an OS service on the hub host
    RestartService { service_name: String },
    /// Power-cycle the device (executed on the edge: DAQmx reset / fallback script)
    PowerCycle {
        #[serde(default)]
        delay_secs: Option<u64>,
    },
    /// Send a command to the edge node attached to the device
    EdgeCommand {
        command: String,
        #[serde(default)]
        parameters: std::collections::HashMap<String, String>,
    },
    /// Run an allowlisted script on the edge node attached to the device
    CustomScript { script: String },
}

/// Desired-state configuration for edge nodes, pushed on registration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EdgesConfig {
    /// Per-edge desired state, keyed by edge id
    #[serde(default)]
    pub defaults: EdgeDesiredConfig,
    #[serde(default)]
    pub overrides: Vec<EdgeOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeOverride {
    pub edge_id: String,
    #[serde(flatten)]
    pub config: EdgeDesiredConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EdgeDesiredConfig {
    /// Sweep/poll interval in seconds
    pub poll_interval_secs: Option<u64>,
    /// Temperature warning threshold (C)
    pub temperature_warning: Option<f64>,
    /// Temperature critical threshold (C)
    pub temperature_critical: Option<f64>,
}

impl EdgesConfig {
    /// Resolve the desired config for a given edge (defaults + override).
    pub fn resolve(&self, edge_id: &str) -> EdgeDesiredConfig {
        let mut config = self.defaults.clone();
        if let Some(ov) = self.overrides.iter().find(|o| o.edge_id == edge_id) {
            if ov.config.poll_interval_secs.is_some() {
                config.poll_interval_secs = ov.config.poll_interval_secs;
            }
            if ov.config.temperature_warning.is_some() {
                config.temperature_warning = ov.config.temperature_warning;
            }
            if ov.config.temperature_critical.is_some() {
                config.temperature_critical = ov.config.temperature_critical;
            }
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

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            alert_retention_days: default_alert_retention_days(),
            metric_retention_days: default_metric_retention_days(),
            prediction_retention_days: default_prediction_retention_days(),
            prediction_expiry_minutes: default_prediction_expiry_minutes(),
        }
    }
}

fn parse_comparison(s: &str) -> Result<ComparisonOp, String> {
    match s {
        "gt" | "greater_than" | ">" => Ok(ComparisonOp::GreaterThan),
        "lt" | "less_than" | "<" => Ok(ComparisonOp::LessThan),
        "eq" | "equal" | "==" => Ok(ComparisonOp::Equal),
        "ne" | "not_equal" | "!=" => Ok(ComparisonOp::NotEqual),
        "ge" | "greater_or_equal" | ">=" => Ok(ComparisonOp::GreaterOrEqual),
        "le" | "less_or_equal" | "<=" => Ok(ComparisonOp::LessOrEqual),
        other => Err(format!("Unknown comparison operator: {}", other)),
    }
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
            } => {
                let operator = match comparison.as_deref() {
                    Some(c) => parse_comparison(c)?,
                    None => ComparisonOp::GreaterThan,
                };
                Ok(RuleCondition::MetricThreshold {
                    metric_name: metric,
                    operator,
                    threshold: threshold.unwrap_or(0.0),
                    duration_minutes,
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

/// Convert a RuleConfig to an AlertRule
pub fn try_rule_config_to_alert_rule(config: &RuleConfig) -> Result<AlertRule, String> {
    let condition = RuleCondition::try_from(config.condition.clone())?;
    let severity = Severity::parse(&config.severity.to_lowercase())?;
    // Generate a slug id from the name
    let id = config.name.to_lowercase().replace(' ', "-");
    Ok(AlertRule {
        id,
        name: config.name.clone(),
        description: config.description.clone(),
        enabled: config.enabled,
        severity,
        condition,
        cooldown_minutes: config.cooldown_minutes,
        notification_channels: if config.notification_channels.is_empty() {
            vec!["console".to_string()]
        } else {
            config.notification_channels.clone()
        },
        suppress_repeat: config.suppress_repeat,
        max_firing_count: None,
        action: None,
    })
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            default_cooldown_minutes: default_cooldown(),
            max_firing_count: default_max_firing(),
            cleanup_interval_hours: default_cleanup_interval(),
            prediction_alert_threshold: default_prediction_threshold(),
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
        }
    }
}

impl HubConfig {
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: HubConfig = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    pub fn load_or_default(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            Self::from_file(path)
        } else {
            Ok(Self::default())
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
        assert_eq!(config.port, 8080);
        assert_eq!(config.database_path, "data/nimon.db");
        assert_eq!(config.alert.cleanup_interval_hours, 24);
        assert_eq!(config.alert.prediction_alert_threshold, 0.8);
    }

    #[test]
    fn test_from_yaml() {
        let yaml = "port: 9090\ndatabase_path: /tmp/nimon.db";
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.port, 9090);
        assert_eq!(config.database_path, "/tmp/nimon.db");
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
            } => {
                assert_eq!(metric, "temperature");
                assert_eq!(*threshold, Some(75.0));
                assert_eq!(comparison.as_deref(), Some("greater_than"));
                assert!(duration_minutes.is_none());
            }
            other => panic!("expected MetricThreshold, got {:?}", other),
        }

        // Conversion to the core rule succeeds
        let rule = try_rule_config_to_alert_rule(high_temp).unwrap();
        assert_eq!(rule.severity, Severity::Critical);
        assert_eq!(rule.notification_channels, vec!["slack-ops".to_string()]);

        match &config.alert.rules[1].condition {
            ConditionConfig::DeviceOffline {
                max_minutes_since_poll,
            } => {
                assert_eq!(*max_minutes_since_poll, 10);
            }
            other => panic!("expected DeviceOffline, got {:?}", other),
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
        let webhook = &config.alert.notification_channels[1];
        assert_eq!(
            webhook.headers.as_ref().unwrap()[0],
            vec!["X-Token".to_string(), "abc".to_string()]
        );
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
    }

    #[test]
    fn test_condition_config_roundtrip() {
        // Serialize a ConditionConfig and parse it back through both shapes
        let condition = ConditionConfig::MetricThreshold {
            metric: "temperature".to_string(),
            threshold: Some(75.0),
            comparison: Some("gt".to_string()),
            duration_minutes: Some(5),
        };
        let yaml = serde_yaml::to_string(&condition).unwrap();
        let parsed: ConditionConfig = serde_yaml::from_str(&yaml).unwrap();
        assert!(matches!(parsed, ConditionConfig::MetricThreshold { .. }));
    }
}
