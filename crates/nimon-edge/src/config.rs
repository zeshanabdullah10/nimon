//! Configuration for the edge node
//!
//! This module handles loading and validating edge node configuration
//! from YAML files. Unknown keys are ignored (older configs may still
//! carry e.g. `api.xnet` or `buffer.persist_path`), so upgrading the edge
//! never breaks an existing file.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use nimon_core::{DEFAULT_TEMP_CRITICAL_C, DEFAULT_TEMP_WARNING_C};

/// Environment variable that overrides `node.hub_token`
pub const HUB_TOKEN_ENV: &str = "NIMON_EDGE_TOKEN";

/// Main configuration for an edge node
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// Offline message buffering settings
    #[serde(default)]
    pub buffer: BufferConfig,
    /// Logging settings
    #[serde(default)]
    pub logging: LoggingConfig,
    /// File this config was loaded from (used by the `reload_config` hub
    /// command). Not part of the YAML.
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
}

impl EdgeConfig {
    /// Load configuration from a YAML file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content =
            std::fs::read_to_string(path.as_ref()).map_err(|e| ConfigError::Io(e.to_string()))?;
        let mut config = Self::from_yaml(&content)?;
        config.source_path = Some(path.as_ref().to_path_buf());
        Ok(config)
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
        if self.node.hub_address.trim().is_empty() {
            return Err(ConfigError::Validation(
                "node.hub_address cannot be empty".into(),
            ));
        }
        if self.node.reconnect_interval_secs == 0 {
            return Err(ConfigError::Validation(
                "node.reconnect_interval_secs must be > 0".into(),
            ));
        }
        if self.node.heartbeat_interval_secs == 0 {
            return Err(ConfigError::Validation(
                "node.heartbeat_interval_secs must be > 0".into(),
            ));
        }
        let p = &self.prediction;
        if !(p.temperature_warning.is_finite() && p.temperature_critical.is_finite())
            || p.temperature_warning >= p.temperature_critical
        {
            return Err(ConfigError::Validation(format!(
                "prediction.temperature_warning ({}) must be below temperature_critical ({})",
                p.temperature_warning, p.temperature_critical
            )));
        }
        if !(p.ewma_min_std.is_finite() && p.ewma_min_std > 0.0) {
            return Err(ConfigError::Validation(
                "prediction.ewma_min_std must be > 0".into(),
            ));
        }
        if p.trend_window_minutes == 0 {
            return Err(ConfigError::Validation(
                "prediction.trend_window_minutes must be > 0".into(),
            ));
        }
        for name in &self.action.allowed_scripts {
            validate_script_name(name.trim()).map_err(|e| {
                ConfigError::Validation(format!(
                    "action.allowed_scripts entry '{name}' is invalid: {e}"
                ))
            })?;
        }
        Ok(())
    }
}

/// A script name must be a plain file name inside `action.scripts_dir`:
/// exactly one normal path component, no separators, no `:` (drive-relative
/// paths like `C:x.ps1` and NTFS alternate data streams like `x.ps1:s`).
pub fn validate_script_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("empty name".into());
    }
    if name.contains(['/', '\\', ':']) || name.chars().any(char::is_control) {
        return Err(format!(
            "'{name}' must be a plain file name (no path separators or ':')"
        ));
    }
    let mut parts = Path::new(name).components();
    match (parts.next(), parts.next()) {
        (Some(std::path::Component::Normal(_)), None) => Ok(()),
        _ => Err(format!("'{name}' must be a plain file name")),
    }
}

