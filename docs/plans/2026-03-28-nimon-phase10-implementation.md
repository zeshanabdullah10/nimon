# NIMon Phase 10 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete email alerting, add systemd and Windows service support, and overhaul the README with complete documentation.

**Architecture:** Email uses `lettre` SMTP client. Systemd uses a simple `.service` file. Windows service uses `windows` crate raw API. README consolidates all existing documentation.

**Tech Stack:** lettre 0.11, windows crate, serde_yaml, tokio.

---

## Task 1: Complete Email Alerting

**Files:**
- Modify: `crates/nimon-hub/Cargo.toml`
- Modify: `crates/nimon-hub/src/config.rs:38-43`
- Modify: `crates/nimon-hub/src/alert/notifier.rs:32-35`

### Step 1: Add lettre dependency

Modify `crates/nimon-hub/Cargo.toml` — add after existing dependencies:

```toml
lettre = "0.11"
```

### Step 2: Add email fields to NotificationChannelConfig

Modify `crates/nimon-hub/src/config.rs` — replace `NotificationChannelConfig` struct (lines 38-43):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannelConfig {
    pub channel_type: String,  // "email", "slack", "teams", "webhook", "console"
    pub name: Option<String>,
    pub webhook_url: Option<String>,
    pub smtp_server: Option<String>,
    pub from_addr: Option<String>,
    pub to_addrs: Option<Vec<String>>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool { true }
```

### Step 3: Implement send_email method

Modify `crates/nimon-hub/src/alert/notifier.rs` — replace the TODO email match arm (around line 32-35):

First, add the import at the top:
```rust
use lettre::transport::smtp::TokioSmtpTransport;
use lettre::{Message, SmtpTransport, Transport};
```

Then replace the email match arm:
```rust
ChannelType::Email { smtp_server, from_addr, to_addrs } => {
    if let Err(e) = self.send_email(smtp_server, from_addr, to_addrs, alert).await {
        error!("Email notification failed for {}: {}", channel.id, e);
    }
}
```

Add the `send_email` method to `AlertNotifier` impl:
```rust
async fn send_email(&self, smtp_server: &str, from_addr: &str, to_addrs: &[String], alert: &Alert) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let email = Message::builder()
        .from(from_addr.parse()?)
        .to(to_addrs.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ").parse()?)
        .subject(format!("[{:?}] {}", alert.severity, alert.title))
        .body(alert.message.clone())?;

    let mailer = TokioSmtpTransport::builder_dangerous(smtp_server).build();
    mailer.send(&email).await?;

    Ok(())
}
```

### Step 4: Wire NotificationChannelConfig to ChannelType

Modify `crates/nimon-hub/src/alert/manager.rs` — find where `NotificationChannelConfig` is converted to `NotificationChannel` (around line 326) and add email handling:

```rust
ChannelType::Email { smtp_server, from_addr, to_addrs } => {
    ChannelType::Email {
        smtp_server: smtp_server.unwrap_or_else(|| "localhost".to_string()),
        from_addr: from_addr.unwrap_or_else(|| "nimon@localhost".to_string()),
        to_addrs: to_addrs.unwrap_or_else(|| vec!["admin@localhost".to_string()]),
    }
}
```

Also update the existing webhook, slack, teams arms to handle the new `name` field and use `enabled` instead of `Some(true)`.

### Step 5: Build and verify

Run: `cargo build -p nimon-hub --release 2>&1 | tail -20`
Expected: Compiles without errors

### Step 6: Commit

```bash
git add crates/nimon-hub/Cargo.toml crates/nimon-hub/src/config.rs crates/nimon-hub/src/alert/notifier.rs crates/nimon-hub/src/alert/manager.rs
git commit -m "feat(hub): complete email alerting with SMTP support"
```

---

## Task 2: Systemd Service

**Files:**
- Create: `systemd/nimon-hub.service`

### Step 1: Create systemd service file

Create `systemd/nimon-hub.service`:

```ini
[Unit]
Description=NIMon Hub Server - NI Hardware Monitoring Platform
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info
WorkingDirectory=/opt/nimon
ExecStart=/opt/nimon/nimon-hub /opt/nimon/config/hub.yaml

[Install]
WantedBy=multi-user.target
```

### Step 2: Commit

```bash
git add systemd/nimon-hub.service
git commit -m "feat(hub): add systemd service file for Linux"
```

---

## Task 3: Windows Service

**Files:**
- Modify: `crates/nimon-hub/Cargo.toml`
- Create: `crates/nimon-hub/src/service.rs`
- Modify: `crates/nimon-hub/src/main.rs`

### Step 1: Add windows crate dependency

Modify `crates/nimon-hub/Cargo.toml` — add:

```toml
[target.'cfg(windows)'.dependencies]
windows = { version = "0.58", features = [
    "Win32_Foundation",
    "Win32_System_Services",
    "Win32_System_Threading",
] }
```

### Step 2: Create service.rs

Create `crates/nimon-hub/src/service.rs`:

```rust
//! Windows Service support for nimon-hub

#[cfg(windows)]
use windows::Win32::Foundation::*;
#[cfg(windows)]
use windows::Win32::System::Services::*;

#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
pub fn run_as_service() -> Result<(), Box<dyn std::error::Error>> {
    // This would be called when --service flag is passed
    // For now, install/uninstall are separate commands
    Ok(())
}

#[cfg(windows)]
pub fn install_service(service_name: &str, display_name: &str, exe_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let sc_manager = OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_ALL_ACCESS)?;
        let exe_path_wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
        let service_name_wide: Vec<u16> = service_name.encode_utf16().chain(std::iter::once(0)).collect();
        let display_name_wide: Vec<u16> = display_name.encode_utf16().chain(std::iter::once(0)).collect();

        let handle = CreateServiceW(
            sc_manager,
            windows::PCWSTR::from_raw(service_name_wide.as_ptr()),
            windows::PCWSTR::from_raw(display_name_wide.as_ptr()),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            windows::PCWSTR::from_raw(exe_path_wide.as_ptr()),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
        )?;

        CloseServiceHandle(handle)?;
        CloseServiceHandle(sc_manager)?;
    }
    Ok(())
}

