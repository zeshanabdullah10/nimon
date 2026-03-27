//! NI API bindings for Rust
//!
//! This crate provides safe Rust wrappers around NI's C APIs for hardware monitoring.
//!
//! # Supported APIs
//! - NI-SysCfg (System Configuration) - Device discovery and status
//! - NI-VISA - Instrument discovery and communication
//! - NI-DAQmx - DAQ device discovery and health monitoring
//!
//! # Example
//! ```no_run
//! use nimon_ni::syscfg::NiSysCfg;
//!
//! if NiSysCfg::is_available() {
//!     let api = NiSysCfg::load().unwrap();
//!     let session = api.create_session().unwrap();
//!     let devices = session.discover_devices().unwrap();
//!     for device in devices {
//!         println!("Found: {}", device.product_name);
//!     }
//! }
//! ```

pub mod common;
pub mod daqmx;
pub mod syscfg;
pub mod visa;

// Re-export core types
pub use nimon_core::{NimonError, NimonResult};