/// Node identification and connection settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Unique identifier for this edge node
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Hub server address: `host:port`, or a full `ws://` / `wss://` URL
    /// (the `/ws` path is appended when no path is given)
    pub hub_address: String,
    /// Initial reconnect delay (seconds); doubles per failed attempt
    #[serde(default = "default_reconnect_interval")]
    pub reconnect_interval_secs: u64,
    /// Upper bound for the reconnect backoff (seconds)
    #[serde(default = "default_max_reconnect_interval")]
    pub max_reconnect_interval_secs: u64,
    /// Interval between heartbeats sent to the hub (seconds)
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_secs: u64,
    /// Interval between WebSocket ping frames (seconds, 0 = off). The
    /// connection is considered dead when nothing arrives for 3 intervals.
    #[serde(default = "default_ping_interval")]
    pub ping_interval_secs: u64,
    /// Bearer token sent in the WebSocket handshake
    /// (`Authorization: Bearer <token>`); `NIMON_EDGE_TOKEN` overrides it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_token: Option<String>,
    /// Use TLS (`wss://`) even when `hub_address` has no scheme
    #[serde(default)]
    pub tls: bool,
}

fn default_reconnect_interval() -> u64 {
    5
}

fn default_max_reconnect_interval() -> u64 {
    60
}

fn default_heartbeat_interval() -> u64 {
    30
}

fn default_ping_interval() -> u64 {
    30
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            id: "edge-01".to_string(),
            name: "Edge Node".to_string(),
            hub_address: "127.0.0.1:9090".to_string(),
            reconnect_interval_secs: default_reconnect_interval(),
            max_reconnect_interval_secs: default_max_reconnect_interval(),
            heartbeat_interval_secs: default_heartbeat_interval(),
            ping_interval_secs: default_ping_interval(),
            hub_token: None,
            tls: false,
        }
    }
}

impl NodeConfig {
    /// Node settings with defaults for everything but identity and hub
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        hub_address: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            hub_address: hub_address.into(),
            ..Self::default()
        }
    }

    /// WebSocket URL of the hub (`ws://host:port/ws` or `wss://...`)
    pub fn hub_url(&self) -> String {
        let addr = self.hub_address.trim();
        let (scheme, rest) = if let Some(rest) = addr.strip_prefix("wss://") {
            ("wss", rest)
        } else if let Some(rest) = addr.strip_prefix("ws://") {
            (if self.tls { "wss" } else { "ws" }, rest)
        } else {
            (if self.tls { "wss" } else { "ws" }, addr)
        };
        let rest = rest.trim_end_matches('/');
        if rest.contains('/') {
            format!("{scheme}://{rest}")
        } else {
            format!("{scheme}://{rest}/ws")
        }
    }

    /// Hub token: `NIMON_EDGE_TOKEN` when set and non-empty, else
    /// `hub_token`
    pub fn resolved_hub_token(&self) -> Option<String> {
        Self::pick_token(std::env::var(HUB_TOKEN_ENV).ok(), self.hub_token.clone())
    }

    fn pick_token(env: Option<String>, configured: Option<String>) -> Option<String> {
        env.map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .or_else(|| {
                configured
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
            })
    }
}

/// API-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    /// NI-SysCfg discovery + health sweep
    #[serde(default)]
    pub syscfg: ApiSettings,
    /// NI-DAQmx (device reset actions)
    #[serde(default)]
    pub daqmx: DaqmxSettings,
    /// NI-VISA instrument discovery
    #[serde(default)]
    pub visa: VisaSettings,
    /// Consecutive sweeps a device must be missing before it is reported
    /// as removed (avoids flapping)
    #[serde(default = "default_removal_sweeps")]
    pub removal_sweeps: u32,
}

fn default_removal_sweeps() -> u32 {
    3
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            syscfg: ApiSettings::default(),
            daqmx: DaqmxSettings::default(),
            visa: VisaSettings::default(),
            removal_sweeps: default_removal_sweeps(),
        }
    }
}

/// NI-SysCfg settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSettings {
    /// Whether this API is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Sweep interval in seconds (also the edge's base polling tick)
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

