//! Command-line definition.

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::api::DEFAULT_HUB_URL;
use crate::format::{parse_duration, parse_rfc3339_arg};

#[derive(Debug, Parser)]
#[command(
    name = "nimon-cli",
    version,
    about = "NIMon CLI - query and operate a NIMon hub"
)]
#[command(propagate_version = true)]
pub struct Cli {
    /// Hub base URL
    #[arg(long, global = true, env = "NIMON_HUB", default_value = DEFAULT_HUB_URL)]
    pub hub: String,

    /// API token sent as `Authorization: Bearer` on write requests
    #[arg(long, global = true, env = "NIMON_API_TOKEN", hide_env_values = true)]
    pub token: Option<String>,

    /// Print the raw JSON response instead of a table (for scripting)
    #[arg(long, global = true)]
    pub json: bool,

    /// Request timeout in seconds
    #[arg(long, global = true, default_value_t = 10, value_name = "SECS")]
    pub timeout: u64,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Hub health plus edge / device / alert / prediction counts
    #[command(alias = "health")]
    Status,
    /// Edge nodes (default: list)
    Edges {
        #[command(subcommand)]
        command: Option<EdgesCommands>,
    },
    /// Devices and their metric history (default: list)
    Devices {
        #[command(subcommand)]
        command: Option<DevicesCommands>,
    },
    /// Alerts (default: list active)
    Alerts {
        #[command(subcommand)]
        command: Option<AlertsCommands>,
    },
    /// Failure predictions (default: list)
    Predictions {
        #[command(subcommand)]
        command: Option<PredictionsCommands>,
    },
    /// Remediation actions (default: list history)
    Actions {
        #[command(subcommand)]
        command: Option<ActionsCommands>,
    },
    /// Effective hub settings (thresholds, auth, per-edge overrides)
    Settings,
}

#[derive(Debug, Subcommand)]
pub enum EdgesCommands {
    /// List live and known-offline edges
    List,
    /// Show one edge
    Show { edge_id: String },
    /// List the devices of one edge
    Devices { edge_id: String },
    /// Push (partial) desired config to an edge; stored when offline
    #[command(group = clap::ArgGroup::new("fields").required(true).multiple(true))]
    PushConfig {
        edge_id: String,
        /// Poll / sweep interval in seconds
        #[arg(long, group = "fields", value_parser = clap::value_parser!(u64).range(1..))]
        poll_interval: Option<u64>,
        /// Temperature warning threshold (C)
        #[arg(long, group = "fields", allow_negative_numbers = true)]
        warning: Option<f64>,
        /// Temperature critical threshold (C)
        #[arg(long, group = "fields", allow_negative_numbers = true)]
        critical: Option<f64>,
    },
}

#[derive(Debug, Subcommand)]
pub enum DevicesCommands {
    /// List devices (live and last-known)
    List {
        /// Only devices of this edge
        #[arg(long)]
        edge: Option<String>,
    },
    /// Metric history of one device (table or CSV export)
    Metrics(MetricsArgs),
}

