//! NIMon Hub Library
//!
//! Central server for aggregating data from edge nodes.

pub mod action;
pub mod alert;
pub mod config;
pub mod server;
pub mod session;

pub use server::run;

