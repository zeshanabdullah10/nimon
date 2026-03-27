use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "nimon")]
#[command(version = "0.1.0")]
#[command(about = "NIMon CLI - Hardware monitoring management tool")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Show system health
    Health,
    /// Show hub status
    Status,
    /// List connected edge nodes
    Edges {
        #[command(subcommand)]
        command: Option<EdgesCommands>,
    },
    /// Manage alerts
    Alerts {
        #[command(subcommand)]
        command: Option<AlertsCommands>,
    },
}

#[derive(Subcommand)]
pub enum EdgesCommands {
    /// List all connected edges
    List,
    /// Show details for an edge
    Show { edge_id: String },
}

#[derive(Subcommand)]
pub enum AlertsCommands {
    /// List active alerts
    List,
    /// Acknowledge an alert
    Ack { alert_id: String },
}
