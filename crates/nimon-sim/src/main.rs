//! NIMon Edge Simulator
//!
//! Test harness for the hub: runs N simulated edges (one WebSocket
//! connection each) speaking the real edge protocol, with scenarios that
//! exercise threshold alerts, trend predictions, hysteresis, device
//! removal, session replacement and load.
//!
//! ```text
//! nimon-sim --scenario overheat --duration-secs 120
//! nimon-sim --edges 3 --devices 8 --action-mode delay:2000 --seed 42
//! NIMON_EDGE_TOKEN=secret nimon-sim --url ws://hub:9090/ws
//! ```

mod actions;
mod cli;
mod edge;
mod model;
mod stats;

use clap::Parser;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;
use tracing::{error, info, warn};

use cli::{Args, Scenario};
use edge::{RunState, Shared};
use stats::Stats;

/// Exit code for fatal protocol/auth errors
const EXIT_FATAL: u8 = 2;
/// Exit code for invalid command-line options
const EXIT_USAGE: u8 = 64;

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_max_level(if args.verbose {
            tracing::Level::DEBUG
        } else {
            tracing::Level::INFO
        })
        .with_target(false)
        .init();

    let cfg = match args.resolve(rand::random()) {
        Ok(cfg) => Arc::new(cfg),
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    info!(
        url = %cfg.url,
        scenario = %cfg.scenario,
        edges = cfg.edges,
        devices = cfg.devices,
        interval = ?cfg.interval,
        seed = cfg.seed,
        action_mode = %cfg.action_mode,
        auth = cfg.token.is_some(),
        "nimon-sim starting (re-run with --seed {} to reproduce)",
        cfg.seed
    );

    let stats = Arc::new(Stats::default());
    let (run_tx, mut run_rx) = watch::channel(RunState::Running);
    let shared = Shared {
        cfg: cfg.clone(),
        stats: stats.clone(),
        run: Arc::new(run_tx),
    };
    let started = Instant::now();

    let handles: Vec<_> = (0..cfg.edges)
        .map(|i| tokio::spawn(edge::run_edge(i, shared.clone())))
        .collect();
    let reporter = (cfg.scenario == Scenario::Burst)
        .then(|| tokio::spawn(throughput_reporter(stats.clone(), cfg.edges)));

    let why = tokio::select! {
        _ = async {
            match cfg.duration {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending().await,
            }
        } => "duration elapsed",
        _ = tokio::signal::ctrl_c() => "interrupted",
        _ = run_rx.wait_for(|s| matches!(s, RunState::Fatal(_))) => "fatal error",
    };
    info!("stopping: {why}");
    shared.run.send_if_modified(|s| {
        if *s == RunState::Running {
            *s = RunState::Stopping;
            true
        } else {
            false
        }
    });
    if let Some(r) = reporter {
        r.abort();
    }
    // let edges send close frames, but never hang on exit
    if tokio::time::timeout(
        Duration::from_secs(5),
        futures_util::future::join_all(handles),
    )
    .await
    .is_err()
    {
        warn!("some edges did not stop within 5s");
    }

    let fatal = match &*shared.run.borrow() {
        RunState::Fatal(reason) => Some(reason.clone()),
        _ => None,
    };
    println!("{}", stats.summary(started.elapsed(), fatal.as_deref()));
    match fatal {
        Some(reason) => {
            error!("{reason}");
            ExitCode::from(EXIT_FATAL)
        }
        None => ExitCode::SUCCESS,
    }
}

/// Burst scenario: log send/receive rates every few seconds.
async fn throughput_reporter(stats: Arc<Stats>, edges: usize) {
    const EVERY: Duration = Duration::from_secs(5);
    let mut last = (stats.total_sent(), stats.total_received(), Instant::now());
    loop {
        tokio::time::sleep(EVERY).await;
        let now = (stats.total_sent(), stats.total_received(), Instant::now());
        let secs = (now.2 - last.2).as_secs_f64().max(0.001);
        info!(
            "throughput: {:.0} msg/s sent, {:.0} msg/s received, {}/{} edges connected",
            (now.0 - last.0) as f64 / secs,
            (now.1 - last.1) as f64 / secs,
            stats::get(&stats.connected),
            edges
        );
        last = now;
    }
}
