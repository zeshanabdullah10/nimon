//! NIMon Edge Node binary
//!
//! Discovers NI hardware via NI-SysCfg, polls device health, runs local
//! prediction models, and streams everything to the central hub over WebSocket.
//!
//! Usage: nimon-edge [path/to/edge.yaml]
//! Defaults to config/edge.yaml when no path is given.

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

    let log_level = match config.logging.level.to_lowercase().as_str() {
        "trace" => tracing::Level::TRACE,
        "debug" => tracing::Level::DEBUG,
        "warn" => tracing::Level::WARN,
        "error" => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    };
    tracing_subscriber::fmt().with_max_level(log_level).init();

    let runtime = actix_rt::System::new();
    runtime.block_on(nimon_edge::start(config));
}
