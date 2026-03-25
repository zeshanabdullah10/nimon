//! NI API bindings for Rust
//!
//! This crate provides safe Rust wrappers around NI's C APIs for hardware monitoring.
//!
//! # Supported APIs
//! - NI-SysCfg (System Configuration)
//!
//! # Example
//! ```ignore
//! use nimon_ni::syscfg::NiSysCfg;
//!
//! // Check if NI software is available
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

// Re-export core types
pub use nimon_core::{NimonError, NimonResult};
