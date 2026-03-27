# NIMon - NI Hardware Monitoring Platform

Rust-based monitoring, prediction, and self-healing for National Instruments devices. Tracks PXI, DAQ, VISA, cDAQ, and other NI hardware in real time.

## Architecture

```
Edge Nodes (nimon-edge)              Central Hub (nimon-hub)
┌─────────────────────────┐           ┌──────────────────────────┐
│  DeviceActor (per device) │           │  WebSocket Server       │
│  ├─ NI-SysCfg polling   │           │  ├─ Session management  │
│  └─ Simulated fallback │           │  └─ Message dispatch   │
│  PredictionActor          │  WS/TLS   │  AlertManager            │
│  ├─ EWMA anomaly       │──────────>│  ├─ Rule evaluation    │
│  ├─ Trend analysis     │           │  ├─ Auto-remediation  │
│  └─ Threshold          │           │  └─ DB persistence    │
│  HubConnectorActor        │           │  ActionExecutor         │
│  └─ WebSocket client    │           │  REST API /api/v1/    │
└─────────────────────────┘           └──────────────────────────┘
     nimon-core (shared)                 nimon-ni (FFI)
     ├─ Types, errors                    ├─ NI-SysCfg
     ├─ Actor messages                 ├─ NI-VISA
     ├─ Protocol types                 ├─ NI-DAQmx
     └─ DB repositories                └─ (graceful fallback)
```

## Crates

| Crate | Purpose |
|-------|---------|
| `nimon-core` | Shared types, error handling, DB schema, actor messages, protocol types |
| `nimon-edge` | Edge node: device discovery, health polling, prediction, WebSocket client |
| `nimon-hub` | Central server: WebSocket server, REST API, alert management, action execution |
| `nimon-ni` | FFI bindings to NI APIs (NI-SysCfg, NI-VISA, NI-DAQmx) |
| `nimon-cli` | Admin CLI tool |
| `nimon-sim` | Edge simulator for end-to-end testing without real NI hardware |

## Prerequisites

- Rust 1.75+ (edition 2021)
- [NI drivers](https://www.ni.com/en-us/support/drivers/software-downloads) installed on the machine running `nimon-edge` (optional — falls back to simulated data)

## Build

```bash
cargo build --workspace --release
```

Binaries are produced in `target/release/`:
- `nimon-hub.exe` — Central server
- `nimon-cli.exe` — CLI tool
- `nimon-sim.exe` — Edge simulator for testing

## Run

### Hub Server

Start with defaults (binds `0.0.0.0:8080`, database at `./data/nimon.db`):

```bash
cargo run -p nimon-hub --release
```

With a config file:

```bash
cargo run -p nimon-hub --release -- config/hub.yaml
```

Create `config/hub.yaml`:

```yaml
host: "0.0.0.0"
port: 8080
database_path: "data/nimon.db"
alert:
  default_cooldown_minutes: 5
  max_firing_count: 100
```

A test config is provided at `config/test-hub.yaml` which uses port 9090.

### Edge Node

The edge binary is a library crate (`nimon-edge`) — it has no `main.rs` yet. It is started programmatically or via the CLI (not yet implemented). When running, it:

1. Discovers NI devices via NI-SysCfg (falls back to simulated devices if drivers not installed)
2. Polls each device at configured intervals
3. Runs prediction models (EWMA anomaly detection, trend analysis, thresholds)
4. Connects to the hub via WebSocket and streams status updates

Configuration: copy `config/edge.example.yaml` to `config/edge.yaml` and edit.

### Edge Simulator

For end-to-end testing without real NI hardware, use the edge simulator (`nimon-sim`):

```bash
# Start the hub first (requires config/test-hub.yaml)
cargo run -p nimon-hub --release -- config/test-hub.yaml

# In another terminal, run the simulator
cargo run -p nimon-sim --release
```

The simulator connects to the hub at `ws://localhost:9090/ws` and:
- Simulates 3 edge devices with random temperature (35-85°C) and voltage metrics
- Sends device status updates every 5 seconds
- Occasionally generates prediction alerts (20% chance per cycle)
- Auto-reconnects if the connection drops

Use the CLI to verify the simulator is connected:

```bash
cargo run -p nimon-cli --release -- edges list
```

## REST API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/ws` | GET | WebSocket for edge connections |
| `/api/v1/status` | GET | Connected edges and device counts |
| `/api/v1/alerts` | GET | Active alerts from AlertManager |
| `/api/v1/alerts/{id}/acknowledge` | POST | Acknowledge/resolve an alert |
| `/api/v1/predictions` | GET | Active predictions |
| `/api/v1/edges` | GET | List connected edge nodes |
| `/api/v1/edges/{id}` | GET | Edge node details |
| `/api/v1/edges/{id}/devices` | GET | Devices for an edge node |

## Testing

```bash
cargo test --workspace
```

210 tests across all 5 crates. All tests use simulated data — no NI hardware required.

## Key Features

- **Real NI API integration** with graceful fallback to simulated data when drivers aren't installed
- **Prediction engine**: EWMA anomaly detection, trend prediction, threshold-based alerting
- **Self-healing**: Auto-remediation actions (service restart) triggered by critical alerts
- **Alert rule engine**: Metric thresholds, health status changes, device offline detection, prediction-based rules
- **Multi-channel notifications**: Console, Webhook, Slack, Teams
- **SQLite persistence**: Alerts, predictions, action history
- **WebSocket protocol**: Versioned JSON messages with ack/error, heartbeat, and ping/pong
- **Configurable**: YAML config for hub and edge nodes

## Development

```bash
# Run tests
cargo test --workspace

# Run with debug logging
RUST_LOG=debug cargo test --workspace

# Check formatting
cargo fmt --check --workspace

# Lint
cargo clippy --workspace
```
