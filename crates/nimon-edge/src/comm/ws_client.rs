//! WebSocket client for hub communication
//!
//! Provides WebSocket connectivity with automatic reconnection,
//! message buffering for offline operation, and connection state management.

use std::sync::Arc;
use std::time::Duration;

use actix::prelude::*;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::protocol::Message as WsProtoMessage,
};
use tracing::{debug, error, info, warn};

use super::message::WsMessage;
use nimon_core::NimonError;

/// Connection state of the WebSocket client
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    ShuttingDown,
}

/// Events emitted by the WebSocket client
#[derive(Debug, Clone)]
pub enum WsClientEvent {
    Connected,
    Disconnected,
    MessageReceived(WsMessage),
    Error(String),
}

/// Configuration for the WebSocket client
#[derive(Debug, Clone)]
pub struct WsClientConfig {
    /// Hub WebSocket URL
    pub hub_url: String,
    /// Edge node ID
    pub edge_id: String,
    /// Auto-reconnect on disconnect
    pub auto_reconnect: bool,
    /// Maximum reconnection attempts (0 = infinite)
    pub max_reconnect_attempts: usize,
    /// Delay between reconnect attempts
    pub reconnect_delay: Duration,
    /// Ping interval for connection health
    pub ping_interval: Duration,
    /// Message buffer size for offline operation
    pub buffer_size: usize,
}

impl Default for WsClientConfig {
    fn default() -> Self {
        Self {
            hub_url: "ws://localhost:8080/ws".to_string(),
            edge_id: "edge-1".to_string(),
            auto_reconnect: true,
            max_reconnect_attempts: 0,
            reconnect_delay: Duration::from_secs(5),
            ping_interval: Duration::from_secs(30),
            buffer_size: 1000,
        }
    }
}

/// WebSocket client
pub struct WsClient {
    config: WsClientConfig,
    state: Arc<tokio::sync::RwLock<ConnectionState>>,
    event_tx: mpsc::UnboundedSender<WsClientEvent>,
    message_buffer: Arc<tokio::sync::Mutex<Vec<WsMessage>>>,
    shutdown_tx: Option<mpsc::UnboundedSender<()>>,
    /// Shared send channel so reconnect_task can update it after reconnecting.
    send_tx: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedSender<String>>>>,
}

impl WsClient {
    /// Create a new WebSocket client
    pub fn new(config: WsClientConfig) -> Self {
        let (event_tx, _) = mpsc::unbounded_channel();
        Self {
            config,
            state: Arc::new(tokio::sync::RwLock::new(ConnectionState::Disconnected)),
            event_tx,
            message_buffer: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            shutdown_tx: None,
            send_tx: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Subscribe to client events
    pub fn subscribe(&mut self) -> mpsc::UnboundedReceiver<WsClientEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.event_tx = tx;
        rx
    }

    /// Get current connection state
    pub async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    /// Send a message over WebSocket
    pub async fn send(&self, msg: WsMessage) -> Result<(), NimonError> {
        let state = self.state.read().await;
        if *state != ConnectionState::Connected {
            let mut buffer = self.message_buffer.lock().await;
            if buffer.len() < self.config.buffer_size {
                buffer.push(msg);
                debug!("Message buffered (not connected)");
                return Ok(());
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "Message buffer full",
                )
                .into());
            }
        }
        drop(state);

