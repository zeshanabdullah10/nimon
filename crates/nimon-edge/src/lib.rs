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

pub mod actor;
pub mod comm;
pub mod config;
pub mod prediction;

pub use config::EdgeConfig;

/// Start the edge node (devices, prediction, hub connector) and run forever.
///
/// This is the library entry point used by the `nimon-edge` binary and by
/// embedding applications (e.g. `nimon-widget`). Must run inside an actix
/// system (e.g. `actix_rt::System::new().block_on(start(cfg))`).
pub async fn start(config: EdgeConfig) {
    use actix::prelude::*;
    use std::time::Duration;

    use crate::actor::{
        ConnectionStateChanged, DeviceManagerActor, HubConnectorActor, HubConnectorConfig,
    };
    use crate::comm::{Connect, WsClient, WsClientConfig, WsClientEvent};
    use tracing::{error, info, warn};

    info!(
        "Starting NIMon edge node {} ({})",
        config.node.name, config.node.id
    );

    let hub_url = format!("ws://{}/ws", config.node.hub_address);

    // WebSocket transport
    let ws_config = WsClientConfig {
        hub_url: hub_url.clone(),
        edge_id: config.node.id.clone(),
        auto_reconnect: true,
        max_reconnect_attempts: 0,
        reconnect_delay: Duration::from_secs(config.node.reconnect_interval_secs),
        ping_interval: Duration::from_secs(30),
        buffer_size: 1000,
    };
    let mut ws_client = WsClient::new(ws_config);
    let events = ws_client.subscribe();
    let ws_addr = ws_client.start();

    // Hub connector: registration, heartbeat, message buffering
    let hostname = std::env::var("COMPUTERNAME").ok();
    let hub_config = HubConnectorConfig {
        hub_url,
        edge_id: config.node.id.clone(),
        edge_name: config.node.name.clone(),
        hostname,
        ip_address: None,
        heartbeat_interval: Duration::from_secs(30),
        auto_connect: false,
    };
    let hub_addr = HubConnectorActor::new(hub_config, ws_addr.clone()).start();

    // Bridge WebSocket client events into the hub connector so it
    // registers on connect and re-registers after every reconnect
    let bridge_hub = hub_addr.clone();
    tokio::spawn(async move {
        let mut events = events;
        while let Some(event) = events.recv().await {
            match event {
                WsClientEvent::Connected => {
                    let _ = bridge_hub
                        .send(ConnectionStateChanged {
                            new_state: crate::comm::ConnectionState::Connected,
                        })
                        .await;
                }
                WsClientEvent::Disconnected => {
                    let _ = bridge_hub
                        .send(ConnectionStateChanged {
                            new_state: crate::comm::ConnectionState::Disconnected,
                        })
                        .await;
                }
                WsClientEvent::MessageReceived(msg) => {
                    bridge_hub.do_send(crate::actor::MessageReceived { message: msg });
                }
                WsClientEvent::Error(e) => warn!("WebSocket error: {}", e),
            }
        }
    });

    // Device discovery + polling. Auto-discovers real NI devices via
    // NI-SysCfg (falls back to simulated devices when unavailable).
    let manager = DeviceManagerActor::new(config.node.id.clone())
        .with_poll_interval(config.api.syscfg.poll_interval_secs)
        .with_hub_connector(hub_addr);
    manager.start();

    info!(
        "Edge node running - polling every {}s, hub at {}",
        config.api.syscfg.poll_interval_secs, config.node.hub_address
    );

    // Initial connect attempt (errors are surfaced; WsClient auto-reconnects)
    if let Err(e) = ws_addr.send(Connect).await {
        error!("Initial hub connection failed: {}", e);
    }

    futures::future::pending::<()>().await;
}