#[derive(Debug, Args)]
pub struct MetricsArgs {
    pub device_id: String,
    /// Metric name
    #[arg(long, default_value = "temperature")]
    pub metric: String,
    /// Start time (RFC3339)
    #[arg(long, value_parser = parse_rfc3339_arg, conflicts_with = "last")]
    pub since: Option<String>,
    /// Relative window instead of --since, e.g. 30m, 1h, 7d
    #[arg(long, value_parser = parse_duration)]
    pub last: Option<chrono::Duration>,
    /// End time (RFC3339)
    #[arg(long, value_parser = parse_rfc3339_arg)]
    pub until: Option<String>,
    /// Maximum number of points (hub caps at 10000; newest kept)
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: Option<u32>,
    /// Print CSV (timestamp,value) instead of a table
    #[arg(long, conflicts_with = "json")]
    pub csv: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SeverityArg {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum AlertStatusArg {
    Pending,
    Firing,
    Acknowledged,
    Resolved,
    Suppressed,
}

#[derive(Debug, Subcommand)]
pub enum AlertsCommands {
    /// List active alerts (pending, firing, acknowledged)
    List,
    /// Search alert history
    History(HistoryArgs),
    /// Acknowledge an alert (stays active, stops re-notification)
    Ack { alert_id: String },
    /// Resolve an alert
    Resolve { alert_id: String },
}

#[derive(Debug, Args)]
pub struct HistoryArgs {
    #[arg(long, value_enum)]
    pub severity: Option<SeverityArg>,
    #[arg(long)]
    pub edge: Option<String>,
    #[arg(long)]
    pub device: Option<String>,
    #[arg(long, value_enum)]
    pub status: Option<AlertStatusArg>,
    /// Start time (RFC3339)
    #[arg(long, value_parser = parse_rfc3339_arg, conflicts_with = "last")]
    pub since: Option<String>,
    /// Relative window instead of --since, e.g. 24h
    #[arg(long, value_parser = parse_duration)]
    pub last: Option<chrono::Duration>,
    /// End time (RFC3339)
    #[arg(long, value_parser = parse_rfc3339_arg)]
    pub until: Option<String>,
    /// Maximum rows (hub caps at 1000)
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: u32,
}

#[derive(Debug, Subcommand)]
pub enum PredictionsCommands {
    /// Latest active prediction per device and type
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ActionTypeArg {
    #[value(name = "power_cycle", alias = "power-cycle")]
    PowerCycle,
    #[value(name = "reset_driver", alias = "reset-driver")]
    ResetDriver,
    #[value(name = "restart_services", alias = "restart-services")]
    RestartServices,
    #[value(name = "custom_script", alias = "custom-script")]
    CustomScript,
}

impl ActionTypeArg {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionTypeArg::PowerCycle => "power_cycle",
            ActionTypeArg::ResetDriver => "reset_driver",
            ActionTypeArg::RestartServices => "restart_services",
            ActionTypeArg::CustomScript => "custom_script",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ActionsCommands {
    /// Action history (newest first)
    List {
        #[arg(long)]
        device: Option<String>,
        #[arg(long)]
        edge: Option<String>,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..))]
        limit: u32,
    },
    /// Run a remediation action on a device (asks for confirmation)
    Run {
        device_id: String,
        /// power_cycle (param delay_secs), reset_driver, restart_services
        /// (param services), custom_script (param script)
        #[arg(value_enum)]
        action_type: ActionTypeArg,
        /// Action parameter, repeatable: --param services=nidaqmx
        #[arg(long = "param", value_name = "KEY=VALUE", value_parser = parse_key_value)]
        params: Vec<(String, String)>,
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
}

/// Parse `key=value` (value may contain `=`; key must be non-empty).
pub fn parse_key_value(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((k, v)) if !k.trim().is_empty() => Ok((k.trim().to_string(), v.to_string())),
        _ => Err(format!("invalid parameter '{}': expected KEY=VALUE", s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("nimon-cli").chain(args.iter().copied()))
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_anywhere() {
        let cli = parse(&["alerts", "list", "--json", "--hub", "http://h:1"]).unwrap();
        assert!(cli.json);
        assert_eq!(cli.hub, "http://h:1");
        assert!(matches!(
            cli.command,
            Commands::Alerts {
                command: Some(AlertsCommands::List)
            }
        ));
        let cli = parse(&["--token", "t", "health"]).unwrap();
        assert_eq!(cli.token.as_deref(), Some("t"));
        assert!(matches!(cli.command, Commands::Status));
    }

    #[test]
    fn push_config_needs_a_field() {
        assert!(parse(&["edges", "push-config", "e1"]).is_err());
        assert!(parse(&["edges", "push-config", "e1", "--poll-interval", "0"]).is_err());
        let cli = parse(&[
            "edges",
            "push-config",
            "e1",
            "--warning",
            "60",
            "--critical",
            "80",
        ])
        .unwrap();
        match cli.command {
            Commands::Edges {
                command:
                    Some(EdgesCommands::PushConfig {
                        warning,
                        critical,
                        poll_interval,
                        ..
                    }),
            } => {
                assert_eq!(warning, Some(60.0));
                assert_eq!(critical, Some(80.0));
                assert_eq!(poll_interval, None);
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn metrics_args() {
        let cli = parse(&["devices", "metrics", "e:dev#1", "--last", "1h", "--csv"]).unwrap();
        match cli.command {
            Commands::Devices {
                command: Some(DevicesCommands::Metrics(m)),
            } => {
                assert_eq!(m.device_id, "e:dev#1");
                assert_eq!(m.last.unwrap().num_seconds(), 3600);
                assert!(m.csv);
                assert_eq!(m.metric, "temperature");
            }
            other => panic!("unexpected {:?}", other),
        }
        // --since and --last conflict; bad timestamps rejected
        assert!(parse(&[
            "devices",
            "metrics",
            "d",
            "--last",
            "1h",
            "--since",
            "2026-09-24T00:00:00Z"
        ])
        .is_err());
        assert!(parse(&["devices", "metrics", "d", "--since", "yesterday"]).is_err());
        assert!(parse(&["devices", "metrics", "d", "--csv", "--json"]).is_err());
    }

    #[test]
    fn action_run_args() {
        let cli = parse(&[
            "actions",
            "run",
            "e:dev",
            "restart_services",
            "--param",
            "services=a,b",
            "--param",
            "x=1=2",
            "-y",
        ])
        .unwrap();
        match cli.command {
            Commands::Actions {
                command:
                    Some(ActionsCommands::Run {
                        action_type,
                        params,
                        yes,
                        ..
                    }),
            } => {
                assert_eq!(action_type.as_str(), "restart_services");
                assert_eq!(
                    params,
                    vec![
                        ("services".to_string(), "a,b".to_string()),
                        ("x".to_string(), "1=2".to_string())
                    ]
                );
                assert!(yes);
            }
            other => panic!("unexpected {:?}", other),
        }
        assert!(parse(&["actions", "run", "d", "format_disk"]).is_err());
        assert!(parse(&["actions", "run", "d", "power-cycle", "--param", "nokey"]).is_err());
        assert!(parse_key_value("=v").is_err());
    }

    #[test]
    fn history_args() {
        let cli = parse(&[
            "alerts",
            "history",
            "--severity",
            "critical",
            "--status",
            "resolved",
            "--last",
            "24h",
            "--limit",
            "5",
        ])
        .unwrap();
        match cli.command {
            Commands::Alerts {
                command: Some(AlertsCommands::History(h)),
            } => {
                assert!(matches!(h.severity, Some(SeverityArg::Critical)));
                assert!(matches!(h.status, Some(AlertStatusArg::Resolved)));
                assert_eq!(h.limit, 5);
            }
            other => panic!("unexpected {:?}", other),
        }
        assert!(parse(&["alerts", "history", "--severity", "extreme"]).is_err());
    }
}
