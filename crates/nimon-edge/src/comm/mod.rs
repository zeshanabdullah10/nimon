//! Communication module for edge-hub connectivity
//!
//! Provides WebSocket client, message protocol, and related utilities
//! for communicating between edge nodes and the central hub.

mod message;
mod ws_client;

pub use message::{AckMessage, ErrorMessage, PingMessage, PongMessage, WsMessage, WsMessageType};
pub use ws_client::{
    Connect, ConnectionState, SendWsMessage, WsClient, WsClientConfig, WsClientEvent,
};

/// Re-exports of commonly used types
pub mod prelude {
    pub use super::{WsClient, WsClientConfig, WsMessage, WsMessageType};
}
