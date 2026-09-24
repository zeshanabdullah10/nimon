//! NIMon Edge Node binary
//!
//! Discovers NI hardware via NI-SysCfg, polls device health, runs local
//! prediction models, and streams everything to the central hub over WebSocket.
//!
//! Usage: nimon-edge [path/to/edge.yaml]
//! Defaults to config/edge.yaml when no path is given. Ctrl-C shuts down
//! gracefully.

use nimon_edge::config::EdgeConfig;

fn main() {
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config/edge.yaml".to_string());

    let config = match EdgeConfig::from_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config {}: {}", config_path, e);
            std::process::exit(1);
        }
    };

    // keep the guard alive: dropping it flushes the file writer
    let _log_guard = nimon_edge::logging::init(&config.logging);

    let runtime = actix_rt::System::new();
    runtime.block_on(nimon_edge::start_with_shutdown(config, async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("Ctrl-C handler unavailable ({e}); running until killed");
            std::future::pending::<()>().await;
        }
    }));
}
