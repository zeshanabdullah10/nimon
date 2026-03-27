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
│  └─ WebSocket client    │           │  REST API /api/v1/     │
└─────────────────────────┘           └──────────────────────────┘
     nimon-core (shared)                  nimon-ni (FFI)
     ├─ Types, errors                     ├─ NI-SysCfg
     ├─ Actor messages                    ├─ NI-VISA
     ├─ Protocol types                    ├─ NI-DAQmx
     └─ DB repositories                   └─ (graceful fallback)
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
      smtp_server: "smtp.example.com"
      smtp_port: 587
      from_addr: "nimon@example.com"
      to_addrs: ["ops@example.com", "admin@example.com"]
      smtp_skip_tls_verify: false
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
