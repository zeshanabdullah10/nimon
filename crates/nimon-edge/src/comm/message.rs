//! WebSocket message protocol for edge-hub communication
//!
//! Messages are serialized as JSON and sent over WebSocket connections.
//! Each message has a type and payload.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use nimon_core::actor::messages::{
    DeviceAlert, DeviceStatusUpdate, EdgeHeartbeat, EdgeRegister, PredictionResult,
};

/// Protocol version for compatibility
pub const PROTOCOL_VERSION: &str = "1.0";

/// WebSocket message wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    /// Protocol version
    pub version: String,
    /// Message type
    #[serde(rename = "type")]
    pub msg_type: WsMessageType,
    /// Message payload
    pub payload: serde_json::Value,
    /// Message timestamp
    pub timestamp: DateTime<Utc>,
    /// Unique message ID for tracking
    pub msg_id: String,
}

/// Message types that can be sent over WebSocket
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsMessageType {
    // Edge → Hub messages
    EdgeRegister,
    DeviceStatus,
    DeviceAlert,
    Prediction,
    Heartbeat,

    // Hub → Edge messages
    ConfigUpdate,
    ExecuteAction,
    HubCommand,

    // Control messages
    Ack,
    Error,
    Ping,
    Pong,
}

/// Acknowledgment message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckMessage {
    pub original_msg_id: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Error message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMessage {
    pub code: String,
    pub message: String,
    pub details: Option<String>,
}

/// Ping message for connection health check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingMessage {
    pub timestamp: DateTime<Utc>,
}

/// Pong response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PongMessage {
    pub ping_timestamp: DateTime<Utc>,
    pub pong_timestamp: DateTime<Utc>,
}

impl WsMessage {
    /// Create a new WebSocket message
    pub fn new(msg_type: WsMessageType, payload: serde_json::Value) -> Self {
        Self {
            version: PROTOCOL_VERSION.to_string(),
            msg_type,
            payload,
            timestamp: Utc::now(),
            msg_id: ulid::Ulid::new().to_string(),
        }
    }

    /// Create an edge registration message
    pub fn edge_register(registration: EdgeRegister) -> Self {
        let payload = serde_json::to_value(registration).unwrap();
        Self::new(WsMessageType::EdgeRegister, payload)
    }

    /// Create a device status message
    pub fn device_status(status: DeviceStatusUpdate) -> Self {
        let payload = serde_json::to_value(status).unwrap();
        Self::new(WsMessageType::DeviceStatus, payload)
    }

    /// Create a device alert message
    pub fn device_alert(alert: DeviceAlert) -> Self {
        let payload = serde_json::to_value(alert).unwrap();
        Self::new(WsMessageType::DeviceAlert, payload)
    }

    /// Create a prediction message
    pub fn prediction(prediction: PredictionResult) -> Self {
        let payload = serde_json::to_value(prediction).unwrap();
        Self::new(WsMessageType::Prediction, payload)
    }

    /// Create a heartbeat message
    pub fn heartbeat(heartbeat: EdgeHeartbeat) -> Self {
        let payload = serde_json::to_value(heartbeat).unwrap();
        Self::new(WsMessageType::Heartbeat, payload)
    }

    /// Create an acknowledgment message
    pub fn ack(original_msg_id: String, success: bool, error: Option<String>) -> Self {
        let payload = serde_json::to_value(AckMessage {
            original_msg_id,
            success,
            error,
        })
        .unwrap();
        Self::new(WsMessageType::Ack, payload)
    }

    /// Create an error message
    pub fn error(code: String, message: String, details: Option<String>) -> Self {
        let payload = serde_json::to_value(ErrorMessage {
            code,
            message,
            details,
        })
        .unwrap();
        Self::new(WsMessageType::Error, payload)
    }

    /// Create a ping message
    pub fn ping() -> Self {
        let payload = serde_json::to_value(PingMessage { timestamp: Utc::now() }).unwrap();
        Self::new(WsMessageType::Ping, payload)
    }

    /// Create a pong message
    pub fn pong(ping_timestamp: DateTime<Utc>) -> Self {
        let payload = serde_json::to_value(PongMessage {
            ping_timestamp,
            pong_timestamp: Utc::now(),
        })
        .unwrap();
        Self::new(WsMessageType::Pong, payload)
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Extract payload as specific type
    pub fn payload<T: for<'de> Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.payload.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_core::{HealthStatus, MetricValue};
    use std::collections::HashMap;

    #[test]
    fn test_ws_message_serialization() {
        let msg = WsMessage::ping();
        let json = msg.to_json().unwrap();
        assert!(json.contains("ping"));
        assert!(json.contains(PROTOCOL_VERSION));

        let parsed = WsMessage::from_json(&json).unwrap();
        assert_eq!(parsed.msg_type, WsMessageType::Ping);
    }

    #[test]
    fn test_device_status_message() {
        let mut metrics = HashMap::new();
        metrics.insert("temperature".to_string(), MetricValue::Float(45.5));

        let status = DeviceStatusUpdate {
            device_id: "device-1".to_string(),
            edge_id: "edge-1".to_string(),
            status: HealthStatus::Healthy,
            metrics,
            timestamp: Utc::now(),
        };

        let ws_msg = WsMessage::device_status(status);
        let json = ws_msg.to_json().unwrap();
        assert!(json.contains("device_status"));

        let parsed = WsMessage::from_json(&json).unwrap();
        assert_eq!(parsed.msg_type, WsMessageType::DeviceStatus);
    }

    #[test]
    fn test_ack_message() {
        let msg = WsMessage::ack("msg-123".to_string(), true, None);
        let json = msg.to_json().unwrap();

        let parsed = WsMessage::from_json(&json).unwrap();
        assert_eq!(parsed.msg_type, WsMessageType::Ack);

        let ack: AckMessage = parsed.payload().unwrap();
        assert_eq!(ack.original_msg_id, "msg-123");
        assert!(ack.success);
    }

    #[test]
    fn test_error_message() {
        let msg = WsMessage::error(
            "E001".to_string(),
            "Connection failed".to_string(),
            Some("Timeout".to_string()),
        );
        let json = msg.to_json().unwrap();

        let parsed = WsMessage::from_json(&json).unwrap();
        assert_eq!(parsed.msg_type, WsMessageType::Error);

        let err: ErrorMessage = parsed.payload().unwrap();
        assert_eq!(err.code, "E001");
        assert_eq!(err.message, "Connection failed");
    }

    #[test]
    fn test_message_has_unique_id() {
        let msg1 = WsMessage::ping();
        let msg2 = WsMessage::ping();
        assert_ne!(msg1.msg_id, msg2.msg_id);
    }
}
