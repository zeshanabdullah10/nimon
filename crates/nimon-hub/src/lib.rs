//! NIMon Hub Library
//!
//! Central server for aggregating data from edge nodes.

pub mod alert;
pub mod server;
pub mod session;

pub use server::run;

