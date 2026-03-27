# NIMon Phase 8: CLI Tool

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a command-line interface for NIMon hub management. Operators can query status, manage edges, acknowledge alerts, and trigger actions from the terminal.

**Tech Stack:** Rust (clap for CLI parsing), reqwest (HTTP client)

---

## Design: Unix-Inspired Developer Tool

**Aesthetic**: Like `kubectl` or `gh` — structured output, human-readable by default, JSON output for scripting. Subcommands for different resource types.

**Commands**:
```
nimon edges list              # List connected edge nodes
nimon edges show <edge-id>    # Show edge details
nimon alerts list             # List active alerts
nimon alerts ack <alert-id>   # Acknowledge an alert
nimon health                  # Show system health
nimon status                  # Show hub status
```

---

## Task 1: Initialize nimon-cli Crate

**Files:**
- Modify: `Cargo.toml` (workspace)
- Modify: `crates/nimon-cli/Cargo.toml`
- Create: `crates/nimon-cli/src/main.rs`
- Create: `crates/nimon-cli/src/cli.rs`

**Step 1: Check existing nimon-cli structure**
```bash
ls crates/nimon-cli/
```

**Step 2: Update workspace Cargo.toml**

Add nimon-cli if not present:
```toml
members = [
    ...
    "crates/nimon-cli",
]
```

**Step 3: Write Cargo.toml for nimon-cli**

```toml
[package]
name = "nimon-cli"
version.workspace = true
edition.workspace = true

[dependencies]
clap = { version = "4", features = ["derive"] }
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
anyhow = "1"
```

---

## Task 2: CLI Structure with Clap

**Files:**
- Create: `crates/nimon-cli/src/cli.rs`

**Step 1: Define CLI structure**

```rust
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
```

---

## Task 3: API Client Module

**Files:**
- Create: `crates/nimon-cli/src/api.rs`

**Step 1: Implement API client**

```rust
use anyhow::Result;
use serde::Deserialize;

pub struct Client {
    base_url: String,
    client: reqwest::Client,
}

impl Client {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn health(&self) -> Result<HealthResponse> {
        Ok(self.client.get(format!("{}/health", self.base_url))
            .send().await?
            .json().await?)
    }

    pub async fn edges(&self) -> Result<EdgesResponse> {
        Ok(self.client.get(format!("{}/api/v1/edges", self.base_url))
            .send().await?
            .json().await?)
    }

    pub async fn edge(&self, edge_id: &str) -> Result<EdgeDetailResponse> {
        Ok(self.client.get(format!("{}/api/v1/edges/{}", self.base_url, edge_id))
            .send().await?
            .json().await?)
    }

    pub async fn alerts(&self) -> Result<AlertsResponse> {
        Ok(self.client.get(format!("{}/api/v1/alerts", self.base_url))
            .send().await?
            .json().await?)
    }

    pub async fn ack_alert(&self, alert_id: &str) -> Result<()> {
        self.client.post(format!("{}/api/v1/alerts/{}/acknowledge", self.base_url, alert_id))
            .send().await?;
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub connected_edges: usize,
}

#[derive(Deserialize)]
pub struct EdgesResponse {
    pub edges: Vec<EdgeInfo>,
    pub total: usize,
}

#[derive(Deserialize)]
pub struct EdgeInfo {
    pub edge_id: String,
    pub name: String,
    pub connected_at: String,
    pub device_count: usize,
}

#[derive(Deserialize)]
pub struct EdgeDetailResponse {
    pub edge_id: String,
    pub name: String,
    pub hostname: String,
    pub ip_address: String,
    pub connected_at: String,
    pub device_count: usize,
    pub status: String,
}

#[derive(Deserialize)]
pub struct AlertsResponse {
    pub alerts: Vec<AlertInfo>,
    pub total: usize,
}

#[derive(Deserialize)]
pub struct AlertInfo {
    pub id: String,
    pub device_id: Option<String>,
    pub edge_id: Option<String>,
    pub severity: String,
    pub title: String,
    pub message: String,
    pub status: String,
    pub triggered_at: String,
}
```

---

## Task 4: Main Entry Point

**Files:**
- Create: `crates/nimon-cli/src/main.rs`

**Step 1: Implement main**

```rust
mod api;
mod cli;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands, EdgesCommands, AlertsCommands};

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
        Commands::Edges { command } => {
            match command {
                Some(EdgesCommands::List) => {
                    let edges = client.edges().await?;
                    println!("{:<20} {:<30} {:>10}", "EDGE ID", "NAME", "DEVICES");
                    println!("{}", "-".repeat(62));
                    for edge in edges.edges {
                        println!("{:<20} {:<30} {:>10}",
                            edge.edge_id, edge.name, edge.device_count);
                    }
                }
                Some(EdgesCommands::Show { edge_id }) => {
                    let edge = client.edge(&edge_id).await?;
                    println!("Edge Details");
                    println!("  ID: {}", edge.edge_id);
                    println!("  Name: {}", edge.name);
                    println!("  Hostname: {}", edge.hostname);
                    println!("  IP: {}", edge.ip_address);
                    println!("  Status: {}", edge.status);
                    println!("  Devices: {}", edge.device_count);
                    println!("  Connected: {}", edge.connected_at);
                }
                None => {
                    let edges = client.edges().await?;
                    println!("{:<20} {:<30} {:>10}", "EDGE ID", "NAME", "DEVICES");
                    println!("{}", "-".repeat(62));
                    for edge in edges.edges {
                        println!("{:<20} {:<30} {:>10}",
                            edge.edge_id, edge.name, edge.device_count);
                    }
                }
            }
        }
        Commands::Alerts { command } => {
            match command {
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
                        println!("{:<12} {:<20} {:<40}",
                            alert.severity.to_uppercase(), title, msg);
                    }
                }
                Some(AlertsCommands::Ack { alert_id }) => {
                    client.ack_alert(&alert_id).await?;
                    println!("Alert {} acknowledged", alert_id);
                }
            }
        }
    }

    Ok(())
}
```

---

## Task 5: Build and Test

**Step 1: Build CLI**
```bash
cargo build -p nimon-cli --release
```

**Step 2: Test with running hub**
```bash
# In one terminal, start hub
cargo run -p nimon-hub --release -- config/test-hub.yaml

# In another terminal, test CLI
./target/release/nimon.exe health
./target/release/nimon.exe edges list
./target/release/nimon.exe alerts list
```

Expected: CLI connects to hub and displays formatted output.

---

## Commit

```bash
git add crates/nimon-cli/
git commit -m "feat(cli): add nimon CLI tool for hub management"
```
