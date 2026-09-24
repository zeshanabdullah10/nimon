//! NIMon Hub Library
//!
//! Central server for aggregating data from edge nodes.

pub mod action;
pub mod alert;
pub mod config;
pub mod db_writer;
pub mod queries;
pub mod server;
pub mod service;
pub mod session;

pub use server::{build_router, run, run_with_shutdown, start_hub_services, HubState};
