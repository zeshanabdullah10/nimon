//! NIMon Core - Shared types and utilities

pub mod actor;
pub mod alert;
pub mod db;
pub mod error;
pub mod protocol;
pub mod types;

pub use actor::messages::{DEFAULT_TEMP_CRITICAL_C, DEFAULT_TEMP_WARNING_C};
pub use error::{NimonError, NimonResult};
pub use types::*;