#[cfg(windows)]
pub fn uninstall_service(service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let sc_manager = OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_ALL_ACCESS)?;
        let service_name_wide: Vec<u16> = service_name.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = OpenServiceW(sc_manager, windows::PCWSTR::from_raw(service_name_wide.as_ptr()), DELETE)?;
        DeleteService(handle)?;
        CloseServiceHandle(handle)?;
        CloseServiceHandle(sc_manager)?;
    }
    Ok(())
}

#[cfg(windows)]
pub fn start_service(service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let sc_manager = OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_ALL_ACCESS)?;
        let service_name_wide: Vec<u16> = service_name.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = OpenServiceW(sc_manager, windows::PCWSTR::from_raw(service_name_wide.as_ptr()), SERVICE_ALL_ACCESS)?;
        StartServiceW(handle, None)?;
        CloseServiceHandle(handle)?;
        CloseServiceHandle(sc_manager)?;
    }
    Ok(())
}

#[cfg(windows)]
pub fn stop_service(service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let sc_manager = OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_ALL_ACCESS)?;
        let service_name_wide: Vec<u16> = service_name.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = OpenServiceW(sc_manager, windows::PCWSTR::from_raw(service_name_wide.as_ptr()), SERVICE_ALL_ACCESS)?;
        let mut status = SERVICE_STATUS::default();
        ControlService(handle, SERVICE_CONTROL_STOP, &mut status)?;
        CloseServiceHandle(handle)?;
        CloseServiceHandle(sc_manager)?;
    }
    Ok(())
}
```

### Step 3: Add service CLI commands to main.rs

Modify `crates/nimon-hub/src/main.rs` — add CLI argument parsing near the top:

```rust
#[derive(clap::Subcommand)]
enum Commands {
    Install,
    Uninstall,
    Start,
    Stop,
}
```

Update the main match:

```rust
#[derive(clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    config_path: Option<PathBuf>,
}

match cli.command {
    #[cfg(windows)]
    Some(Commands::Install) => {
        nimon_hub::service::install_service("NIMonHub", "NIMon Hub Server", &std::env::current_exe()?.display().to_string())?;
        println!("Service installed");
        return Ok(());
    }
    #[cfg(windows)]
    Some(Commands::Uninstall) => {
        nimon_hub::service::uninstall_service("NIMonHub")?;
        println!("Service uninstalled");
        return Ok(());
    }
    #[cfg(windows)]
    Some(Commands::Start) => {
        nimon_hub::service::start_service("NIMonHub")?;
        println!("Service started");
        return Ok(());
    }
    #[cfg(windows)]
    Some(Commands::Stop) => {
        nimon_hub::service::stop_service("NIMonHub")?;
        println!("Service stopped");
        return Ok(());
    }
    None => {}
    _ => {}
}
```

Also add `clap = "4"` to `nimon-hub/Cargo.toml` dependencies.

### Step 4: Build and verify

Run: `cargo build -p nimon-hub --release 2>&1 | tail -20`
Expected: Compiles without errors

### Step 5: Commit

```bash
git add crates/nimon-hub/Cargo.toml crates/nimon-hub/src/service.rs crates/nimon-hub/src/main.rs
git commit -m "feat(hub): add Windows service support with install/uninstall/start/stop commands"
```

---

## Task 4: README Overhaul

**Files:**
- Modify: `README.md`

### Step 1: Replace README with complete documentation

Replace the entire content of `README.md` with the following. This consolidates all documentation from phases 1-9.

```markdown
# NIMon - NI Hardware Monitoring Platform

