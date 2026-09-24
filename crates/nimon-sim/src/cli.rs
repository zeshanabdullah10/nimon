//! Command-line options and their resolution into a run configuration.

use clap::{Parser, ValueEnum};
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

/// Canned behaviours that exercise specific hub features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Scenario {
    /// Healthy devices: bounded random walk well below the warning threshold
    Steady,
    /// One device ramps ~1 C/interval through warning to critical and plateaus
    Overheat,
    /// One device oscillates around the warning threshold
    Flap,
    /// One device stops reporting, later a `device_removed` is sent
    Offline,
    /// Every edge drops and re-establishes its socket periodically
    Reconnect,
    /// Two devices share a product name (ids use the `#1`/`#2` suffix form)
    Duplicate,
    /// Many edges/devices on a fast interval; prints msgs/sec
    Burst,
}

impl fmt::Display for Scenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self
            .to_possible_value()
            .map(|v| v.get_name().to_string())
            .unwrap_or_default();
        f.write_str(&name)
    }
}

/// How the simulator answers hub `execute_action` requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionMode {
    /// Reply immediately with success
    Success,
    /// Reply immediately with failure
    Fail,
    /// Never reply (exercises hub-side action timeouts)
    Timeout,
    /// Reply with success after the given number of milliseconds
    Delay(u64),
}

impl FromStr for ActionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = s.trim().to_ascii_lowercase();
        match lower.as_str() {
            "success" | "ok" => Ok(Self::Success),
            "fail" | "failure" => Ok(Self::Fail),
            "timeout" => Ok(Self::Timeout),
            other => match other.strip_prefix("delay:") {
                Some(ms) => ms
                    .trim()
                    .parse::<u64>()
                    .map(Self::Delay)
                    .map_err(|_| format!("invalid delay '{ms}': expected milliseconds")),
                None => Err(format!(
                    "unknown action mode '{s}' (expected success|fail|timeout|delay:<ms>)"
                )),
            },
        }
    }
}

impl fmt::Display for ActionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => f.write_str("success"),
            Self::Fail => f.write_str("fail"),
            Self::Timeout => f.write_str("timeout"),
            Self::Delay(ms) => write!(f, "delay:{ms}"),
        }
    }
}

/// NIMon edge simulator: drives one or more fake edges against a hub.
#[derive(Debug, Parser)]
#[command(name = "nimon-sim", version, about, long_about = None)]
pub struct Args {
    /// Hub WebSocket URL
    #[arg(long, env = "NIMON_HUB_WS", default_value = "ws://127.0.0.1:9090/ws")]
    pub url: String,

    /// Bearer token sent in the WebSocket handshake (Authorization header)
    #[arg(long, env = "NIMON_EDGE_TOKEN", hide_env_values = true)]
    pub token: Option<String>,

    /// Number of simulated edges, each with its own connection [default: 1, burst: 20]
    #[arg(long)]
    pub edges: Option<usize>,

    /// Devices per edge [default: 4, burst: 16]
    #[arg(long)]
    pub devices: Option<usize>,

    /// Seconds between status sweeps; fractions allowed [default: 5, burst: 0.5]
    #[arg(long)]
    pub interval_secs: Option<f64>,

    /// RNG seed for reproducible runs (random when omitted; logged at start)
    #[arg(long)]
    pub seed: Option<u64>,

    /// Stop after this many seconds and print a summary (0 = run forever)
    #[arg(long, default_value_t = 0)]
    pub duration_secs: u64,

    /// Scenario to run
    #[arg(long, value_enum, default_value_t = Scenario::Steady)]
    pub scenario: Scenario,

    /// Reply to execute_action with: success | fail | timeout | delay:<ms>
    #[arg(long, default_value = "success")]
    pub action_mode: ActionMode,

    /// Edge id prefix (edges are named <prefix>-01, <prefix>-02, ...)
    #[arg(long, default_value = "sim-edge")]
    pub edge_prefix: String,

    /// Debug logging (every message sent/received)
    #[arg(short, long)]
    pub verbose: bool,
}

/// Fully resolved run configuration.
#[derive(Debug, Clone)]
pub struct SimConfig {
    pub url: String,
    pub token: Option<String>,
    pub edges: usize,
    pub devices: usize,
    pub interval: Duration,
    pub seed: u64,
    pub duration: Option<Duration>,
    pub scenario: Scenario,
    pub action_mode: ActionMode,
    pub edge_prefix: String,
}

/// Smallest accepted sweep interval.
const MIN_INTERVAL_SECS: f64 = 0.05;

