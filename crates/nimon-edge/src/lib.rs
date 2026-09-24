//! NIMon Edge Node
//!
//! Edge collector that monitors NI hardware on a single system and
//! communicates with the central hub.
//!
//! # Features
//! - Device discovery + health via one shared NI-SysCfg sweep
//! - Optional NI-VISA resource discovery
//! - Local prediction engine (per device)
//! - In-memory buffering while the hub is unreachable
//! - WebSocket (`ws://` / `wss://`, bearer token) communication with the hub
//!
//! # Example
//! ```no_run
//! use nimon_edge::config::EdgeConfig;
//!
//! let config = EdgeConfig::from_file("config/edge.yaml").unwrap();
//! println!("Edge node: {} ({})", config.node.name, config.node.id);
//! // run until Ctrl-C:
//! actix_rt::System::new().block_on(nimon_edge::start_with_shutdown(config, async {
//!     let _ = tokio::signal::ctrl_c().await;
//! }));
//! ```

pub mod action;
pub mod actor;
pub mod comm;
pub mod config;
pub mod devices;
pub mod logging;
pub mod prediction;

pub use config::EdgeConfig;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use actix::prelude::*;
use tracing::info;

use crate::action::ActionRuntime;
use crate::actor::{
    AttachDeviceManager, DeviceManagerActor, DeviceManagerSettings, Disconnect, HubConnectorActor,
    HubConnectorConfig, StopDeviceManager,
};
use crate::comm::WsClientConfig;
use crate::devices::DeviceRegistry;

/// Start the edge node and run forever (no signal handling).
///
/// Kept for embedders that run the edge on a dedicated thread for the
/// process lifetime (e.g. `nimon-widget`). Must run inside an actix
/// system (e.g. `actix_rt::System::new().block_on(start(cfg))`).
pub async fn start(config: EdgeConfig) {
    start_with_shutdown(config, std::future::pending::<()>()).await
}

/// Start the edge node and run until `shutdown` completes, then shut down
/// gracefully: stop the sweep timer, close the hub connection with a
/// close frame (bounded wait) and return.
///
/// Must run inside an actix system. Typical shutdown futures:
/// `async { let _ = tokio::signal::ctrl_c().await; }` or a
/// `tokio::sync::oneshot::Receiver` (`async { let _ = rx.await; }`).
pub async fn start_with_shutdown<F>(config: EdgeConfig, shutdown: F)
where
    F: Future<Output = ()>,
{
    let (hub, manager) = spawn_actors(&config);
    info!(
        "Edge node {} running - sweep every {}s, hub at {}",
        config.node.id,
        config.api.syscfg.poll_interval_secs,
        config.node.hub_url()
    );

    shutdown.await;

    info!("Shutting down edge node {}", config.node.id);
    let _ = manager.send(StopDeviceManager).await;
    let _ = hub.send(Disconnect).await;
    info!("Edge node stopped");
}

/// Build all actors from the config (must be inside an actix system).
/// Returns the hub connector and the device manager.
pub fn spawn_actors(config: &EdgeConfig) -> (Addr<HubConnectorActor>, Addr<DeviceManagerActor>) {
    info!(
        "Starting NIMon edge node {} ({}) v{}",
        config.node.name,
        config.node.id,
        env!("CARGO_PKG_VERSION")
    );

    let registry = DeviceRegistry::new();
    let action_runtime = Arc::new(ActionRuntime::new(
        &config.action,
        config.api.daqmx.enabled,
        config.node.id.clone(),
        registry.clone(),
    ));

    let node = &config.node;
    let ws = WsClientConfig {
        hub_url: node.hub_url(),
        auth_token: node.resolved_hub_token(),
        reconnect_delay: Duration::from_secs(node.reconnect_interval_secs.max(1)),
        max_reconnect_delay: Duration::from_secs(
            node.max_reconnect_interval_secs
                .max(node.reconnect_interval_secs.max(1)),
        ),
        ping_interval: Duration::from_secs(node.ping_interval_secs),
        ..WsClientConfig::default()
    };
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok();
    let hub_config = HubConnectorConfig {
        edge_id: node.id.clone(),
        edge_name: node.name.clone(),
        hostname,
        ip_address: None,
        heartbeat_interval: Duration::from_secs(node.heartbeat_interval_secs.max(1)),
        auto_connect: true,
        action_runtime,
        ws,
        buffer_enabled: config.buffer.enabled,
        buffer_max_messages: config.buffer.max_messages,
        buffer_max_bytes: (config.buffer.max_size_mb.max(1) as usize).saturating_mul(1024 * 1024),
        config_path: config.source_path.clone(),
    };
    let hub = HubConnectorActor::new(hub_config).start();

    let manager = DeviceManagerActor::new(node.id.clone())
        .with_settings(DeviceManagerSettings::from_config(config))
        .with_registry(registry)
        .with_hub_connector(hub.clone())
        .start();

    hub.do_send(AttachDeviceManager {
        manager: manager.clone(),
    });
    (hub, manager)
}