Rust-based monitoring, prediction, and self-healing for National Instruments hardware. Monitors PXI, DAQ, VISA, cDAQ, and other NI devices in real time.

## Architecture

```
Edge Nodes (nimon-edge)              Central Hub (nimon-hub)
┌─────────────────────────┐           ┌──────────────────────────┐
│  DeviceActor (per device) │           │  WebSocket Server       │
│  ├─ NI-SysCfg polling   │           │  ├─ Session management  │
│  └─ Simulated fallback  │           │  └─ Message dispatch    │
│  PredictionActor         │  WS/TLS   │  AlertManager           │
│  ├─ EWMA anomaly        │───────────│  ├─ Rule evaluation     │
│  ├─ Trend analysis      │           │  ├─ Auto-remediation   │
│  └─ Threshold           │           │  └─ DB persistence     │
│  HubConnectorActor       │           │  ActionExecutor          │
│  └─ WebSocket client    │           │  REST API /api/v1/      │
└─────────────────────────┘           └──────────────────────────┘
     nimon-core (shared)                  nimon-ni (FFI)
     ├─ Types, errors                     ├─ NI-SysCfg
     ├─ Actor messages                    ├─ NI-VISA
     ├─ Protocol types                   ├─ NI-DAQmx
     └─ DB repositories                  └─ (graceful fallback)
```

## Quick Start

```bash
# 1. Build everything
cargo build --workspace --release

# 2. Start the hub (terminal 1)
./target/release/nimon-hub.exe config/test-hub.yaml

# 3. Start the simulator (terminal 2)
./target/release/nimon-sim.exe

# 4. Check the dashboard
open http://localhost:9090

# 5. Use the CLI
./target/release/nimon-cli.exe edges list
./target/release/nimon-cli.exe alerts list
```

## Crates

| Crate | Purpose |
|-------|---------|
| `nimon-core` | Shared types, errors, DB schema, actor messages, protocol |
| `nimon-edge` | Edge node: NI device discovery, health polling, prediction |
| `nimon-hub` | Central server: WebSocket, REST API, alerts, actions |
| `nimon-ni` | FFI bindings to NI-SysCfg, NI-VISA, NI-DAQmx |
| `nimon-cli` | Admin CLI for edges, alerts, health |
| `nimon-sim` | Edge simulator for end-to-end testing |

## Prerequisites

