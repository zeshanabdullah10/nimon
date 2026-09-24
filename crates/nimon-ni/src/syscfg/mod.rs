//! NI System Configuration API bindings
//!
//! This module provides safe Rust wrappers around the NI-SysCfg API for
//! discovering and querying NI hardware devices.
//!
//! # Example
//! ```no_run
//! use nimon_ni::syscfg::{NiSysCfg, SysCfgSession};
//!
//! // Check if NI-SysCfg is available
//! if NiSysCfg::is_available() {
//!     let api = NiSysCfg::load().unwrap();
//!     let session = api.create_session().unwrap();
//!     let devices = session.discover_devices().unwrap();
//!     for device in devices {
//!         println!("Found: {} ({})", device.product_name, device.serial_number);
//!     }
//! }
//! ```
//!
//! # Reusing a session across sweeps
//! All calls block; run them on a blocking thread. The session is
//! `Send + Sync`, reopens itself after an enumeration failure, and closes
//! its handle on drop.
//! ```no_run
//! use std::sync::Arc;
//! use nimon_ni::syscfg::SysCfgSession;
//!
//! let session = Arc::new(SysCfgSession::open().unwrap()); // once
//! // every sweep (e.g. inside spawn_blocking):
//! let s = Arc::clone(&session);
//! let sweep = s.discover_with_health();
//! let system = s.system_info();
//! ```

mod ffi;
mod safe;
mod types;

pub use safe::{NiSysCfg, SysCfgSession};
pub use types::{DeviceHealth, DiscoveredDevice, SensorReading, SystemInfo};
