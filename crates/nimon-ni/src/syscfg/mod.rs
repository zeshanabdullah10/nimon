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

mod ffi;
mod safe;
mod types;

pub use safe::{NiSysCfg, SysCfgSession};
pub use types::{DiscoveredDevice, DeviceHealth};
