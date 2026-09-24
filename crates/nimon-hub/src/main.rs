//! NIMon Hub Server
//!
//! Central aggregation server for edge nodes.
//! Accepts WebSocket connections from edge nodes and aggregates device data.

use std::path::PathBuf;

use clap::Parser;
use tracing::info;
use tracing_subscriber::{EnvFilter, FmtSubscriber};

#[derive(clap::Subcommand)]
enum Commands {
    /// Register the hub as a Windows service (auto-start)
    Install,
    /// Remove the Windows service registration
    Uninstall,
    /// Start the registered Windows service
    Start,
    /// Stop the running Windows service
    Stop,
    /// Internal: service entry point used by the SCM (run-service)
    #[command(hide = true)]
    RunService,
}

#[derive(clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    config_path: Option<PathBuf>,
}

/// Initialize tracing (RUST_LOG controls verbosity, default info). The SCM
/// discards a service's stdout, so service mode logs to a daily-rotated file
/// in `<exe dir>/logs`. The returned guard must live until exit.
fn init_logging(service_mode: bool) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let log_dir = service_mode
        .then(|| std::env::current_exe().ok())
        .flatten()
        .and_then(|exe| exe.parent().map(|dir| dir.join("logs")))
        .filter(|dir| std::fs::create_dir_all(dir).is_ok());

    match log_dir {
        Some(dir) => {
            let appender = tracing_appender::rolling::daily(dir, "nimon-hub.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let subscriber = FmtSubscriber::builder()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(writer)
                .finish();
            tracing::subscriber::set_global_default(subscriber)
                .expect("setting default subscriber failed");
            Some(guard)
        }
        None => {
            let subscriber = FmtSubscriber::builder().with_env_filter(filter).finish();
            tracing::subscriber::set_global_default(subscriber)
                .expect("setting default subscriber failed");
            None
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let _log_guard = init_logging(matches!(cli.command, Some(Commands::RunService)));

    match &cli.command {
        #[cfg(windows)]
        Some(Commands::Install) => {
            let cmd_line = nimon_hub::service::service_exe_command_line()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            nimon_hub::service::install_service(
                nimon_hub::service::SERVICE_NAME,
                "NIMon Hub Server",
                &cmd_line,
            )
            .map_err(|e| anyhow::anyhow!("{}", e))?;
            nimon_hub::service::write_default_config_if_missing()
                .map_err(|e| anyhow::anyhow!("failed to write service config: {}", e))?;
            println!("Service installed ({})", cmd_line);
            return Ok(());
        }
        #[cfg(windows)]
        Some(Commands::Uninstall) => {
            nimon_hub::service::uninstall_service(nimon_hub::service::SERVICE_NAME)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("Service uninstalled");
            return Ok(());
        }
        #[cfg(windows)]
        Some(Commands::Start) => {
            nimon_hub::service::start_service(nimon_hub::service::SERVICE_NAME)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("Service started");
            return Ok(());
        }
        #[cfg(windows)]
        Some(Commands::Stop) => {
            nimon_hub::service::stop_service(nimon_hub::service::SERVICE_NAME)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("Service stopped");
            return Ok(());
        }
        #[cfg(windows)]
        Some(Commands::RunService) => {
            nimon_hub::service::run_as_service().map_err(|e| anyhow::anyhow!("{}", e))?;
            return Ok(());
        }
        #[cfg(not(windows))]
        Some(_) => {
            eprintln!("Service commands are only available on Windows");
            return Ok(());
        }
        None => {}
    }

    let config_path = cli.config_path.as_ref();
    let config = match config_path {
        Some(path) => {
            info!("Loading config from {:?}", path);
            nimon_hub::config::HubConfig::from_file(path)
                .map_err(|e| anyhow::anyhow!("failed to load config {}: {}", path.display(), e))?
        }
        None => {
            info!("Using default configuration");
            nimon_hub::config::HubConfig::default()
        }
    };

    info!(
        "Starting NIMon Hub Server on {}:{}",
        config.host, config.port
    );

    // Actix 0.13 uses tokio::task::spawn_local internally, which requires a LocalSet.
    let local = tokio::task::LocalSet::new();
    local.run_until(nimon_hub::run(config)).await?;

    Ok(())
}
