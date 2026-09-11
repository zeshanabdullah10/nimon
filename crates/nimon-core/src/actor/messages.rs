//! Actor message definitions
//!
//! This module contains all the message types used for communication
//! between actors in the NIMon system.

use actix::Message;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{HealthStatus, MetricValue, PredictionType, Severity};

// ============================================================================
// Device Messages
// ============================================================================

/// Poll a device for current status
#[derive(Debug, Clone, Message)]
#[rtype(result = "DevicePollResult")]
pub struct DevicePoll {
    /// Device ID to poll
    pub device_id: String,
    /// Force a refresh even if cached
    pub force: bool,
}

/// Result of a device poll
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicePollResult {
    /// Whether the poll was successful
    pub success: bool,
    /// Current health status
    pub status: HealthStatus,
    /// Collected metrics
    pub metrics: HashMap<String, MetricValue>,
    /// Error message if poll failed
    pub error: Option<String>,
    /// Timestamp of the poll
    pub timestamp: DateTime<Utc>,
}

/// Device status update (broadcast)
#[derive(Debug, Clone, Message, Serialize, Deserialize)]
#[rtype(result = "()")]
pub struct DeviceStatusUpdate {
    /// Device ID
    pub device_id: String,
    /// Edge node ID
    pub edge_id: String,
    /// Current health status
    pub status: HealthStatus,
    /// Current metrics
    pub metrics: HashMap<String, MetricValue>,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// True when this update originates from a simulated device
    #[serde(default)]
    pub is_simulated: bool,
}

/// Device alert notification
#[derive(Debug, Clone, Message, Serialize, Deserialize)]
#[rtype(result = "()")]
pub struct DeviceAlert {
    /// Device ID
    pub device_id: String,
    /// Edge node ID
    pub edge_id: String,
    /// Alert severity
    pub severity: Severity,
    /// Alert message
    pub message: String,
    /// Related metric (if any)
    pub metric_name: Option<String>,
    /// Metric value that triggered alert
    pub metric_value: Option<f64>,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

// ============================================================================
// Hub Communication Messages
// ============================================================================

/// Register this edge node with the hub
#[derive(Debug, Clone, Message, Serialize, Deserialize)]
#[rtype(result = "()")]
pub struct EdgeRegister {
    /// Edge node ID
    pub edge_id: String,
    /// Edge node name
    pub name: String,
    /// Edge node hostname
    pub hostname: Option<String>,
    /// Edge node IP address
    pub ip_address: Option<String>,
}

/// Heartbeat message to hub
#[derive(Debug, Clone, Message, Serialize, Deserialize)]
#[rtype(result = "()")]
pub struct EdgeHeartbeat {
    /// Edge node ID
    pub edge_id: String,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Number of devices being monitored
    pub device_count: usize,
    /// Hub connector status
    pub status: String,
}

/// Configuration update from hub
#[derive(Debug, Clone, Default, Message, Serialize, Deserialize)]
#[rtype(result = "()")]
pub struct ConfigUpdate {
    /// Polling interval in seconds applied to the edge sweep
    #[serde(default)]
    pub poll_interval_secs: Option<u64>,
    /// Temperature thresholds
    #[serde(default)]
    pub thresholds: ThresholdConfig,
}

/// Threshold configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdConfig {
    /// Temperature warning threshold (C)
    pub temperature_warning: f64,
    /// Temperature critical threshold (C)
    pub temperature_critical: f64,
}

impl Default for ThresholdConfig {
    fn default() -> Self {
        Self {
            temperature_warning: 65.0,
            temperature_critical: 75.0,
        }
    }
}

/// Execute a remote action on an edge node
#[derive(Debug, Clone, Message, Serialize, Deserialize)]
#[rtype(result = "ActionResult")]
pub struct ExecuteAction {
    /// Action ID for tracking
    pub action_id: String,
    /// Target device ID
    pub device_id: String,
    /// Action type
    pub action_type: ActionType,
    /// Action parameters
    pub parameters: HashMap<String, String>,
}

/// Types of actions that can be executed (wire + cross-actor family)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    /// Power cycle the device
    PowerCycle,
    /// Restart OS services on the edge host
    RestartServices,
    /// Reset the driver session
    ResetDriver,
    /// Run a custom script (must be allowlisted edge-side)
    CustomScript,
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionType::PowerCycle => write!(f, "power_cycle"),
            ActionType::RestartServices => write!(f, "restart_services"),
            ActionType::ResetDriver => write!(f, "reset_driver"),
            ActionType::CustomScript => write!(f, "custom_script"),
        }
    }
}

/// Result of an action execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    /// Action ID
    pub action_id: String,
    /// Whether the action succeeded
    pub success: bool,
    /// Output from the action
    pub output: Option<String>,
    /// Error message if failed
    pub error: Option<String>,
    /// Process exit code (when a process ran)
    pub exit_code: Option<i32>,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

// ============================================================================
// Prediction Messages
// ============================================================================

/// Prediction result from the prediction engine
#[derive(Debug, Clone, Message, Serialize, Deserialize)]
#[rtype(result = "()")]
pub struct PredictionResult {
    /// Device ID
    pub device_id: String,
    /// Edge node ID
    pub edge_id: String,
    /// Prediction type
    pub prediction_type: PredictionType,
    /// Probability (0.0 to 1.0)
    pub probability: f64,
    /// Estimated time to event (minutes)
    pub eta_minutes: Option<i32>,
    /// Confidence level
    pub confidence: f64,
    /// Human-readable reason for the prediction
    #[serde(default)]
    pub reason: Option<String>,
    /// Model version identifier
    #[serde(default)]
    pub model_version: Option<String>,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_status_update_serialization() {
        let mut metrics = HashMap::new();
        metrics.insert("temperature".to_string(), MetricValue::Float(45.5));

        let update = DeviceStatusUpdate {
            device_id: "device-1".to_string(),
            edge_id: "edge-1".to_string(),
            status: HealthStatus::Healthy,
            metrics,
            timestamp: Utc::now(),
            is_simulated: false,
        };

        let json = serde_json::to_string(&update).unwrap();
        assert!(json.contains("device-1"));
        assert!(json.contains("healthy"));

        let parsed: DeviceStatusUpdate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.device_id, "device-1");
        assert!(!parsed.is_simulated);
    }

    #[test]
    fn test_status_update_without_simulated_flag_parses() {
        // Old edges do not send is_simulated; default must be false
        let json = r#"{"device_id":"d","edge_id":"e","status":"healthy","metrics":{},"timestamp":"2024-01-01T00:00:00Z"}"#;
        let parsed: DeviceStatusUpdate = serde_json::from_str(json).unwrap();
        assert!(!parsed.is_simulated);
    }

    #[test]
    fn test_config_update_default() {
        let config = ThresholdConfig::default();
        assert_eq!(config.temperature_warning, 65.0);
        assert_eq!(config.temperature_critical, 75.0);
    }

    #[test]
    fn test_action_type_serde() {
        let action = ActionType::PowerCycle;
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"power_cycle\"");
    }

    #[test]
    fn test_action_result_roundtrip() {
        let result = ActionResult {
            action_id: "a-1".to_string(),
            success: true,
            output: Some("done".to_string()),
            error: None,
            exit_code: Some(0),
            duration_ms: 42,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ActionResult = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.exit_code, Some(0));
    }
}
