//! NIMon CLI Application
//!
//! Command-line interface for NIMon hardware monitoring.

mod api;
mod cli;

use anyhow::Result;
use clap::Parser;
use cli::{AlertsCommands, Cli, Commands, EdgesCommands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = api::Client::new("http://localhost:9090");

    match cli.command {
        Commands::Health => {
            let health = client.health().await?;
            println!("Status: {}", health.status);
            println!("Version: {}", health.version);
            println!("Connected Edges: {}", health.connected_edges);
        }
        Commands::Status => {
            let edges = client.edges().await?;
            println!("Total Edges: {}", edges.total);
            for edge in edges.edges {
                println!("  - {} ({})", edge.name, edge.edge_id);
            }
        }
        Commands::Edges { command } => match command {
            Some(EdgesCommands::List) => {
                let edges = client.edges().await?;
                println!("{:<20} {:<30} {:>10}", "EDGE ID", "NAME", "DEVICES");
                println!("{}", "-".repeat(62));
                for edge in edges.edges {
                    println!(
                        "{:<20} {:<30} {:>10}",
                        edge.edge_id,
                        edge.name,
                        edge.device_count
                            .map(|d| d.to_string())
                            .unwrap_or_else(|| "-".into())
                    );
                }
            }
            Some(EdgesCommands::Show { edge_id }) => {
                let edge = client.edge(&edge_id).await?;
                println!("Edge Details");
                println!("  ID: {}", edge.edge_id);
                println!("  Name: {}", edge.name);
                println!("  Hostname: {}", edge.hostname.as_deref().unwrap_or("-"));
                println!("  IP: {}", edge.ip_address.as_deref().unwrap_or("-"));
                println!("  Status: {}", edge.status);
                println!(
                    "  Devices: {}",
                    edge.device_count
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| "-".into())
                );
                println!(
                    "  Connected: {}",
                    edge.connected_at
                        .as_deref()
                        .or(edge.last_seen.as_deref())
                        .unwrap_or("-")
                );
            }
            None => {
                let edges = client.edges().await?;
                println!("{:<20} {:<30} {:>10}", "EDGE ID", "NAME", "DEVICES");
                println!("{}", "-".repeat(62));
                for edge in edges.edges {
                    println!(
                        "{:<20} {:<30} {:>10}",
                        edge.edge_id,
                        edge.name,
                        edge.device_count
                            .map(|d| d.to_string())
                            .unwrap_or_else(|| "-".into())
                    );
                }
            }
        },
        Commands::Alerts { command } => match command {
            Some(AlertsCommands::List) | None => {
                let alerts = client.alerts().await?;
                if alerts.alerts.is_empty() {
                    println!("No active alerts");
                    return Ok(());
                }
                println!("{:<12} {:<20} {:<40}", "SEVERITY", "TITLE", "MESSAGE");
                println!("{}", "-".repeat(74));
                for alert in alerts.alerts {
                    let title = if alert.title.len() > 20 {
                        format!("{}...", &alert.title[..17])
                    } else {
                        alert.title
                    };
                    let msg = if alert.message.len() > 40 {
                        format!("{}...", &alert.message[..37])
                    } else {
                        alert.message
                    };
                    println!(
                        "{:<12} {:<20} {:<40}",
                        alert.severity.to_uppercase(),
                        title,
                        msg
                    );
                }
            }
            Some(AlertsCommands::Ack { alert_id }) => {
                client.ack_alert(&alert_id).await?;
                println!("Alert {} acknowledged", alert_id);
            }
        },
    }

    Ok(())
}
