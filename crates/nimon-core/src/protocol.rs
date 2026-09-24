//! WebSocket message protocol for edge-hub communication
//!
//! Messages are serialized as JSON and sent over WebSocket connections.
//! Each message has a type and payload.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::actor::messages::{
    ActionResult, ConfigUpdate, DeviceAlert, DeviceRemoved, DeviceStatusUpdate, EdgeHeartbeat,
    EdgeRegister, ExecuteAction, PredictionResult,
};

/// Protocol version for compatibility.
///
/// 1.1 (additive over 1.0): `reply_to` envelope field, `device_removed`
/// message type, partial `ThresholdConfig`, optional heartbeat fields.
pub const PROTOCOL_VERSION: &str = "1.1";

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
    /// For replies: the `msg_id` of the request this message answers
    /// (e.g. an `action_result` answering an `execute_action`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

/// Message types that can be sent over WebSocket
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsMessageType {
    // Edge -> Hub messages
    EdgeRegister,
    DeviceStatus,
    DeviceAlert,
    Prediction,
    Heartbeat,
    ActionResult,
    /// A device disappeared from discovery (payload: `DeviceRemoved`)
    DeviceRemoved,

    // Hub -> Edge messages
    ConfigUpdate,
    ExecuteAction,
    HubCommand,

    // Control messages
    Ack,
    Error,
    Ping,
    Pong,

    /// Any type this build does not know (sent by a newer peer). Parsing
    /// succeeds so the receiver can ignore/log it instead of failing.
    /// Must stay the last variant.
    #[serde(other)]
    Unknown,
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

/// A generic command from the hub to an edge node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubCommand {
    /// Command name (e.g. "resend_state", "reload_config")
    pub command: String,
    /// Command parameters
    #[serde(default)]
    pub parameters: HashMap<String, String>,
}

/// Check whether a peer's protocol version is compatible with ours:
/// same major version (additive minor changes are tolerated both ways).
pub fn is_compatible(version: &str) -> bool {
    let ours: u64 = PROTOCOL_VERSION
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    let theirs: u64 = version
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(u64::MAX);
    ours == theirs
}

