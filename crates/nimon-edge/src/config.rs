//! Configuration for the edge node
//!
//! This module handles loading and validating edge node configuration
//! from YAML files.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Main configuration for an edge node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeConfig {
    /// Node identification and connection settings
    pub node: NodeConfig,
    /// API-specific settings
    #[serde(default)]
    pub api: ApiConfig,
    /// Prediction engine settings
    #[serde(default)]
    pub prediction: PredictionConfig,
    /// Remediation action settings (hub-requested execution)
    #[serde(default)]
    pub action: ActionConfig,
    /// Data buffering settings
    #[serde(default)]
    pub buffer: BufferConfig,
    /// Logging settings
    #[serde(default)]
    pub logging: LoggingConfig,
}

impl EdgeConfig {
    /// Load configuration from a YAML file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content =
            std::fs::read_to_string(path.as_ref()).map_err(|e| ConfigError::Io(e.to_string()))?;
        Self::from_yaml(&content)
    }

    /// Parse configuration from a YAML string
    pub fn from_yaml(yaml: &str) -> Result<Self, ConfigError> {
        let config: Self = serde_yaml::from_str(yaml)?;
        config.validate()?;
        Ok(config)
    }

    /// Serialize configuration to YAML
    pub fn to_yaml(&self) -> Result<String, ConfigError> {
        Ok(serde_yaml::to_string(self)?)
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.node.id.is_empty() {
            return Err(ConfigError::Validation("node.id cannot be empty".into()));
        }
        if self.node.name.is_empty() {
            return Err(ConfigError::Validation("node.name cannot be empty".into()));
        }
        if self.node.hub_address.is_empty() {
            return Err(ConfigError::Validation(
                "node.hub_address cannot be empty".into(),
            ));
        }
        if self.node.reconnect_interval_secs == 0 {
            return Err(ConfigError::Validation(
                "node.reconnect_interval_secs must be > 0".into(),
            ));
        }
        Ok(())
    }
}

/// Node identification and connection settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Unique identifier for this edge node
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Hub server address (host:port)
    pub hub_address: String,
    /// Interval between reconnection attempts (seconds)
    #[serde(default = "default_reconnect_interval")]
    pub reconnect_interval_secs: u64,
}

fn default_reconnect_interval() -> u64 {
    5
}

/// API-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiConfig {
    /// NI-SysCfg settings
    #[serde(default)]
    pub syscfg: ApiSettings,
    /// NI-DAQmx settings
    #[serde(default)]
    pub daqmx: ApiSettings,
    /// NI-VISA settings
    #[serde(default)]
    pub visa: ApiSettings,
    /// NI-XNET settings
    #[serde(default)]
    pub xnet: ApiSettings,
}

/// Settings for a specific NI API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSettings {
    /// Whether this API is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Polling interval in seconds
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
}

fn default_enabled() -> bool {
    true
}

fn default_poll_interval() -> u64 {
    10
}

impl Default for ApiSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 10,
        }
    }
}

/// Prediction engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionConfig {
    /// Whether prediction is enabled
    #[serde(default = "default_prediction_enabled")]
    pub enabled: bool,
    /// List of prediction models to use
    #[serde(default = "default_models")]
    pub models: Vec<String>,
    /// Threshold for temperature warning (C)
    #[serde(default = "default_temp_warning")]
    pub temperature_warning: f64,
    /// Threshold for temperature critical (C)
    #[serde(default = "default_temp_critical")]
    pub temperature_critical: f64,
}

fn default_prediction_enabled() -> bool {
    true
}

fn default_models() -> Vec<String> {
    vec!["threshold".to_string(), "ewma_anomaly".to_string()]
}

fn default_temp_warning() -> f64 {
    65.0
}

fn default_temp_critical() -> f64 {
    75.0
}

impl Default for PredictionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            models: default_models(),
            temperature_warning: default_temp_warning(),
            temperature_critical: default_temp_critical(),
        }
    }
}

/// Remediation action execution settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionConfig {
    /// Script names this edge is allowed to run (CustomScript actions)
    #[serde(default)]
    pub allowed_scripts: Vec<String>,
    /// Directory containing allowlisted scripts
    #[serde(default = "default_scripts_dir")]
    pub scripts_dir: String,
    /// Per-script timeout in seconds
    #[serde(default = "default_script_timeout")]
    pub timeout_secs: u64,
}

fn default_scripts_dir() -> String {
    "./scripts".to_string()
}

fn default_script_timeout() -> u64 {
    120
}

impl Default for ActionConfig {
    fn default() -> Self {
        Self {
            allowed_scripts: Vec::new(),
            scripts_dir: default_scripts_dir(),
            timeout_secs: default_script_timeout(),
        }
    }
}