- Rust 1.75+ (edition 2021)
- [NI drivers](https://www.ni.com/en-us/support/drivers/software-downloads) on machines running `nimon-edge` (optional — falls back to simulated data)

## Build

```bash
cargo build --workspace --release
```

Binaries are produced in `target/release/`:

| Binary | Description |
|--------|-------------|
| `nimon-hub.exe` | Central server |
| `nimon-cli.exe` | Admin CLI |
| `nimon-sim.exe` | Edge simulator |

## Configuration

Create `config/hub.yaml`:

```yaml
# Network
host: "0.0.0.0"
port: 9090

# Database
database_path: "data/nimon.db"

# Alerting
alert:
  default_cooldown_minutes: 5
  max_firing_count: 100

  # Notification channels (see Notification Channels section)
  notification_channels: []

  # Alert rules (see Alert Rules section)
  rules: []
```

### Alert Rules

```yaml
rules:
  - name: "High Temperature"
    severity: "critical"
    condition:
      MetricThreshold:
        metric: "temperature"
        threshold: 75.0
        comparison: "greater_than"
    cooldown_minutes: 5
    notification_channels: ["slack-ops"]

  - name: "Device Offline"
    severity: "warning"
    condition:
      DeviceOffline:
        max_minutes_since_poll: 10
```

Condition types: `MetricThreshold`, `DeviceOffline`, `HealthStatusChange`

Metric comparisons: `gt`, `lt`, `eq`, `ne`, `ge`, `le`

Severities: `info`, `warning`, `critical`

### Notification Channels

Configure multiple channels in `hub.yaml`:

```yaml
alert:
  notification_channels:
    # Console (always available)
    - channel_type: "console"
      enabled: true

    # Slack
    - channel_type: "slack"
      webhook_url: "https://hooks.slack.com/services/XXX/YYY/ZZZ"
      enabled: true

    # Microsoft Teams
    - channel_type: "teams"
      webhook_url: "https://outlook.office.com/webhook/XXX"
      enabled: true

    # Generic webhook
    - channel_type: "webhook"
      webhook_url: "https://example.com/webhook"
      enabled: true

    # Email (SMTP)
    - channel_type: "email"
      smtp_server: "smtp.example.com:587"
      from_addr: "nimon@example.com"
      to_addrs: ["ops@example.com", "admin@example.com"]
      enabled: true
```

Rules can specify which channels to use:

```yaml
rules:
  - name: "Critical Temperature"
    notification_channels: ["slack-ops", "email"]
    ...
```

## REST API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check with connected edge count |
| `/ws` | GET | WebSocket for edge connections |
| `/` | GET | Web dashboard |
| `/style.css` | GET | Dashboard stylesheet |
| `/app.js` | GET | Dashboard JavaScript |
| `/api/v1/status` | GET | Connected edges and device counts |
| `/api/v1/alerts` | GET | Active alerts |
| `/api/v1/alerts/history` | GET | Alert history (query: `?limit=100&severity=critical`) |
| `/api/v1/alerts/:id/acknowledge` | POST | Acknowledge/resolve an alert |
| `/api/v1/predictions` | GET | Active predictions |
| `/api/v1/edges` | GET | List connected edges |
| `/api/v1/edges/:id` | GET | Edge node details |
| `/api/v1/edges/:id/devices` | GET | Devices for an edge |

## CLI Tool

```bash
# Health check
nimon-cli health

# Status summary
nimon-cli status

# List edges
nimon-cli edges list

# Edge details
nimon-cli edges show <edge_id>

# List active alerts
nimon-cli alerts list

# Acknowledge an alert
nimon-cli alerts ack <alert_id>
```

## Edge Simulator

For testing without real NI hardware:

```bash
cargo run -p nimon-sim --release
```

Simulates 3 edge devices with:
- Temperature: 35-85°C (triggers alerts > 75°C)
- Voltage 5V: 4.8-5.2V
- Voltage 3.3V: 3.1-3.5V
- 20% chance of prediction per cycle
- Auto-reconnects on disconnect

## Service Installation

### Linux (systemd)

```bash
# Install service file
sudo cp systemd/nimon-hub.service /etc/systemd/system/

# Reload systemd
sudo systemctl daemon-reload

# Enable (start on boot)
sudo systemctl enable nimon-hub

# Start now
sudo systemctl start nimon-hub

# Check status
sudo systemctl status nimon-hub

# View logs
journalctl -u nimon-hub -f
```

### Windows

```bash
# Install as Windows service (run as Administrator)
nimon-hub.exe install

# Start the service
nimon-hub.exe start

# Stop the service
nimon-hub.exe stop

# Uninstall the service
nimon-hub.exe uninstall
```

## Key Features

- **NI Hardware Integration** — NI-SysCfg, NI-VISA, NI-DAQmx with graceful simulated fallback
- **Prediction Engine** — EWMA anomaly detection, trend analysis, threshold-based
- **Self-Healing** — Auto-remediation actions on critical alerts
- **Multi-Channel Alerts** — Console, Slack, Teams, Webhook, Email (SMTP)
- **SQLite Persistence** — Alerts, predictions, and action history
- **WebSocket Protocol** — Versioned JSON with ack/error, heartbeat, ping/pong
- **Configurable** — YAML config with rule engine and per-rule notification channels
- **Edge Simulator** — End-to-end testing without hardware

## Development

```bash
# Run all tests
cargo test --workspace

# With debug logging
RUST_LOG=debug cargo test --workspace

# Format
cargo fmt --workspace

# Lint
cargo clippy --workspace --fix
```
```

### Step 2: Commit

```bash
git add README.md
git commit -m "docs: complete README overhaul with all features documented"
```

---

## Task 5: Final Build Verification

### Step 1: Run full test suite

Run: `cargo test --workspace 2>&1 | tail -15`
Expected: All tests pass

### Step 2: Verify workspace builds clean

Run: `cargo build --workspace --release 2>&1 | tail -10`
Expected: All crates build without errors or warnings

### Step 3: Commit

```bash
git add -A
git commit -m "chore: final verification - all tests pass, workspace builds clean"
```