/// NI-DAQmx settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaqmxSettings {
    /// Allow DAQmx device resets (PowerCycle / ResetDriver actions)
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl Default for DaqmxSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// NI-VISA settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisaSettings {
    /// Periodically list VISA resources and report them as devices
    #[serde(default)]
    pub enabled: bool,
    /// Discovery interval in seconds
    #[serde(default = "default_visa_interval")]
    pub poll_interval_secs: u64,
    /// Send `*IDN?` to every resource. This WRITES to instruments and
    /// serial ports, so it is off by default.
    #[serde(default)]
    pub probe_idn: bool,
    /// VISA resource expression passed to viFindRsrc
    #[serde(default = "default_visa_expression")]
    pub expression: String,
}

fn default_visa_interval() -> u64 {
    60
}

fn default_visa_expression() -> String {
    "?*INSTR".to_string()
}

impl Default for VisaSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            poll_interval_secs: default_visa_interval(),
            probe_idn: false,
            expression: default_visa_expression(),
        }
    }
}

/// Prediction engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionConfig {
    /// Whether prediction is enabled
    #[serde(default = "default_prediction_enabled")]
    pub enabled: bool,
    /// Models to run: `threshold`, `ewma_anomaly`, `trend_prediction`
    #[serde(default = "default_models")]
    pub models: Vec<String>,
    /// Threshold for temperature warning (C)
    #[serde(default = "default_temp_warning")]
    pub temperature_warning: f64,
    /// Threshold for temperature critical (C)
    #[serde(default = "default_temp_critical")]
    pub temperature_critical: f64,
    /// EWMA: minimum standard deviation (C) used for z-scores, so
    /// quantized, perfectly flat readings don't make tiny changes look
    /// like huge anomalies
    #[serde(default = "default_ewma_min_std")]
    pub ewma_min_std: f64,
    /// Trend: while a rising trend persists, re-send the prediction at
    /// most this often (minutes) unless the ETA changes by more than 20%
    #[serde(default = "default_trend_reemit_minutes")]
    pub trend_reemit_minutes: u64,
    /// Trend: regression window (minutes of history). Time based, so the
    /// sensitivity does not depend on the poll interval; no trend is
    /// reported before half a window of history exists.
    #[serde(default = "default_trend_window_minutes")]
    pub trend_window_minutes: u64,
}

fn default_trend_window_minutes() -> u64 {
    5
}

fn default_prediction_enabled() -> bool {
    true
}

fn default_models() -> Vec<String> {
    vec![
        "threshold".to_string(),
        "ewma_anomaly".to_string(),
        "trend_prediction".to_string(),
    ]
}

fn default_temp_warning() -> f64 {
    DEFAULT_TEMP_WARNING_C
}

fn default_temp_critical() -> f64 {
    DEFAULT_TEMP_CRITICAL_C
}

fn default_ewma_min_std() -> f64 {
    0.5
}

fn default_trend_reemit_minutes() -> u64 {
    10
}

impl Default for PredictionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            models: default_models(),
            temperature_warning: default_temp_warning(),
            temperature_critical: default_temp_critical(),
            ewma_min_std: default_ewma_min_std(),
            trend_reemit_minutes: default_trend_reemit_minutes(),
            trend_window_minutes: default_trend_window_minutes(),
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
    /// Timeout for any single action (script, service restart, reset)
    #[serde(default = "default_script_timeout")]
    pub timeout_secs: u64,
    /// OS services the hub may restart (empty = service restarts denied)
    #[serde(default)]
    pub allowed_services: Vec<String>,
    /// Maximum number of actions executing at the same time
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
}

fn default_scripts_dir() -> String {
    "./scripts".to_string()
}

fn default_script_timeout() -> u64 {
    120
}

fn default_max_concurrent() -> usize {
    1
}

impl Default for ActionConfig {
    fn default() -> Self {
        Self {
            allowed_scripts: Vec::new(),
            scripts_dir: default_scripts_dir(),
            timeout_secs: default_script_timeout(),
            allowed_services: Vec::new(),
            max_concurrent: default_max_concurrent(),
        }
    }
}

