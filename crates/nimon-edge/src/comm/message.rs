//! WebSocket message protocol for edge-hub communication
//!
//! Re-exports shared protocol types from nimon-core.

pub use nimon_core::protocol::*;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nimon_core::actor::messages::DeviceStatusUpdate;
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
            is_simulated: false,
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
