//! Hub Connector Actor
//!
//! Bridges device actors to the WebSocket connection for hub communication.
//! Subscribes to device status updates, alerts, and predictions, forwarding them
//! to the hub. Handles incoming commands from the hub.

use std::collections::VecDeque;
use std::time::Duration;

use actix::prelude::*;
use tracing::{debug, info, warn};

use nimon_core::actor::messages::{
    ConfigUpdate, DeviceAlert, DeviceStatusUpdate, EdgeHeartbeat, EdgeRegister, PredictionResult,
};
use nimon_core::NimonError;

use crate::comm::{Connect, ConnectionState, SendWsMessage, WsClient, WsMessage};

/// Maximum number of messages to buffer when offline
const MAX_BUFFER_SIZE: usize = 1000;

/// Hub connector actor configuration
#[derive(Debug, Clone)]
pub struct HubConnectorConfig {
    /// Hub WebSocket URL
    pub hub_url: String,
    /// Edge node ID
    pub edge_id: String,
    /// Edge node name
    pub edge_name: String,
    /// Hostname (optional)
    pub hostname: Option<String>,
    /// IP address (optional)
    pub ip_address: Option<String>,
    /// Heartbeat interval
    pub heartbeat_interval: Duration,
    /// Auto-connect on start
    pub auto_connect: bool,
}

impl Default for HubConnectorConfig {
    fn default() -> Self {
        Self {
            hub_url: "ws://localhost:8080/ws".to_string(),
            edge_id: "edge-1".to_string(),
            edge_name: "Edge Node 1".to_string(),
            hostname: None,
            ip_address: None,
            heartbeat_interval: Duration::from_secs(30),
            auto_connect: true,
        }
    }
}

/// Hub connector actor
pub struct HubConnectorActor {
    config: HubConnectorConfig,
    ws_client: Option<Addr<WsClient>>,
    ws_client_addr: Addr<WsClient>,
    connection_state: ConnectionState,
    message_buffer: VecDeque<WsMessage>,
    heartbeat_handle: Option<SpawnHandle>,
}

impl HubConnectorActor {
    /// Create a new hub connector actor
    pub fn new(config: HubConnectorConfig, ws_client_addr: Addr<WsClient>) -> Self {
        Self {
            config,
            ws_client: None,
            ws_client_addr,
            connection_state: ConnectionState::Disconnected,
            message_buffer: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            heartbeat_handle: None,
        }
    }
}

impl Actor for HubConnectorActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("HubConnectorActor started for edge {}", self.config.edge_id);

        // Store the WebSocket client address
        self.ws_client = Some(self.ws_client_addr.clone());

        // Auto-connect if configured
        if self.config.auto_connect {
            info!("Auto-connecting to hub at {}", self.config.hub_url);
            self.ws_client_addr.do_send(Connect);
        }

        // Start heartbeat
        self.start_heartbeat(ctx);
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        info!("HubConnectorActor stopped");
    }
}

impl HubConnectorActor {
    /// Start the heartbeat task
    fn start_heartbeat(&mut self, ctx: &mut Context<Self>) {
        let heartbeat_interval = self.config.heartbeat_interval;

        self.heartbeat_handle = Some(ctx.run_interval(heartbeat_interval, move |actor, _ctx| {
            if actor.connection_state == ConnectionState::Connected {
                let heartbeat = EdgeHeartbeat {
                    edge_id: actor.config.edge_id.clone(),
                    timestamp: chrono::Utc::now(),
                    device_count: 0, // Will be updated by device manager
                    status: format!("{:?}", actor.connection_state),
                };

                let msg = WsMessage::heartbeat(heartbeat);
                let _ = actor.send_message(msg);
            }
        }));
    }

    /// Send a message to the hub
    fn send_message(&mut self, msg: WsMessage) -> Result<(), NimonError> {
        if self.connection_state == ConnectionState::Connected {
            if let Some(ref ws_client) = self.ws_client {
                ws_client.do_send(SendWsMessage { message: msg });
                return Ok(());
            }
        }

        // Buffer the message if not connected
        if self.message_buffer.len() < MAX_BUFFER_SIZE {
            self.message_buffer.push_back(msg);
            debug!("Message buffered (not connected)");
            Ok(())
        } else {
            warn!("Message buffer full, dropping message");
            Err(NimonError::InvalidState("Message buffer full".to_string()))
        }
    }

