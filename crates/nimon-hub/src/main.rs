//! NIMon Hub Server
//!
//! Central aggregation server for edge nodes.
//! Accepts WebSocket connections from edge nodes and aggregates device data.

use std::path::PathBuf;

use clap::Parser;
use tracing::info;
use tracing_subscriber::FmtSubscriber;

#[derive(clap::Subcommand)]
enum Commands {
    Install,
    Uninstall,
    Start,
    Stop,
}

#[derive(clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    config_path: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    let cli = Cli::parse();

    match &cli.command {
        #[cfg(windows)]
        Some(Commands::Install) => {
            let exe_path = std::env::current_exe()?.display().to_string();
            nimon_hub::service::install_service("NIMonHub", "NIMon Hub Server", &exe_path)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("Service installed");
            return Ok(());
        }
        #[cfg(windows)]
        Some(Commands::Uninstall) => {
            nimon_hub::service::uninstall_service("NIMonHub")
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("Service uninstalled");
            return Ok(());
        }
        #[cfg(windows)]
        Some(Commands::Start) => {
            nimon_hub::service::start_service("NIMonHub")
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("Service started");
            return Ok(());
        }
        #[cfg(windows)]
        Some(Commands::Stop) => {
            nimon_hub::service::stop_service("NIMonHub")
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("Service stopped");
            return Ok(());
        }
        None => {}
        #[cfg(not(windows))]
        _ => {
            eprintln!("Service commands are only available on Windows");
            return Ok(());
        }
    }

    let config_path = cli.config_path.as_ref();
    let config = match config_path {
        Some(path) => {
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
