//! NIMon Edge Node
//!
//! Edge collector that monitors NI hardware on a single system and
//! communicates with the central hub.
//!
//! # Features
//! - Device discovery via NI-SysCfg
//! - Health monitoring and polling
//! - Local prediction engine
//! - Data buffering for offline operation
//! - WebSocket communication with hub
//!
//! # Example
//! ```no_run
//! use nimon_edge::config::EdgeConfig;
//!
//! let config = EdgeConfig::from_file("config/edge.yaml").unwrap();
//! println!("Edge node: {} ({})", config.node.name, config.node.id);
//! ```

pub mod config;

pub use config::EdgeConfig;
