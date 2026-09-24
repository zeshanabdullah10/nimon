//! Actor infrastructure for the edge node
//!
//! - [`DeviceManagerActor`] - owns all devices; one timer drives the shared
//!   NI-SysCfg sweep (blocking pool), VISA discovery and simulation
//! - [`PredictionActor`] - per-(device, metric) prediction models
//! - [`HubConnectorActor`] - registration, heartbeats, offline buffer,
//!   hub commands/actions; owns the WebSocket client
//!
//! # Architecture
//!
//! ```text
//! DeviceManagerActor --(DeviceStatusUpdate, DeviceRemoved)--> PredictionActor
//!        |                                                        |
//!        | DeviceAlert, UpdateDeviceCount       PredictionResult, status, removals
//!        v                                                        v
//!   HubConnectorActor <-------------------------------------------+
//!        |  single offline buffer, register-then-flush
//!        v
//!   WsClient (one supervisor task: connect / backoff / ping)  <-->  hub
//! ```

pub mod device_manager;
pub mod hub_connector;
pub mod prediction_actor;

pub use device_manager::{
    AddDevice, ApplyConfig, DeviceManagerActor, DeviceManagerSettings, GetDeviceCount,
    GetManagerSnapshot, ListDevices, ManagerSnapshot, PollAllDevices, RemoveDevice, ResendState,
    StopDeviceManager,
};
pub use hub_connector::{
    AttachDeviceManager, Connect, Disconnect, GetBufferedCount, GetConnectionState,
    HubConnectorActor, HubConnectorConfig, UpdateDeviceCount,
};
pub use prediction_actor::{PredictionActor, PredictionSettings, UpdateThresholds};