/// Serialize a payload value infallibly. All payload types are plain
/// data (no non-string map keys, no cycles), so serialization cannot
/// fail; a failure would be a programming error and degrades to `Null`
/// rather than panicking.
fn to_payload<T: Serialize>(value: T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
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
            reply_to: None,
        }
    }

    /// Mark this message as a reply to the request with `msg_id == id`.
    pub fn with_reply_to(mut self, id: impl Into<String>) -> Self {
        self.reply_to = Some(id.into());
        self
    }

    /// The id to correlate this message with a request: `reply_to` when
    /// set, otherwise this message's own `msg_id`.
    pub fn correlation_id(&self) -> &str {
        self.reply_to.as_deref().unwrap_or(&self.msg_id)
    }

    /// True when the sender's protocol version is compatible with ours.
    pub fn version_compatible(&self) -> bool {
        is_compatible(&self.version)
    }

    /// Create an edge registration message
    pub fn edge_register(registration: EdgeRegister) -> Self {
        Self::new(WsMessageType::EdgeRegister, to_payload(registration))
    }

    /// Create a device status message
    pub fn device_status(status: DeviceStatusUpdate) -> Self {
        Self::new(WsMessageType::DeviceStatus, to_payload(status))
    }

    /// Create a device alert message
    pub fn device_alert(alert: DeviceAlert) -> Self {
        Self::new(WsMessageType::DeviceAlert, to_payload(alert))
    }

    /// Create a prediction message
    pub fn prediction(prediction: PredictionResult) -> Self {
        Self::new(WsMessageType::Prediction, to_payload(prediction))
    }

    /// Create a heartbeat message
    pub fn heartbeat(heartbeat: EdgeHeartbeat) -> Self {
        Self::new(WsMessageType::Heartbeat, to_payload(heartbeat))
    }

    /// Create an action result message (edge -> hub)
    pub fn action_result(result: ActionResult) -> Self {
        Self::new(WsMessageType::ActionResult, to_payload(result))
    }

    /// Create a device removed message (edge -> hub)
    pub fn device_removed(removed: DeviceRemoved) -> Self {
        Self::new(WsMessageType::DeviceRemoved, to_payload(removed))
    }

    /// Create a config update message (hub -> edge)
    pub fn config_update(update: ConfigUpdate) -> Self {
        Self::new(WsMessageType::ConfigUpdate, to_payload(update))
    }

    /// Create an execute action message (hub -> edge)
    pub fn execute_action(action: ExecuteAction) -> Self {
        Self::new(WsMessageType::ExecuteAction, to_payload(action))
    }

    /// Create a hub command message (hub -> edge)
    pub fn hub_command(command: HubCommand) -> Self {
        Self::new(WsMessageType::HubCommand, to_payload(command))
    }

    /// Create an acknowledgment message (also sets `reply_to` to
    /// `original_msg_id`)
    pub fn ack(original_msg_id: String, success: bool, error: Option<String>) -> Self {
        let reply_to = original_msg_id.clone();
        Self::new(
            WsMessageType::Ack,
            to_payload(AckMessage {
                original_msg_id,
                success,
                error,
            }),
        )
        .with_reply_to(reply_to)
    }

    /// Create an error message
    pub fn error(code: String, message: String, details: Option<String>) -> Self {
        Self::new(
            WsMessageType::Error,
            to_payload(ErrorMessage {
                code,
                message,
                details,
            }),
        )
    }

    /// Create a ping message
    pub fn ping() -> Self {
        Self::new(
            WsMessageType::Ping,
            to_payload(PingMessage {
                timestamp: Utc::now(),
            }),
        )
    }

    /// Create a pong message
    pub fn pong(ping_timestamp: DateTime<Utc>) -> Self {
        Self::new(
            WsMessageType::Pong,
            to_payload(PongMessage {
                ping_timestamp,
                pong_timestamp: Utc::now(),
            }),
        )
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Extract payload as specific type (clones the payload; prefer
    /// [`WsMessage::into_payload`] when the message is no longer needed)
    pub fn payload<T: for<'de> Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        T::deserialize(&self.payload)
    }

    /// Consume the message and extract its payload without cloning
    pub fn into_payload<T: for<'de> Deserialize<'de>>(self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::messages::ActionType;
    use crate::Severity;
    use chrono::TimeZone;

    #[test]
    fn test_version_compat_same_major() {
        assert!(is_compatible("1.0"));
        assert!(is_compatible("1.1"));
        assert!(is_compatible("1"));
        assert!(!is_compatible("2.0"));
        assert!(!is_compatible("0.9"));
        assert!(!is_compatible("garbage"));
    }

    #[test]
    fn test_message_version_compatible() {
        let mut msg = WsMessage::ping();
        assert!(msg.version_compatible());
        msg.version = "2.0".to_string();
        assert!(!msg.version_compatible());
    }

    #[test]
    fn test_action_result_message_roundtrip() {
        let result = ActionResult {
            action_id: "act-1".to_string(),
            success: true,
            output: Some("ok".to_string()),
            error: None,
            exit_code: Some(0),
            duration_ms: 10,
        };
        let msg = WsMessage::action_result(result);
        let json = msg.to_json().unwrap();
        assert!(json.contains("\"action_result\""));

        let parsed = WsMessage::from_json(&json).unwrap();
        assert_eq!(parsed.msg_type, WsMessageType::ActionResult);
        let payload: ActionResult = parsed.payload().unwrap();
        assert!(payload.success);
        assert_eq!(payload.action_id, "act-1");
    }

    #[test]
    fn test_execute_action_message_roundtrip() {
        let mut parameters = HashMap::new();
        parameters.insert("script".to_string(), "power_cycle_relays.sh".to_string());
        let action = ExecuteAction {
            action_id: "act-2".to_string(),
            device_id: "dev-1".to_string(),
            action_type: ActionType::PowerCycle,
            parameters,
        };
        let msg = WsMessage::execute_action(action);
        let json = msg.to_json().unwrap();
        assert!(json.contains("\"execute_action\""));

        let parsed = WsMessage::from_json(&json).unwrap();
        assert_eq!(parsed.msg_type, WsMessageType::ExecuteAction);
        let payload: ExecuteAction = parsed.payload().unwrap();
        assert_eq!(payload.action_type, ActionType::PowerCycle);
    }

    #[test]
    fn test_config_update_message_roundtrip() {
        let update = ConfigUpdate {
            poll_interval_secs: Some(30),
            thresholds: crate::actor::messages::ThresholdConfig::full(70.0, 80.0),
        };
        let msg = WsMessage::config_update(update);
        let parsed = WsMessage::from_json(&msg.to_json().unwrap()).unwrap();
        let payload: ConfigUpdate = parsed.payload().unwrap();
        assert_eq!(payload.poll_interval_secs, Some(30));
        assert_eq!(payload.thresholds.temperature_warning, Some(70.0));
    }

    #[test]
    fn test_reply_to_correlation() {
        let request = WsMessage::execute_action(ExecuteAction {
            action_id: "a".to_string(),
            device_id: "d".to_string(),
            action_type: ActionType::ResetDriver,
            parameters: HashMap::new(),
        });
        assert_eq!(request.reply_to, None);
        assert_eq!(request.correlation_id(), request.msg_id);
        // reply_to is omitted from the wire when unset
        assert!(!request.to_json().unwrap().contains("reply_to"));

        let reply = WsMessage::action_result(ActionResult {
            action_id: "a".to_string(),
            success: true,
            output: None,
            error: None,
            exit_code: None,
            duration_ms: 1,
        })
        .with_reply_to(request.msg_id.clone());
        assert_ne!(reply.msg_id, request.msg_id);
        let parsed = WsMessage::from_json(&reply.to_json().unwrap()).unwrap();
        assert_eq!(parsed.reply_to.as_deref(), Some(request.msg_id.as_str()));
        assert_eq!(parsed.correlation_id(), request.msg_id);

        // Acks carry reply_to too
        let ack = WsMessage::ack("orig".to_string(), true, None);
        assert_eq!(ack.correlation_id(), "orig");
    }

    #[test]
    fn test_v1_0_message_without_reply_to_parses() {
        let json = r#"{"version":"1.0","type":"ping","payload":null,
            "timestamp":"2024-01-01T00:00:00Z","msg_id":"01ABC"}"#;
        let msg = WsMessage::from_json(json).unwrap();
        assert_eq!(msg.reply_to, None);
        assert_eq!(msg.correlation_id(), "01ABC");
        assert!(msg.version_compatible());
    }

    #[test]
    fn test_unknown_message_type_parses() {
        let json = r#"{"version":"1.7","type":"some_future_type","payload":{"x":1},
            "timestamp":"2024-01-01T00:00:00Z","msg_id":"01ABC"}"#;
        let msg = WsMessage::from_json(json).unwrap();
        assert_eq!(msg.msg_type, WsMessageType::Unknown);
        assert!(msg.version_compatible());
        // Known types are unaffected
        assert_eq!(
            serde_json::from_str::<WsMessageType>("\"device_status\"").unwrap(),
            WsMessageType::DeviceStatus
        );
    }

    #[test]
    fn test_device_removed_message_roundtrip() {
        let ts = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let msg = WsMessage::device_removed(DeviceRemoved {
            edge_id: "e".to_string(),
            device_id: "e:Dev1".to_string(),
            timestamp: ts,
        });
        let json = msg.to_json().unwrap();
        assert!(json.contains("\"type\":\"device_removed\""));
        let parsed = WsMessage::from_json(&json).unwrap();
        assert_eq!(parsed.msg_type, WsMessageType::DeviceRemoved);
        let payload: DeviceRemoved = parsed.into_payload().unwrap();
        assert_eq!(payload.device_id, "e:Dev1");
        assert_eq!(payload.timestamp, ts);
    }

    #[test]
    fn test_into_payload_matches_payload() {
        let msg = WsMessage::hub_command(HubCommand {
            command: "reload_config".to_string(),
            parameters: HashMap::new(),
        });
        let a: HubCommand = msg.payload().unwrap();
        let b: HubCommand = msg.into_payload().unwrap();
        assert_eq!(a.command, b.command);
    }

    #[test]
    fn test_hub_command_message_roundtrip() {
        let mut parameters = HashMap::new();
        parameters.insert("reason".to_string(), "resync".to_string());
        let msg = WsMessage::hub_command(HubCommand {
            command: "resend_state".to_string(),
            parameters,
        });
        let parsed = WsMessage::from_json(&msg.to_json().unwrap()).unwrap();
        assert_eq!(parsed.msg_type, WsMessageType::HubCommand);
        let payload: HubCommand = parsed.payload().unwrap();
        assert_eq!(payload.command, "resend_state");
    }

    #[test]
    fn test_wire_format_golden_fixtures() {
        // Golden fixture: wire strings must stay stable across refactors.
        let ts = Utc.timestamp_opt(0, 0).unwrap();
        let msg = WsMessage::new(WsMessageType::Ping, serde_json::Value::Null);
        let json = msg.to_json().unwrap();
        assert!(json.contains("\"type\":\"ping\""));
        assert!(json.contains("\"version\":\"1.1\""));
        assert!(!json.contains("reply_to"));
        let json = msg.clone().with_reply_to("req-1").to_json().unwrap();
        assert!(json.contains("\"reply_to\":\"req-1\""));
        assert_eq!(
            serde_json::to_string(&WsMessageType::DeviceRemoved).unwrap(),
            "\"device_removed\""
        );
        assert_eq!(
            serde_json::to_string(&WsMessageType::ActionResult).unwrap(),
            "\"action_result\""
        );

        let alert = DeviceAlert {
            device_id: "d".to_string(),
            edge_id: "e".to_string(),
            severity: Severity::Critical,
            message: "m".to_string(),
            metric_name: None,
            metric_value: None,
            timestamp: ts,
        };
        let json = serde_json::to_string(&alert).unwrap();
        assert!(json.contains("\"severity\":\"critical\""));
    }
}
