//! NIMon Core - Shared types and utilities

pub mod actor;
pub mod alert;
pub mod db;
pub mod error;
pub mod protocol;
pub mod types;

pub use error::{NimonError, NimonResult};
pub use types::*;