/// In-memory buffering while the hub is unreachable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferConfig {
    /// Buffer messages while disconnected (false = drop them)
    #[serde(default = "default_buffer_enabled")]
    pub enabled: bool,
    /// Maximum number of buffered messages
    #[serde(default = "default_buffer_messages")]
    pub max_messages: usize,
    /// Maximum buffered payload size in MB
    #[serde(default = "default_buffer_size")]
    pub max_size_mb: u64,
}

fn default_buffer_enabled() -> bool {
    true
}

fn default_buffer_messages() -> usize {
    1000
}

fn default_buffer_size() -> u64 {
    16
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_messages: default_buffer_messages(),
            max_size_mb: default_buffer_size(),
        }
    }
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level or EnvFilter directive (`RUST_LOG` overrides)
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Log file path (rotated daily, 7 files kept); empty = no file
    #[serde(default = "default_log_file")]
    pub file: String,
    /// Also log to stdout
    #[serde(default = "default_log_stdout")]
    pub stdout: bool,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_file() -> String {
    "./logs/nimon-edge.log".to_string()
}

fn default_log_stdout() -> bool {
    true
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file: default_log_file(),
            stdout: default_log_stdout(),
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
        assert_eq!(config.node.max_reconnect_interval_secs, 60);
        assert_eq!(config.node.heartbeat_interval_secs, 30);
        assert_eq!(config.node.ping_interval_secs, 30);
        assert!(!config.node.tls);
        assert!(config.prediction.enabled);
        assert!(config.buffer.enabled);
        assert_eq!(config.buffer.max_messages, 1000);
        assert!(!config.api.visa.enabled);
        assert!(!config.api.visa.probe_idn);
        assert!(config.api.daqmx.enabled);
        assert_eq!(config.api.removal_sweeps, 3);
        assert!(config.action.allowed_services.is_empty());
        assert_eq!(config.action.max_concurrent, 1);
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
        assert_eq!(config.prediction.temperature_warning, 65.0);
        assert_eq!(config.prediction.temperature_critical, 75.0);
        assert_eq!(config.prediction.ewma_min_std, 0.5);
        assert_eq!(config.prediction.models.len(), 3);
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
        assert!(result.unwrap_err().to_string().contains("hub_address"));
    }

    #[test]
    fn test_validation_threshold_order() {
        let yaml = r#"
node: { id: a, name: b, hub_address: "c:1" }
prediction:
  temperature_warning: 80
  temperature_critical: 70
"#;
        let err = EdgeConfig::from_yaml(yaml).unwrap_err().to_string();
        assert!(err.contains("temperature_warning"), "{err}");
    }

    #[test]
    fn test_full_config_and_legacy_keys_tolerated() {
        // api.xnet, api.daqmx.poll_interval_secs and buffer.persist_path
        // are no longer used but must still parse
        let yaml = r#"
node:
  id: test-edge-01
  name: Test Edge
  hub_address: "192.168.1.100:8080"
  reconnect_interval_secs: 10
  hub_token: "abc"
  tls: true

api:
  syscfg:
    enabled: true
    poll_interval_secs: 10
  daqmx:
    enabled: false
    poll_interval_secs: 5
  visa:
    enabled: true
    poll_interval_secs: 30
    probe_idn: true
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
        assert_eq!(config.node.hub_token.as_deref(), Some("abc"));
        assert!(config.node.tls);
        assert!(!config.api.daqmx.enabled);
        assert!(config.api.visa.enabled);
        assert!(config.api.visa.probe_idn);
        assert_eq!(config.api.visa.expression, "?*INSTR");
        assert_eq!(config.prediction.models.len(), 3);
        assert_eq!(config.buffer.max_size_mb, 200);
        assert_eq!(config.logging.level, "debug");
    }

    #[test]
    fn test_hub_url() {
        let mut node = NodeConfig::new("a", "b", "10.0.0.1:8080");
        assert_eq!(node.hub_url(), "ws://10.0.0.1:8080/ws");
        node.tls = true;
        assert_eq!(node.hub_url(), "wss://10.0.0.1:8080/ws");
        node.tls = false;
        node.hub_address = "wss://hub.example.com".into();
        assert_eq!(node.hub_url(), "wss://hub.example.com/ws");
        node.hub_address = "ws://hub:9/custom/path/".into();
        assert_eq!(node.hub_url(), "ws://hub:9/custom/path");
        node.tls = true;
        assert_eq!(node.hub_url(), "wss://hub:9/custom/path");
    }

    #[test]
    fn test_token_precedence() {
        assert_eq!(
            NodeConfig::pick_token(Some("env".into()), Some("cfg".into())),
            Some("env".to_string())
        );
        assert_eq!(
            NodeConfig::pick_token(Some("  ".into()), Some(" cfg ".into())),
            Some("cfg".to_string())
        );
        assert_eq!(NodeConfig::pick_token(None, Some("".into())), None);
        assert_eq!(NodeConfig::pick_token(None, None), None);
    }

    #[test]
    fn test_serialize_config() {
        let config = EdgeConfig {
            node: NodeConfig::new("test", "Test", "localhost:8080"),
            ..EdgeConfig::default()
        };
        let yaml = config.to_yaml().unwrap();
        assert!(yaml.contains("id: test"));
        assert!(yaml.contains("hub_address: localhost:8080"));
        assert!(!yaml.contains("hub_token"));
        // round trip
        let parsed = EdgeConfig::from_yaml(&yaml).unwrap();
        assert_eq!(parsed.node.id, "test");
    }

    #[test]
    fn test_from_file_records_source_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edge.yaml");
        std::fs::write(&path, "node: { id: a, name: b, hub_address: \"c:1\" }\n").unwrap();
        let config = EdgeConfig::from_file(&path).unwrap();
        assert_eq!(config.source_path.as_deref(), Some(path.as_path()));
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
  allowed_services: [nidevldu]
  max_concurrent: 2