        // Send through channel to connection task
        let json = msg.to_json().map_err(|e| NimonError::Connection(e.to_string()))?;
        let tx_guard = self.send_tx.lock().await;
        if let Some(ref tx) = *tx_guard {
            tx.send(json).map_err(|e| NimonError::Connection(e.to_string()))?;
        } else {
            return Err(NimonError::Connection("No send channel available".to_string()));
        }
        Ok(())
    }

    /// Connect to the hub
    pub async fn connect(&mut self) -> Result<(), NimonError> {
        *self.state.write().await = ConnectionState::Connecting;
        self.emit_event(WsClientEvent::Connected);

        let url = self.config.hub_url.clone();
        let state = self.state.clone();

        match connect_async(&url).await {
            Ok((ws_stream, _)) => {
                *state.write().await = ConnectionState::Connected;
                info!("Connected to hub at {}", url);

                let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();
                self.shutdown_tx = Some(shutdown_tx);

                let (send_tx, outgoing_rx) = mpsc::unbounded_channel();
                *self.send_tx.lock().await = Some(send_tx);

                tokio::spawn(async move {
                    Self::handle_connection(ws_stream, &mut shutdown_rx, outgoing_rx).await;
                });

                self.send_buffered_messages().await;
                Ok(())
            }
            Err(e) => {
                let err: NimonError = std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    format!("Failed to connect: {}", e),
                )
                .into();
                *state.write().await = ConnectionState::Disconnected;
                self.emit_event(WsClientEvent::Error(format!("Connection failed: {}", e)));

                if self.config.auto_reconnect {
                    tokio::spawn(Self::reconnect_task(
                        self.config.clone(),
                        state.clone(),
                        self.send_tx.clone(),
                    ));
                }

                Err(err)
            }
        }
    }

    /// Disconnect from the hub
    pub async fn disconnect(&mut self) {
        *self.state.write().await = ConnectionState::ShuttingDown;

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        *self.state.write().await = ConnectionState::Disconnected;
        self.emit_event(WsClientEvent::Disconnected);
        info!("Disconnected from hub");
    }

    /// Handle WebSocket connection
    async fn handle_connection(
        ws_stream: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        shutdown: &mut mpsc::UnboundedReceiver<()>,
        mut outgoing: mpsc::UnboundedReceiver<String>,
    ) {
        let (mut write, mut read) = ws_stream.split();
        let ping_interval = Duration::from_secs(30);
        let mut ping_interval_tokio = tokio::time::interval(ping_interval);

        loop {
            tokio::select! {
                _ = ping_interval_tokio.tick() => {
                    let ping_msg = WsMessage::ping();
                    if let Ok(json) = ping_msg.to_json() {
                        if let Err(e) = write.send(WsProtoMessage::Text(json)).await {
                            error!("Failed to send ping: {}", e);
                            break;
                        }
                    }
                }
                Some(_) = shutdown.recv() => {
                    info!("Connection shutdown requested");
                    break;
                }
                Some(text) = outgoing.recv() => {
                    if let Err(e) = write.send(WsProtoMessage::Text(text)).await {
                        error!("Failed to send message: {}", e);
                        break;
                    }
                }
                Some(msg) = read.next() => {
                    match msg {
                        Ok(WsProtoMessage::Text(text)) => {
                            if let Ok(ws_msg) = WsMessage::from_json(&text) {
                                debug!("Received message: {:?}", ws_msg.msg_type);
                            }
                        }
                        Ok(WsProtoMessage::Close(_)) => {
                            info!("WebSocket closed by server");
                            break;
                        }
                        Ok(WsProtoMessage::Ping(data)) => {
                            let _ = write.send(WsProtoMessage::Pong(data)).await;
                        }
                        Err(e) => {
                            error!("WebSocket error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Reconnection task
    async fn reconnect_task(
        config: WsClientConfig,
        state: Arc<tokio::sync::RwLock<ConnectionState>>,
        send_tx: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedSender<String>>>>,
    ) {
        let mut attempts = 0;

        loop {
            if config.max_reconnect_attempts > 0 && attempts >= config.max_reconnect_attempts {
                warn!("Max reconnect attempts reached");
                break;
            }

            *state.write().await = ConnectionState::Reconnecting;
            tokio::time::sleep(config.reconnect_delay).await;

            match connect_async(&config.hub_url).await {
                Ok((ws_stream, _)) => {
                    info!("Reconnected to hub");
                    *state.write().await = ConnectionState::Connected;
                    let (_shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();
                    let (new_send_tx, outgoing_rx) = mpsc::unbounded_channel();

                    // Update the shared send_tx so the actor can send messages again
                    *send_tx.lock().await = Some(new_send_tx);

                    tokio::spawn(async move {
                        Self::handle_connection(ws_stream, &mut shutdown_rx, outgoing_rx).await;
                    });
                    break;
                }
                Err(e) => {
                    warn!("Reconnect attempt {} failed: {}", attempts + 1, e);
                    attempts += 1;
                }
            }
        }
    }

    /// Send buffered messages after reconnection
    async fn send_buffered_messages(&self) {
        let mut buffer = self.message_buffer.lock().await;
        if buffer.is_empty() {
            return;
        }

        info!("Sending {} buffered messages", buffer.len());
        for msg in buffer.drain(..) {
            if let Some(ref tx) = *self.send_tx.lock().await {
                match msg.to_json() {
                    Ok(json) => {
                        if let Err(e) = tx.send(json) {
                            error!("Failed to send buffered message: {}", e);
                            break;
                        }
                        debug!("Sent buffered message: {:?}", msg.msg_type);
                    }
                    Err(e) => {
                        error!("Failed to serialize buffered message: {}", e);
                    }
                }
            }
        }
    }

    /// Emit an event to subscribers
    fn emit_event(&self, event: WsClientEvent) {
        let _ = self.event_tx.send(event);
    }
}

impl Actor for WsClient {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        debug!("WsClient actor started");
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        debug!("WsClient actor stopped");
    }
}

/// Message to initiate connection
#[derive(Message)]
#[rtype(result = "Result<(), NimonError>")]
pub struct Connect;

impl Handler<Connect> for WsClient {
    type Result = ResponseActFuture<Self, Result<(), NimonError>>;

    fn handle(&mut self, _msg: Connect, _ctx: &mut Self::Context) -> Self::Result {
        let url = self.config.hub_url.clone();
        let state = self.state.clone();
        let auto_reconnect = self.config.auto_reconnect;
        let config_for_reconnect = self.config.clone();
        let message_buffer = self.message_buffer.clone();
        let send_tx_shared = self.send_tx.clone();
        let send_tx_for_map = self.send_tx.clone();

        let connect_fut = async move {
            *state.write().await = ConnectionState::Connecting;

            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    *state.write().await = ConnectionState::Connected;

                    let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();
                    let (send_tx, outgoing_rx) = mpsc::unbounded_channel();

                    tokio::spawn(async move {
                        Self::handle_connection(ws_stream, &mut shutdown_rx, outgoing_rx).await;
                    });

                    // Send any buffered messages through the channel
                    let mut buffer = message_buffer.lock().await;
                    if !buffer.is_empty() {
                        info!("Sending {} buffered messages", buffer.len());
                        for msg in buffer.drain(..) {
                            if let Ok(json) = msg.to_json() {
                                if let Err(e) = send_tx.send(json) {
                                    error!("Failed to send buffered message: {}", e);
                                    break;
                                }
                                debug!("Sent buffered message: {:?}", msg.msg_type);
                            }
                        }
                    }

                    Ok((shutdown_tx, send_tx))
                }
                Err(e) => {
                    let err: NimonError = std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        format!("Failed to connect: {}", e),
                    )
                    .into();
                    *state.write().await = ConnectionState::Disconnected;

                    if auto_reconnect {
                        tokio::spawn(Self::reconnect_task(
                            config_for_reconnect,
                            state,
                            Arc::clone(&send_tx_shared),
                        ));
                    }

                    Err(err)
                }
            }
        };

        Box::pin(connect_fut.into_actor(self).map(move |result, act, _ctx| {
            match result {
                Ok((shutdown_tx, send_tx)) => {
                    act.shutdown_tx = Some(shutdown_tx);
                    // Update the shared send_tx (this runs on the actor thread, safe to set directly)
                    let _ = send_tx_for_map.blocking_lock().insert(send_tx);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }))
    }
}

/// Message to send a WebSocket message
#[derive(Message)]
#[rtype(result = "Result<(), NimonError>")]
pub struct SendWsMessage {
    pub message: WsMessage,
}

impl Handler<SendWsMessage> for WsClient {
    type Result = ResponseActFuture<Self, Result<(), NimonError>>;

    fn handle(&mut self, msg: SendWsMessage, _ctx: &mut Self::Context) -> Self::Result {
        let client = self.clone();
        let message = msg.message;

        let fut = async move {
            client.send(message).await
        }
        .into_actor(self);

        Box::pin(fut)
    }
}

impl Clone for WsClient {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            state: self.state.clone(),
            event_tx: self.event_tx.clone(),
            message_buffer: self.message_buffer.clone(),
            shutdown_tx: None,
            send_tx: Arc::clone(&self.send_tx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = WsClientConfig::default();
        assert!(config.auto_reconnect);
        assert_eq!(config.max_reconnect_attempts, 0);
        assert_eq!(config.reconnect_delay, Duration::from_secs(5));
    }

    #[test]
    fn test_connection_state() {
        assert_ne!(
            ConnectionState::Connected,
            ConnectionState::Disconnected
        );
    }

    #[test]
    fn test_message_serialization() {
        let msg = WsMessage::ping();
        let json = msg.to_json();
        assert!(json.is_ok());
        assert!(json.unwrap().contains("ping"));
    }
}
