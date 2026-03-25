//! Actor message definitions
//!
//! This module contains all the message types used for communication
//! between actors in the NIMon system.

use actix::Message;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{DeviceType, HealthStatus, MetricValue};

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
    pub severity: AlertSeverity,
    /// Alert message
    pub message: String,
    /// Related metric (if any)
    pub metric_name: Option<String>,
    /// Metric value that triggered alert
    pub metric_value: Option<f64>,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

/// Alert severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
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
#[derive(Debug, Clone, Message, Serialize, Deserialize)]
#[rtype(result = "()")]
pub struct ConfigUpdate {
    /// Polling intervals per device type
    pub poll_intervals: HashMap<DeviceType, u64>,
    /// Temperature thresholds
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

/// Types of actions that can be executed
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    /// Power cycle the device
    PowerCycle,
    /// Restart NI services
    RestartServices,
    /// Reset driver session
    ResetDriver,
    /// Run a custom script
    CustomScript,
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
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

/// Types of predictions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredictionType {
    Overheating,
    ConnectionFailure,
    PowerSupplyFailure,
    BusDegradation,
    FirmwareIssue,
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
        };

        let json = serde_json::to_string(&update).unwrap();
        assert!(json.contains("device-1"));
        assert!(json.contains("healthy"));

        let parsed: DeviceStatusUpdate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.device_id, "device-1");
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
}
