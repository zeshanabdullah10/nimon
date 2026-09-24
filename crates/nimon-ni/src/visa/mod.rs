//! NI-VISA instrument communication
//!
//! This module provides safe Rust wrappers around the NI-VISA API for
//! discovering and communicating with VISA instruments.
//!
//! # Example
//! ```no_run
//! use nimon_ni::visa::{NiVisa, VisaSession};
//!
//! // Check if NI-VISA is available
//! if NiVisa::is_available() {
//!     let api = NiVisa::load().unwrap();
//!     let session = api.create_session().unwrap();
//!     let instruments = session.discover_instruments().unwrap();
//!     for instr in instruments {
//!         println!("Found: {}", instr.resource_name);
//!     }
//! }
//! ```

mod ffi;
mod safe;
mod types;

pub use safe::{NiVisa, VisaSession, DEFAULT_IO_TIMEOUT_MS};
pub use types::{VisaHealth, VisaInstrument};
