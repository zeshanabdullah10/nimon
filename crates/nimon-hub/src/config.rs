//! Hub server configuration

use serde::{Deserialize, Serialize};
use std::path::Path;

use nimon_core::alert::rules::{AlertRule, ComparisonOp, RuleCondition};
use nimon_core::alert::AlertSeverity;
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
}

fn default_host() -> String { "0.0.0.0".to_string() }
fn default_port() -> u16 { 8080 }
fn default_db_path() -> String { "data/nimon.db".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    #[serde(default = "default_cooldown")]
    pub default_cooldown_minutes: i64,
    #[serde(default = "default_max_firing")]
    pub max_firing_count: i32,
    #[serde(default)]
    pub notification_channels: Vec<NotificationChannelConfig>,
    #[serde(default)]
    pub rules: Vec<RuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannelConfig {
    pub channel_type: String,  // "email", "slack", "teams", "webhook", "console"
    pub name: Option<String>,
    pub webhook_url: Option<String>,
    pub smtp_server: Option<String>,
    pub from_addr: Option<String>,
    pub to_addrs: Option<Vec<String>>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool { true }

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

fn default_severity() -> String { "warning".to_string() }
fn default_rule_description() -> String { String::new() }
fn default_rule_enabled() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionConfig {
    #[serde(default)]
    pub action_type: Option<String>,
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub service_name: Option<String>,
}

impl TryFrom<ConditionConfig> for RuleCondition {
    type Error = String;

    fn try_from(config: ConditionConfig) -> Result<Self, Self::Error> {
        match config {
            ConditionConfig::MetricThreshold { metric, threshold, comparison, duration_minutes } => {
                let operator = match comparison.as_deref() {
                    Some("gt") | Some("greater_than") => ComparisonOp::GreaterThan,
                    Some("lt") | Some("less_than") => ComparisonOp::LessThan,
                    Some("eq") | Some("equal") => ComparisonOp::Equal,
                    Some("ne") | Some("not_equal") => ComparisonOp::NotEqual,
                    Some("ge") | Some("greater_or_equal") => ComparisonOp::GreaterOrEqual,
                    Some("le") | Some("less_or_equal") => ComparisonOp::LessOrEqual,
                    Some(other) => return Err(format!("Unknown comparison operator: {}", other)),
                    None => ComparisonOp::GreaterThan,
                };
                Ok(RuleCondition::MetricThreshold {
                    metric_name: metric,
                    operator,
                    threshold: threshold.unwrap_or(0.0),
                    duration_minutes,
                })
            }
            ConditionConfig::DeviceOffline { max_minutes_since_poll } => {
                Ok(RuleCondition::DeviceOffline { max_minutes_since_poll })
            }
            ConditionConfig::HealthStatusChange { from, to } => {
                let from_status = match from.as_deref() {
                    Some("healthy") => Some(HealthStatus::Healthy),
                    Some("warning") => Some(HealthStatus::Warning),
                    Some("error") => Some(HealthStatus::Error),
                    Some("offline") => Some(HealthStatus::Offline),
                    Some(other) => return Err(format!("Unknown health status: {}", other)),
                    None => None,
                };
                let to_status = match to.as_str() {
                    "healthy" => HealthStatus::Healthy,
                    "warning" => HealthStatus::Warning,
                    "error" => HealthStatus::Error,
                    "offline" => HealthStatus::Offline,
                    other => return Err(format!("Unknown health status: {}", other)),
                };
                Ok(RuleCondition::HealthStatusChange {
                    from: from_status,
                    to: to_status,
                })
            }
        }
    }
}

/// Convert a RuleConfig to an AlertRule
pub fn try_rule_config_to_alert_rule(config: &RuleConfig) -> Result<AlertRule, String> {
    let condition = RuleCondition::try_from(config.condition.clone())?;
    let severity = match config.severity.to_lowercase().as_str() {
        "info" => AlertSeverity::Info,
        "warning" => AlertSeverity::Warning,
        "critical" => AlertSeverity::Critical,
        other => return Err(format!("Unknown severity: {}", other)),
    };
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
    })
}

fn default_cooldown() -> i64 { 5 }
fn default_max_firing() -> i32 { 100 }

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            default_cooldown_minutes: default_cooldown(),
            max_firing_count: default_max_firing(),
            notification_channels: Vec::new(),
            rules: Vec::new(),
        }
    }
}

impl Default for HubConfig {
    fn default() -> Self {
        Self { host: default_host(), port: default_port(), database_path: default_db_path(), alert: AlertConfig::default() }
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
    }

    #[test]
    fn test_from_yaml() {
        let yaml = "port: 9090\ndatabase_path: /tmp/nimon.db";
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.port, 9090);
        assert_eq!(config.database_path, "/tmp/nimon.db");
    }
}