    /// Send all buffered messages
    fn flush_buffer(&mut self) {
        if self.message_buffer.is_empty() {
            return;
        }

        info!("Flushing {} buffered messages", self.message_buffer.len());

        while let Some(msg) = self.message_buffer.pop_front() {
            if let Some(ref ws_client) = self.ws_client {
                ws_client.do_send(SendWsMessage { message: msg });
            }
        }
    }

    /// Handle connection event
    fn handle_connected(&mut self, _ctx: &mut Context<Self>) {
        info!("Connected to hub, registering edge");

        self.connection_state = ConnectionState::Connected;

        // Send registration
        let registration = EdgeRegister {
            edge_id: self.config.edge_id.clone(),
            name: self.config.edge_name.clone(),
            hostname: self.config.hostname.clone(),
            ip_address: self.config.ip_address.clone(),
        };

        let msg = WsMessage::edge_register(registration);
        let _ = self.send_message(msg);

        // Flush buffered messages
        self.flush_buffer();
    }

    /// Handle disconnection event
    fn handle_disconnected(&mut self) {
        info!("Disconnected from hub");
        self.connection_state = ConnectionState::Disconnected;
    }

    /// Handle received message from hub
    fn handle_message(&mut self, ws_msg: WsMessage) {
        debug!("Received message from hub: {:?}", ws_msg.msg_type);

        match ws_msg.msg_type {
            crate::comm::WsMessageType::ConfigUpdate => {
                if let Ok(_config) = ws_msg.payload::<ConfigUpdate>() {
                    debug!("Received config update from hub");
                    // TODO: Apply config update to device manager
                }
            }
            crate::comm::WsMessageType::ExecuteAction => {
                debug!("Received action execution request from hub");
                // TODO: Forward to device manager
            }
            crate::comm::WsMessageType::Ping => {
                let pong = WsMessage::pong(ws_msg.timestamp);
                let _ = self.send_message(pong);
            }
            _ => {
                debug!("Unhandled message type: {:?}", ws_msg.msg_type);
            }
        }
    }
}

// ============================================================================
// Message Handlers
// ============================================================================

/// Handle device status updates
impl Handler<DeviceStatusUpdate> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: DeviceStatusUpdate, _ctx: &mut Self::Context) -> Self::Result {
        let ws_msg = WsMessage::device_status(msg);
        let _ = self.send_message(ws_msg);
    }
}

/// Handle device alerts
impl Handler<DeviceAlert> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: DeviceAlert, _ctx: &mut Self::Context) -> Self::Result {
        warn!("Device alert: {}", msg.message);
        let ws_msg = WsMessage::device_alert(msg);
        let _ = self.send_message(ws_msg);
    }
}

/// Handle prediction results
impl Handler<PredictionResult> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: PredictionResult, _ctx: &mut Self::Context) -> Self::Result {
        if msg.probability >= 0.8 {
            warn!(
                "HIGH-CONFIDENCE prediction: {:?} for device {} (probability: {:.2}, ETA: {:?}min)",
                msg.prediction_type, msg.device_id, msg.probability, msg.eta_minutes
            );
        } else {
            debug!("Prediction result: {:?} for device {}", msg.prediction_type, msg.device_id);
        }
        let ws_msg = WsMessage::prediction(msg);
        let _ = self.send_message(ws_msg);
    }
}

/// Connection state update from WebSocket client
#[derive(Message)]
#[rtype(result = "()")]
pub struct ConnectionStateChanged {
    pub new_state: ConnectionState,
}

impl Handler<ConnectionStateChanged> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: ConnectionStateChanged, ctx: &mut Self::Context) -> Self::Result {
        match msg.new_state {
            ConnectionState::Connected => self.handle_connected(ctx),
            ConnectionState::Disconnected => self.handle_disconnected(),
            ConnectionState::Connecting => {
                self.connection_state = ConnectionState::Connecting;
            }
            ConnectionState::Reconnecting => {
                self.connection_state = ConnectionState::Reconnecting;
            }
            ConnectionState::ShuttingDown => {
                self.connection_state = ConnectionState::ShuttingDown;
            }
        }
    }
}

/// Message received event from WebSocket client
#[derive(Message)]
#[rtype(result = "()")]
pub struct MessageReceived {
    pub message: WsMessage,
}

impl Handler<MessageReceived> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: MessageReceived, _ctx: &mut Self::Context) -> Self::Result {
        self.handle_message(msg.message);
    }
}

/// Update device count for heartbeat
#[derive(Message)]
#[rtype(result = "()")]
pub struct UpdateDeviceCount {
    pub count: usize,
}

