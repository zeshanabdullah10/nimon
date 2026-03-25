//! Actor infrastructure for NIMon
//!
//! This module provides the actor message types and base infrastructure
//! for the actor-based device monitoring system.
//!
//! # Architecture
//! ```text
//! +--------------------------------------------------------------+
//! |                      EDGE NODE                                |
//! |                                                               |
//! |  +-----------------+     +-----------------+                 |
//! |  | DeviceManager   |---->| DeviceActor     | (per device)    |
//! |  | Actor           |     +--------+--------+                 |
//! |  +--------+--------+              |                          |
//! |           |                       | DeviceStatusUpdate       |
//! |           |                       v                          |
//! |           |              +-----------------+                 |
//! |           |              | PredictionActor |                 |
//! |           |              +--------+--------+                 |
//! |           |                       | PredictionResult         |
//! |           |                       v                          |
//! |           |              +-----------------+                 |
//! |           +------------->| HubConnector    |                 |
//! |                          | Actor           |                 |
//! |                          +--------+--------+                 |
//! |                                   |                          |
//! |                                   | WebSocket                |
//! +-----------------------------------+--------------------------+
//!                                     |
//!                                     v
//!                          +-----------------+
//!                          | HUB SERVER      |
//!                          | (EdgeSessions)  |
//!                          +-----------------+
//! ```

pub mod messages;

pub use messages::*;
