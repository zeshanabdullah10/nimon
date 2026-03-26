//! NIMon Hub Server
//!
//! Central aggregation server for edge nodes.
//! Accepts WebSocket connections from edge nodes and aggregates device data.

use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    info!("Starting NIMon Hub Server");

    // Start the server
    nimon_hub::run().await?;

    Ok(())
}
