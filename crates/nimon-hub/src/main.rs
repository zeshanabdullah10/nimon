//! NIMon Hub Server
//!
//! Central aggregation server for edge nodes.
//! Accepts WebSocket connections from edge nodes and aggregates device data.

use std::path::PathBuf;

use tracing::info;
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    let config_path = std::env::args().nth(1);
    let config = match config_path {
        Some(path) => {
            let path = PathBuf::from(path);
            info!("Loading config from {:?}", path);
            nimon_hub::config::HubConfig::load_or_default(&path)?
        }
        None => {
            info!("Using default configuration");
            nimon_hub::config::HubConfig::default()
        }
    };

    info!("Starting NIMon Hub Server on {}:{}", config.host, config.port);

    // Actix 0.13 uses tokio::task::spawn_local internally, which requires a LocalSet.
    let local = tokio::task::LocalSet::new();
    local.run_until(nimon_hub::run(config)).await?;

    Ok(())
}