/// Data buffering configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferConfig {
    /// Whether buffering is enabled
    #[serde(default = "default_buffer_enabled")]
    pub enabled: bool,
    /// Maximum buffer size in MB
    #[serde(default = "default_buffer_size")]
    pub max_size_mb: u64,
    /// Path to persist buffered data
    #[serde(default = "default_buffer_path")]
    pub persist_path: String,
}

fn default_buffer_enabled() -> bool {
    true
}

fn default_buffer_size() -> u64 {
    100
}

fn default_buffer_path() -> String {
    "./data/buffer.db".to_string()
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size_mb: 100,
            persist_path: default_buffer_path(),
        }
    }
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Path to log file
    #[serde(default = "default_log_file")]
    pub file: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_file() -> String {
    "./logs/nimon-edge.log".to_string()
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file: default_log_file(),
        }
    }
}

/// Configuration error type
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(String),

    #[error("YAML parsing error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Validation error: {0}")]
    Validation(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let yaml = r#"
node:
  id: test-edge-01
  name: Test Edge
  hub_address: "192.168.1.100:8080"
"#;
        let config = EdgeConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.node.id, "test-edge-01");
        assert_eq!(config.node.reconnect_interval_secs, 5);
        assert!(config.prediction.enabled);
        assert!(config.buffer.enabled);
    }

    #[test]
    fn test_default_config() {
        let yaml = r#"
node:
  id: x
  name: y
  hub_address: "z:8080"
"#;
        let config = EdgeConfig::from_yaml(yaml).unwrap();
        assert!(config.prediction.enabled);
        assert!(config.buffer.enabled);
        assert_eq!(config.prediction.temperature_warning, 65.0);
        assert_eq!(config.prediction.temperature_critical, 75.0);
    }

    #[test]
    fn test_validation_empty_id() {
        let yaml = r#"
node:
  id: ""
  name: "Test"
  hub_address: "localhost:8080"
"#;
        let result = EdgeConfig::from_yaml(yaml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("node.id"));
    }

    #[test]
    fn test_validation_empty_hub_address() {
        let yaml = r#"
node:
  id: "test-01"
  name: "Test"
  hub_address: ""
"#;
        let result = EdgeConfig::from_yaml(yaml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("hub_address"));
    }

    #[test]
    fn test_full_config() {
        let yaml = r#"
node:
  id: test-edge-01
  name: Test Edge
  hub_address: "192.168.1.100:8080"
  reconnect_interval_secs: 10

api:
  syscfg:
    enabled: true
    poll_interval_secs: 10
  daqmx:
    enabled: true
    poll_interval_secs: 5
  visa:
    enabled: false
    poll_interval_secs: 30
  xnet:
    enabled: true
    poll_interval_secs: 2

prediction:
  enabled: true
  models:
    - threshold
    - ewma_anomaly
    - trend_prediction
  temperature_warning: 70.0
  temperature_critical: 80.0

buffer:
  enabled: true
  max_size_mb: 200
  persist_path: "/var/lib/nimon/buffer.db"

logging:
  level: debug
  file: "/var/log/nimon/edge.log"
"#;
        let config = EdgeConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.node.reconnect_interval_secs, 10);
        assert_eq!(config.api.visa.enabled, false);
        assert_eq!(config.prediction.models.len(), 3);
        assert_eq!(config.buffer.max_size_mb, 200);
        assert_eq!(config.logging.level, "debug");
    }

    #[test]
    fn test_serialize_config() {
        let config = EdgeConfig {
            node: NodeConfig {
                id: "test".to_string(),
                name: "Test".to_string(),
                hub_address: "localhost:8080".to_string(),
                reconnect_interval_secs: 5,
            },
            api: ApiConfig::default(),
            prediction: PredictionConfig::default(),
            action: ActionConfig::default(),
            buffer: BufferConfig::default(),
            logging: LoggingConfig::default(),
        };
        let yaml = config.to_yaml().unwrap();
        assert!(yaml.contains("id: test"));
        assert!(yaml.contains("hub_address: localhost:8080"));
    }

    #[test]
    fn test_action_config_parses() {
        let yaml = r#"
node:
  id: test-edge-01
  name: Test Edge
  hub_address: "localhost:8080"

action:
  allowed_scripts:
    - drain-chassis.ps1
    - reset-relays.sh
  scripts_dir: "/opt/nimon/scripts"
  timeout_secs: 60
"#;
        let config = EdgeConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.action.allowed_scripts.len(), 2);
        assert_eq!(config.action.scripts_dir, "/opt/nimon/scripts");
        assert_eq!(config.action.timeout_secs, 60);
    }

    #[test]
    fn test_action_config_defaults() {
        let yaml = r#"
node:
  id: test-edge-01
  name: Test Edge
  hub_address: "localhost:8080"
"#;
        let config = EdgeConfig::from_yaml(yaml).unwrap();
        assert!(config.action.allowed_scripts.is_empty());
        assert_eq!(config.action.scripts_dir, "./scripts");
        assert_eq!(config.action.timeout_secs, 120);
    }
}
