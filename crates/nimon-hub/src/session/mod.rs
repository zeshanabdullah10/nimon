//! Session management for connected edge nodes

pub mod edge_session;
pub mod session_store;

pub use edge_session::{
    is_legacy_protocol, DeviceSnapshot, EdgeSession, HeartbeatInfo, OUTBOUND_CAPACITY,
};
pub use session_store::SessionStore;