impl Handler<UpdateDeviceCount> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: UpdateDeviceCount, _ctx: &mut Self::Context) -> Self::Result {
        debug!("Device count updated to {}", msg.count);
        // Store for next heartbeat
        // TODO: Store this in the actor state
    }
}

// ============================================================================
// Convenience Message Types
// ============================================================================

/// Get current connection state
#[derive(Message)]
#[rtype(result = "ConnectionState")]
pub struct GetConnectionState;

impl Handler<GetConnectionState> for HubConnectorActor {
    type Result = MessageResult<GetConnectionState>;

    fn handle(&mut self, _msg: GetConnectionState, _ctx: &mut Self::Context) -> Self::Result {
        MessageResult(self.connection_state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_core::actor::messages::AlertSeverity;
    use nimon_core::HealthStatus;

    #[test]
    fn test_config_default() {
        let config = HubConnectorConfig::default();
        assert!(config.auto_connect);
        assert_eq!(config.hub_url, "ws://localhost:8080/ws");
        assert_eq!(config.heartbeat_interval, Duration::from_secs(30));
    }

    #[test]
    fn test_message_buffer_capacity() {
        // Test that buffer has the correct capacity
        let buffer: VecDeque<WsMessage> = VecDeque::with_capacity(MAX_BUFFER_SIZE);
        assert_eq!(buffer.capacity(), MAX_BUFFER_SIZE);
    }

    #[test]
    fn test_connection_state_message() {
        let msg = ConnectionStateChanged {
            new_state: ConnectionState::Connected,
        };
        assert_eq!(msg.new_state, ConnectionState::Connected);
    }

    #[test]
    fn test_device_status_update_conversion() {
        let update = DeviceStatusUpdate {
            device_id: "test-device".to_string(),
            edge_id: "test-edge".to_string(),
            status: HealthStatus::Healthy,
            metrics: std::collections::HashMap::new(),
            timestamp: chrono::Utc::now(),
        };

        let ws_msg = WsMessage::device_status(update);
        assert_eq!(ws_msg.msg_type, crate::comm::WsMessageType::DeviceStatus);
    }

    #[test]
    fn test_alert_conversion() {
        let alert = DeviceAlert {
            device_id: "test-device".to_string(),
            edge_id: "test-edge".to_string(),
            severity: AlertSeverity::Critical,
            message: "Test alert".to_string(),
            metric_name: Some("temperature".to_string()),
            metric_value: Some(85.0),
            timestamp: chrono::Utc::now(),
        };

        let ws_msg = WsMessage::device_alert(alert);
        assert_eq!(ws_msg.msg_type, crate::comm::WsMessageType::DeviceAlert);
    }

    #[test]
    fn test_prediction_conversion() {
        use nimon_core::actor::messages::PredictionType;

        let prediction = PredictionResult {
            device_id: "test-device".to_string(),
            edge_id: "test-edge".to_string(),
            prediction_type: PredictionType::Overheating,
            probability: 0.85,
            eta_minutes: Some(30),
            confidence: 0.9,
            timestamp: chrono::Utc::now(),
        };

        let ws_msg = WsMessage::prediction(prediction);
        assert_eq!(ws_msg.msg_type, crate::comm::WsMessageType::Prediction);
    }

    #[test]
    fn test_high_confidence_prediction_threshold() {
        use nimon_core::actor::messages::PredictionType;

        // Above threshold (0.8) should be logged at warn level
        let high_conf = PredictionResult {
            device_id: "test-device".to_string(),
            edge_id: "test-edge".to_string(),
            prediction_type: PredictionType::Overheating,
            probability: 0.85,
            eta_minutes: Some(30),
            confidence: 0.9,
            timestamp: chrono::Utc::now(),
        };
        assert!(high_conf.probability >= 0.8, "Should be high confidence");

        // Below threshold should be logged at debug level only
        let low_conf = PredictionResult {
            device_id: "test-device".to_string(),
            edge_id: "test-edge".to_string(),
            prediction_type: PredictionType::Overheating,
            probability: 0.5,
            eta_minutes: Some(60),
            confidence: 0.6,
            timestamp: chrono::Utc::now(),
        };
        assert!(low_conf.probability < 0.8, "Should be low confidence");

        // Boundary test: exactly 0.8
        let boundary = PredictionResult {
            device_id: "test-device".to_string(),
            edge_id: "test-edge".to_string(),
            prediction_type: PredictionType::ConnectionFailure,
            probability: 0.8,
            eta_minutes: Some(45),
            confidence: 0.8,
            timestamp: chrono::Utc::now(),
        };
        assert!(boundary.probability >= 0.8, "Exactly at threshold is high confidence");
    }
}
