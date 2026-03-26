//! Actor infrastructure for the edge node
//!
//! This module provides the actor-based device monitoring system:
//!
//! - [`DeviceManagerActor`] - Manages all device actors and discovery
//! - [`DeviceActor`] - Monitors a single NI device
//! - [`PredictionActor`] - Analyzes device metrics and generates predictions
//!
//! # Architecture
//!
//! ```text
//! DeviceManagerActor
//!     |
//!     +-- DeviceActor (per device)
//!     |       |
//!     |       +-- Periodic polling
//!     |       +-- DeviceStatusUpdate emissions
//!     |
//!     +-- PredictionActor
//!             |
//!             +-- Receives DeviceStatusUpdate
//!             +-- Generates PredictionResult
//! ```

pub mod device_actor;
pub mod device_manager;
pub mod prediction_actor;

pub use device_actor::DeviceActor;
pub use device_manager::{
    AddDevice, DeviceManagerActor, GetDeviceCount, ListDevices, PollAllDevices, PollDevice,
    RemoveDevice,
};
pub use prediction_actor::PredictionActor;