impl Args {
    /// Apply scenario defaults and validate. `fallback_seed` is used when
    /// no `--seed` was given.
    pub fn resolve(self, fallback_seed: u64) -> Result<SimConfig, String> {
        let burst = self.scenario == Scenario::Burst;
        let edges = self.edges.unwrap_or(if burst { 20 } else { 1 });
        let mut devices = self.devices.unwrap_or(if burst { 16 } else { 4 });
        let interval_secs = self.interval_secs.unwrap_or(if burst { 0.5 } else { 5.0 });

        if edges == 0 {
            return Err("--edges must be at least 1".into());
        }
        if devices == 0 {
            return Err("--devices must be at least 1".into());
        }
        if !interval_secs.is_finite() || interval_secs < MIN_INTERVAL_SECS {
            return Err(format!("--interval-secs must be >= {MIN_INTERVAL_SECS}"));
        }
        if self.scenario == Scenario::Duplicate && devices < 2 {
            devices = 2;
        }
        let token = self.token.filter(|t| !t.trim().is_empty());

        Ok(SimConfig {
            url: self.url,
            token,
            edges,
            devices,
            interval: Duration::from_secs_f64(interval_secs),
            seed: self.seed.unwrap_or(fallback_seed),
            duration: (self.duration_secs > 0).then(|| Duration::from_secs(self.duration_secs)),
            scenario: self.scenario,
            action_mode: self.action_mode,
            edge_prefix: self.edge_prefix,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Args {
        let mut v = vec!["nimon-sim"];
        v.extend_from_slice(args);
        Args::try_parse_from(v).expect("args parse")
    }

    #[test]
    fn action_mode_parsing() {
        assert_eq!("success".parse::<ActionMode>(), Ok(ActionMode::Success));
        assert_eq!(" FAIL ".parse::<ActionMode>(), Ok(ActionMode::Fail));
        assert_eq!("timeout".parse::<ActionMode>(), Ok(ActionMode::Timeout));
        assert_eq!(
            "delay:250".parse::<ActionMode>(),
            Ok(ActionMode::Delay(250))
        );
        assert_eq!("delay: 0".parse::<ActionMode>(), Ok(ActionMode::Delay(0)));
        assert!("delay:".parse::<ActionMode>().is_err());
        assert!("delay:abc".parse::<ActionMode>().is_err());
        assert!("delay:-5".parse::<ActionMode>().is_err());
        assert!("sometimes".parse::<ActionMode>().is_err());
        // Display round-trips
        for m in [
            ActionMode::Success,
            ActionMode::Fail,
            ActionMode::Timeout,
            ActionMode::Delay(42),
        ] {
            assert_eq!(m.to_string().parse::<ActionMode>(), Ok(m));
        }
    }

    #[test]
    fn defaults_resolve() {
        let cfg = parse(&[]).resolve(7).unwrap();
        assert_eq!(cfg.edges, 1);
        assert_eq!(cfg.devices, 4);
        assert_eq!(cfg.interval, Duration::from_secs(5));
        assert_eq!(cfg.seed, 7);
        assert_eq!(cfg.duration, None);
        assert_eq!(cfg.scenario, Scenario::Steady);
        assert_eq!(cfg.action_mode, ActionMode::Success);
    }

    #[test]
    fn burst_defaults_and_explicit_override() {
        let cfg = parse(&["--scenario", "burst"]).resolve(0).unwrap();
        assert_eq!(cfg.edges, 20);
        assert_eq!(cfg.devices, 16);
        assert_eq!(cfg.interval, Duration::from_millis(500));

        let cfg = parse(&[
            "--scenario",
            "burst",
            "--edges",
            "3",
            "--interval-secs",
            "2",
        ])
        .resolve(0)
        .unwrap();
        assert_eq!(cfg.edges, 3);
        assert_eq!(cfg.interval, Duration::from_secs(2));
    }

    #[test]
    fn options_parse() {
        let cfg = parse(&[
            "--url",
            "ws://hub:1/ws",
            "--token",
            "abc",
            "--seed",
            "99",
            "--duration-secs",
            "30",
            "--action-mode",
            "delay:1500",
            "--scenario",
            "overheat",
        ])
        .resolve(0)
        .unwrap();
        assert_eq!(cfg.url, "ws://hub:1/ws");
        assert_eq!(cfg.token.as_deref(), Some("abc"));
        assert_eq!(cfg.seed, 99);
        assert_eq!(cfg.duration, Some(Duration::from_secs(30)));
        assert_eq!(cfg.action_mode, ActionMode::Delay(1500));
        assert_eq!(cfg.scenario, Scenario::Overheat);
    }

    #[test]
    fn validation() {
        assert!(parse(&["--edges", "0"]).resolve(0).is_err());
        assert!(parse(&["--devices", "0"]).resolve(0).is_err());
        assert!(parse(&["--interval-secs", "0"]).resolve(0).is_err());
        assert!(Args::try_parse_from(["nimon-sim", "--action-mode", "bogus"]).is_err());
        // duplicate needs two devices
        let cfg = parse(&["--scenario", "duplicate", "--devices", "1"])
            .resolve(0)
            .unwrap();
        assert_eq!(cfg.devices, 2);
        // blank token counts as no token
        let cfg = parse(&["--token", "  "]).resolve(0).unwrap();
        assert_eq!(cfg.token, None);
    }
}
