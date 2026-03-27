//! NI-DAQmx device monitoring
//!
//! This module provides safe Rust wrappers around the NI-DAQmx C API for
//! discovering DAQ devices and querying their health information (temperature,
//! self-test results, power supply voltages).
//!
//! # Example
//! ```no_run
//! use nimon_ni::daqmx::NiDaqMx;
//!
//! // Check if NI-DAQmx is available
//! if NiDaqMx::is_available() {
//!     let api = NiDaqMx::load().unwrap();
//!     let devices = api.discover_devices().unwrap();
//!     for device in devices {
//!         println!("Found: {}", device.device_name);
//!     }
//! }
//! ```

mod ffi;
mod safe;
mod types;

pub use safe::NiDaqMx;
pub use types::{DaqDevice, DaqHealth};