"#;
        let config = EdgeConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.action.allowed_scripts.len(), 2);
        assert_eq!(config.action.scripts_dir, "/opt/nimon/scripts");
        assert_eq!(config.action.timeout_secs, 60);
        assert_eq!(config.action.allowed_services, vec!["nidevldu"]);
        assert_eq!(config.action.max_concurrent, 2);
    }

    #[test]
    fn test_invalid_allowlisted_script_rejected_at_load() {
        for bad in ["C:evil.ps1", "x.ps1:stream", "..\\up.ps1", "sub/x.sh", ".."] {
            let yaml = format!(
                "node: {{ id: a, name: b, hub_address: \"c:1\" }}\naction:\n  allowed_scripts: ['{bad}']\n"
            );
            let err = EdgeConfig::from_yaml(&yaml).unwrap_err().to_string();
            assert!(err.contains("action.allowed_scripts"), "{bad}: {err}");
        }
        let ok = "node: { id: a, name: b, hub_address: \"c:1\" }\naction:\n  allowed_scripts: [reset.ps1]\n";
        assert!(EdgeConfig::from_yaml(ok).is_ok());
    }

    #[test]
    fn test_trend_window_default_and_validation() {
        let base = "node: { id: a, name: b, hub_address: \"c:1\" }\n";
        assert_eq!(
            EdgeConfig::from_yaml(base)
                .unwrap()
                .prediction
                .trend_window_minutes,
            5
        );
        let zero = format!("{base}prediction:\n  trend_window_minutes: 0\n");
        assert!(EdgeConfig::from_yaml(&zero).is_err());
    }

    #[test]
    fn test_example_configs_parse() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
        for name in ["edge.example.yaml", "edge.yaml"] {
            let path = root.join(name);
            if path.exists() {
                EdgeConfig::from_file(&path)
                    .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));
            }
        }
    }
}
