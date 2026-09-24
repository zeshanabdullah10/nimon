//! Communication module for edge-hub connectivity
//!
//! Provides the WebSocket transport, the offline message buffer and the
//! message protocol (re-exported from nimon-core).

mod buffer;
mod message;
mod ws_client;

pub use buffer::{Outbound, OutboundBuffer, OutboundClass};
pub use message::{
    AckMessage, ErrorMessage, HubCommand, PingMessage, PongMessage, WsMessage, WsMessageType,
};
pub use ws_client::{
    build_request, Backoff, ConnectionSender, ConnectionState, WsClient, WsClientConfig, WsEvent,
};

/// Re-exports of commonly used types
pub mod prelude {
    pub use super::{WsClient, WsClientConfig, WsMessage, WsMessageType};
}
