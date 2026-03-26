# NIMon - NI Hardware Monitoring Platform

**Design Document** | Version 1.0 | 2026-03-26

## Overview

NIMon is a Rust-based hardware monitoring platform for National Instruments devices. It provides real-time monitoring, predictive failure detection, and self-healing capabilities through a hybrid edge-to-hub architecture.

### Key Features

- **Comprehensive NI Device Coverage**: DAQ, PXI, cDAQ, VISA instruments, XNET, GPIB, power supplies
- **Predictive Maintenance**: ML-based failure prediction before issues occur
- **Self-Healing**: Automated remediation actions (power cycle, service restart, config restore)
- **Real-Time Dashboard**: Leptos-based web UI with live WebSocket updates
- **Multi-Channel Alerts**: Email, Slack, Teams, SMS, webhooks
- **Hybrid Architecture**: Edge collectors + central hub aggregation
- **Lightweight**: SQLite database, single binary deployment

### USP vs NI SystemLink

| Feature | NI SystemLink | NIMon |
|---------|---------------|-------|
| Deployment | Requires NI server infrastructure (~$10K+) | Single binary, runs anywhere |
| License | Commercial, per-seat | Open source, free |
| Resource usage | Heavy (Windows Server + SQL Server) | Lightweight (<50MB RAM) |
| Customization | Limited to NI's extensions | Full control |
| Offline/Edge | Requires server connection | Works fully offline |
| Cross-platform | Windows Server only | Windows, Linux |

---

## Architecture

### High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         NIMon PLATFORM                               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────────────┐     ┌──────────────────┐                      │
│  │  EDGE NODE A     │     │  EDGE NODE B     │      ... N nodes     │
│  │  (nimon-edge)    │     │  (nimon-edge)    │                      │
│  │                  │     │                  │                      │
│  │  ┌────────────┐  │     │  ┌────────────┐  │                      │
│  │  │ Device     │  │     │  │ Device     │  │                      │
│  │  │ Actors     │  │     │  │ Actors     │  │                      │
│  │  │            │  │     │  │            │  │                      │
│  │  │ PXI, cDAQ, │  │     │  │ DAQ, VISA, │  │                      │
│  │  │ GPIB, etc. │  │     │  │ XNET, etc. │  │                      │
│  │  └────────────┘  │     │  └────────────┘  │                      │
│  │  ┌────────────┐  │     │  ┌────────────┐  │                      │
│  │  │ Prediction │  │     │  │ Prediction │  │                      │
│  │  │ Actor      │  │     │  │ Actor      │  │                      │
│  │  └────────────┘  │     │  └────────────┘  │                      │
│  │  ┌────────────┐  │     │  ┌────────────┐  │                      │
│  │  │ WS Client  │  │     │  │ WS Client  │  │                      │
│  │  │ Actor      │  │     │  │ Actor      │  │                      │
│  │  └────────────┘  │     │  └────────────┘  │                      │
│  │   SQLite        │     │   SQLite        │                      │
│  └────────┬─────────┘     └────────┬─────────┘                      │
│           │                        │                                 │
│           │     WebSocket (TLS)    │                                 │
│           └────────────┬───────────┘                                 │
│                        ▼                                             │
│           ┌────────────────────────────┐                             │
│           │   CENTRAL SERVER           │                             │
│           │   (nimon-hub)              │                             │
│           │                            │                             │
│           │  ┌──────────────────────┐  │                             │
│           │  │ Edge Connection Hub  │  │                             │
│           │  │ (WebSocket Server)   │  │                             │
│           │  └──────────────────────┘  │                             │
│           │  ┌──────────────────────┐  │                             │
│           │  │ Aggregation Engine   │  │                             │
│           │  └──────────────────────┘  │                             │
│           │  ┌──────────────────────┐  │                             │
│           │  │ Alert Manager        │  │                             │
│           │  └──────────────────────┘  │                             │
│           │  ┌──────────────────────┐  │                             │
│           │  │ Action Executor      │  │                             │
│           │  │ (Self-Healing)       │  │                             │
│           │  └──────────────────────┘  │                             │
│           │  ┌──────────────────────┐  │                             │
│           │  │ Leptos Web UI        │  │                             │
│           │  │ + REST API           │  │                             │
│           │  └──────────────────────┘  │                             │
│           │        SQLite              │                             │
│           └────────────────────────────┘                             │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Binary Structure

| Binary | Purpose | Deployment |
|--------|---------|------------|
| `nimon-edge` | Edge collector, runs on each test system | Windows/Linux |
| `nimon-hub` | Central aggregation server | Windows Server/Linux |
| `nimon-cli` | Command-line admin tool | Any |

### Tech Stack

| Component | Technology |
|-----------|------------|
| Runtime | Tokio + Actix Actor System |
| Web Server | Axum (for REST + WebSocket) |
| Frontend | Leptos (Rust SPA with SSR) |
| Database | SQLite + SQLx |
| NI Integration | FFI bindings to NI C APIs |
| ML/Prediction | `linfa` or `smartcore` (Rust ML crates) |

---

## Edge Node Design

### Actor Hierarchy

```
┌───────────────────────────────────────────────────────────────┐
│                    EDGE SUPERVISOR                             │
│                    (Root Actor)                                │
│                                                                │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │  DEVICE LAYER SUPERVISOR                                 │  │
│  │                                                          │  │
│  │   ┌──────────┐  ┌──────────┐  ┌──────────┐              │  │
│  │   │ DAQ      │  │ PXI      │  │ VISA     │  ...         │  │
│  │   │ Actor    │  │ Actor    │  │ Actor    │              │  │
│  │   │          │  │          │  │          │              │  │
│  │   │ ni-daqmx │  │ ni-syscfg│  │ ni-visa  │              │  │
│  │   └──────────┘  └──────────┘  └──────────┘              │  │
│  └─────────────────────────────────────────────────────────┘  │
│                                                                │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │  ANALYSIS LAYER SUPERVISOR                               │  │
│  │                                                          │  │
│  │   ┌──────────────┐  ┌──────────────┐                     │  │
│  │   │ Prediction   │  │ Anomaly      │                     │  │
│  │   │ Actor        │  │ Detector     │                     │  │
│  │   │              │  │ Actor        │                     │  │
│  │   └──────────────┘  └──────────────┘                     │  │
│  └─────────────────────────────────────────────────────────┘  │
│                                                                │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │  COMMUNICATION LAYER SUPERVISOR                          │  │
│  │                                                          │  │
│  │   ┌──────────────┐  ┌──────────────┐                     │  │
│  │   │ WebSocket    │  │ Config       │                     │  │
│  │   │ Client Actor │  │ Manager      │                     │  │
│  │   └──────────────┘  └──────────────┘                     │  │
│  └─────────────────────────────────────────────────────────┘  │
│                                                                │
└───────────────────────────────────────────────────────────────┘
```

### Device Actor Types

| Actor Type | NI API | Monitored Metrics |
|------------|--------|-------------------|
| `DaqDeviceActor` | NI-DAQmx | Temperature, self-test status, connection state |
| `PxiDeviceActor` | NI-SysCfg | Module inventory, temperatures, voltages, fan speeds |
| `VisaDeviceActor` | NI-VISA | Instrument connectivity, timeout errors, IDN response |
| `XnetDeviceActor` | NI-XNET | CAN/LIN bus load, error frames, transceiver status |
| `GpibDeviceActor` | NI-488.2 | Controller status, device addresses, errors |
| `PowerSupplyActor` | NIPCAL | Output voltage/current, protection status |

### Polling Configuration

| Device Type | Default Poll Interval | Configurable |
|-------------|----------------------|--------------|
| DAQ devices | 5 seconds | Yes |
| PXI chassis | 10 seconds | Yes |
| VISA instruments | 15 seconds | Yes |
| XNET interfaces | 2 seconds | Yes |
| GPIB controllers | 30 seconds | Yes |

### Edge Node Responsibilities

1. **Device Discovery** - Auto-detect NI hardware on startup via NI-SysCfg
2. **Health Polling** - Query each device at configured intervals
3. **Local Prediction** - Run anomaly detection locally (reduces hub load)
4. **Buffering** - Store data locally if connection to hub is lost
5. **Reconnection** - Auto-reconnect to hub with exponential backoff
6. **Config Sync** - Receive config updates from hub via WebSocket

---

## Central Server Design

### Actor Hierarchy

```
┌───────────────────────────────────────────────────────────────────┐
│                       HUB SUPERVISOR (Root)                        │
│                                                                    │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │  CONNECTION LAYER SUPERVISOR                                 │  │
│  │   ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │  │
│  │   │ WS Server    │  │ Edge Session │  │ Auth         │      │  │
│  │   │ Actor        │  │ Actors (1/N) │  │ Manager      │      │  │
│  │   └──────────────┘  └──────────────┘  └──────────────┘      │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                    │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │  PROCESSING LAYER SUPERVISOR                                 │  │
│  │   ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │  │
│  │   │ Aggregator   │  │ Trend        │  │ Correlation  │      │  │
│  │   │ Actor        │  │ Analyzer     │  │ Engine       │      │  │
│  │   └──────────────┘  └──────────────┘  └──────────────┘      │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                    │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │  ACTION LAYER SUPERVISOR                                     │  │
│  │   ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │  │
│  │   │ Alert        │  │ Action       │  │ Escalation   │      │  │
│  │   │ Dispatcher   │  │ Executor     │  │ Manager      │      │  │
│  │   └──────────────┘  └──────────────┘  └──────────────┘      │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                    │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │  API LAYER SUPERVISOR                                        │  │
│  │   ┌──────────────┐  ┌──────────────┐                         │  │
│  │   │ REST API     │  │ Leptos UI    │                         │  │
│  │   │ Handler      │  │ Server       │                         │  │
│  │   └──────────────┘  └──────────────┘                         │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                    │
└───────────────────────────────────────────────────────────────────┘
```

### WebSocket Message Protocol

**Edge → Hub: Device Status Update**
```json
{
  "type": "device_status",
  "edge_id": "test-cell-01",
  "timestamp": "2026-03-26T01:30:00Z",
  "devices": [
    {
      "device_id": "PXI1Slot2",
      "device_type": "daq",
      "status": "healthy",
      "metrics": {
        "temperature": 45.2,
        "self_test": "passed"
      }
    }
  ],
  "predictions": [
    {
      "device_id": "cDAQ-9178",
      "type": "overheating",
      "probability": 0.78,
      "eta_minutes": 120
    }
  ]
}
```

**Hub → Edge: Configuration Update**
```json
{
  "type": "config_update",
  "poll_intervals": { "daq": 3, "pxi": 5 },
  "thresholds": { "temperature_warning": 65, "temperature_critical": 75 },
  "actions_enabled": true
}
```

**Hub → Edge: Remote Action Command**
```json
{
  "type": "execute_action",
  "action_id": "action-123",
  "target_device": "cDAQ-9178",
  "action": "reset_connection"
}
```

---

## Data Models

### Database Schema

```sql
-- Edge nodes
CREATE TABLE edge_nodes (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    hostname        TEXT,
    ip_address      TEXT,
    last_seen       TIMESTAMP,
    status          TEXT DEFAULT 'offline',
    config          TEXT,
    created_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- All discovered devices
CREATE TABLE devices (
    id              TEXT PRIMARY KEY,
    edge_id         TEXT NOT NULL REFERENCES edge_nodes(id),
    device_name     TEXT NOT NULL,
    device_type     TEXT NOT NULL,
    model           TEXT,
    serial_number   TEXT,
    firmware_version TEXT,
    driver_version  TEXT,
    ip_address      TEXT,
    slot            INTEGER,
    chassis         TEXT,
    config          TEXT,
    created_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(edge_id, device_name)
);

-- Current device status
CREATE TABLE device_status (
    device_id       TEXT PRIMARY KEY REFERENCES devices(id),
    status          TEXT NOT NULL,
    last_poll       TIMESTAMP,
    metrics         TEXT,
    error_message   TEXT,
    error_count     INTEGER DEFAULT 0,
    uptime_seconds  BIGINT DEFAULT 0,
    updated_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Time-series metrics
CREATE TABLE device_metrics_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id       TEXT NOT NULL REFERENCES devices(id),
    timestamp       TIMESTAMP NOT NULL,
    metric_name     TEXT NOT NULL,
    metric_value    REAL NOT NULL,
    created_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_metrics_device_time ON device_metrics_history(device_id, timestamp DESC);

-- Predictions
CREATE TABLE predictions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id       TEXT NOT NULL REFERENCES devices(id),
    edge_id         TEXT NOT NULL,
    prediction_type TEXT NOT NULL,
    probability     REAL NOT NULL,
    eta_minutes     INTEGER,
    features        TEXT,
    model_version   TEXT,
    status          TEXT DEFAULT 'active',
    created_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    resolved_at     TIMESTAMP
);

-- Alerts
CREATE TABLE alerts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id       TEXT REFERENCES devices(id),
    edge_id         TEXT,
    rule_name       TEXT NOT NULL,
    severity        TEXT NOT NULL,
    message         TEXT NOT NULL,
    channels        TEXT,
    status          TEXT DEFAULT 'pending',
    action_taken    TEXT,
    action_result   TEXT,
    created_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    resolved_at     TIMESTAMP
);

-- Action history
CREATE TABLE action_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    alert_id        INTEGER REFERENCES alerts(id),
    device_id       TEXT NOT NULL,
    action_id       TEXT NOT NULL,
    action_type     TEXT NOT NULL,
    command         TEXT,
    exit_code       INTEGER,
    output          TEXT,
    duration_ms     INTEGER,
    success         BOOLEAN,
    retry_count     INTEGER DEFAULT 0,
    executed_at     TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

### Metric Types by Device

| Device Type | Metric Name | Unit | Typical Range |
|-------------|-------------|------|---------------|
| **DAQ** | temperature | °C | 20-80 |
| | self_test_status | enum | passed/failed |
| | connection_state | enum | connected/disconnected |
| **PXI** | temperature | °C | 25-70 |
| | voltage_3v3 | V | 3.2-3.4 |
| | voltage_5v | V | 4.9-5.1 |
| | voltage_12v | V | 11.8-12.2 |
| | fan_speed | RPM | 1000-5000 |
| **VISA** | response_time | ms | 1-1000 |
| | timeout_count | count | 0+ |
| | idn_response | string | device-specific |
| **XNET** | bus_load | % | 0-100 |
| | error_frames | count | 0+ |
| | transceiver_status | enum | ok/error |
| **Power Supply** | output_voltage | V | device-specific |
| | output_current | A | device-specific |
| | protection_trip | boolean | true/false |

---

## Prediction Engine

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    PREDICTION ENGINE                             │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                    FEATURE PIPELINE                         │ │
│  │  Raw Metrics ──▶ Extractor ──▶ Normalizer ──▶ Feature Set  │ │
│  │                                                             │ │
│  │  Extracts:                                                  │ │
│  │  • Rolling averages (5min, 1hr, 24hr)                      │ │
│  │  • Rate of change (derivatives)                            │ │
│  │  • Variance / standard deviation                           │ │
│  │  • Time since last error                                   │ │
│  │  • Error frequency                                         │ │
│  │  • Cross-device correlations                               │ │
│  └────────────────────────────────────────────────────────────┘ │
│                              │                                   │
│                              ▼                                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                    MODEL LAYER                              │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐ │ │
│  │  │ Threshold   │  │ Statistical │  │ ML Models           │ │ │
│  │  │ Models      │  │ Models      │  │ (Optional)          │ │ │
│  │  │             │  │             │  │                     │ │ │
│  │  │ • Simple    │  │ • Linear    │  │ • Random Forest     │ │ │
│  │  │   rules     │  │   regression│  │ • Isolation Forest  │ │ │
│  │  │ • Static    │  │ • ARIMA     │  │ • Gradient Boost    │ │ │
│  │  │   limits    │  │ • EWMA      │  │ • Neural Net        │ │ │
│  │  └─────────────┘  └─────────────┘  └─────────────────────┘ │ │
│  └────────────────────────────────────────────────────────────┘ │
│                              │                                   │
│                              ▼                                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                    OUTPUT LAYER                             │ │
│  │  Prediction ──▶ Confidence ──▶ ETA ──▶ Recommended Action  │ │
│  └────────────────────────────────────────────────────────────┘ │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Prediction Types

| Prediction | Input Features | Model | Output |
|------------|---------------|-------|--------|
| **Overheating** | Temperature trend, rate of change | Linear regression + threshold | Probability, ETA to critical |
| **Connection Failure** | Timeout count, response time variance | EWMA + anomaly detection | Probability, severity |
| **Power Supply Failure** | Voltage deviation, current spikes | Statistical + ML | Probability, affected devices |
| **Bus Degradation** | Error frames, bus load | Time series analysis | Probability, recommended action |
| **Firmware Issues** | Error patterns, driver crashes | Pattern matching | Probability, rollback needed |

### Edge vs Hub Prediction

| Aspect | Edge Node | Hub |
|--------|-----------|-----|
| **Models** | Threshold, EWMA | All models + ML |
| **Scope** | Single device | Cross-device correlation |
| **Latency** | <100ms | <1s |
| **Output** | Local alerts | Global predictions |
| **Learning** | None | Can learn from all edges |

---

## Alerting & Self-Healing

### Alert Routing Rules

```yaml
rules:
  - name: "Temperature Warning"
    condition: "device.metrics.temperature > 65"
    severity: warning
    channels: [email, slack]
    recipients: ["lab-team@company.com", "#lab-alerts"]
    cooldown_minutes: 15

  - name: "Temperature Critical"
    condition: "device.metrics.temperature > 75"
    severity: critical
    channels: [email, slack, sms]
    cooldown_minutes: 5

  - name: "Prediction: Overheating"
    condition: "prediction.type == 'overheating' AND prediction.probability > 0.7"
    severity: warning
    channels: [slack, webhook]
    webhook_url: "https://internal.company.com/api/pre-alert"
    cooldown_minutes: 30

  - name: "Device Offline"
    condition: "device.status == 'offline' FOR 5 minutes"
    severity: critical
    channels: [email, slack, sms]
    trigger_action: "power_cycle_attempt"
    cooldown_minutes: 10
```

### Self-Healing Actions

```yaml
actions:
  - id: "power_cycle_attempt"
    type: "script"
    command: "./scripts/power_cycle.sh"
    args: ["${device.ip_address}"]
    timeout_seconds: 30
    retry_count: 2
    on_failure: "escalate"

  - id: "restart_ni_services"
    type: "script"
    command: "./scripts/restart_ni_services.sh"
    args: ["${edge.node_name}"]
    timeout_seconds: 60
    retry_count: 1
    on_failure: "escalate"

  - id: "reset_driver_session"
    type: "builtin"
    handler: "reset_ni_driver"
    timeout_seconds: 10
    retry_count: 3
    on_failure: "next_action"

escalation:
  - id: "escalate"
    type: "notify"
    channels: [email, sms]
    recipients: ["on-call@company.com", "+1-555-ONCALL"]
    message: "Self-healing failed for ${device.id}. Manual intervention required."
```

---

## Web Dashboard

### Pages

1. **Overview** - Real-time system health at a glance
2. **Devices** - Detailed device listing and management
3. **Edges** - Edge node management and status
4. **Alerts** - Alert management and acknowledgment
5. **Config** - System configuration

### Real-time Updates via WebSocket

```rust
pub enum DashboardMessage {
    // Server → Client
    DeviceUpdate(DeviceStatusUpdate),
    AlertNew(Alert),
    AlertUpdate { alert_id: i64, status: AlertStatus },
    PredictionNew(Prediction),
    EdgeStatusChange { edge_id: String, status: EdgeStatus },
    MetricsBatch(Vec<MetricPoint>),

    // Client → Server
    Subscribe { device_ids: Vec<String> },
    Unsubscribe { device_ids: Vec<String> },
    AcknowledgeAlert { alert_id: i64 },
    ExecuteAction { device_id: String, action: String },
}
```

### Component Structure

```
src/
├── app.rs                 # Main app with routing
├── components/
│   ├── layout/            # Sidebar, header, footer
│   ├── dashboard/         # Health cards, predictions, alerts, charts
│   ├── devices/           # Device table, detail, metrics
│   ├── edges/             # Edge table, detail
│   ├── alerts/            # Alert list, detail
│   └── config/            # Threshold, alert, action forms
├── hooks/
│   ├── use_websocket.rs   # WebSocket connection hook
│   ├── use_devices.rs     # Device state management
│   └── use_alerts.rs      # Alert state management
└── pages/
    ├── overview.rs
    ├── devices.rs
    ├── edges.rs
    ├── alerts.rs
    └── config.rs
```

---

## NI API Integration

### Supported NI APIs

| NI API | Purpose | DLL |
|--------|---------|-----|
| **NI-SysCfg** | Device discovery, system config | `niSysCfg.dll` |
| **NI-DAQmx** | DAQ device status | `nicaiu.dll` |
| **NI-VISA** | Instrument communication | `visa32.dll` |
| **NI-XNET** | CAN/LIN/FlexRay status | `niXNET.dll` |
| **NI-488.2** | GPIB controller status | `gpib-32.dll` |
| **NIPCAL** | Power supply status | `nipcal.dll` |

### FFI Module Structure

```
src/ni/
├── mod.rs
├── common.rs           # Shared error types, utils
├── syscfg/
│   ├── mod.rs
│   ├── ffi.rs          # Raw FFI bindings
│   ├── safe.rs         # Safe Rust wrapper
│   └── types.rs        # Type definitions
├── daqmx/
├── visa/
├── xnet/
└── gpib/
```

### Safe Wrapper Pattern

```rust
pub struct NiSysCfg {
    library: Library,
    // Cached function pointers
    initialize: Symbol<'static, NiSysCfgInitialize>,
    // ... other functions
}

impl NiSysCfg {
    pub fn load() -> NiResult<Self> { /* ... */ }
    pub fn create_session(&self) -> NiResult<SysCfgSession> { /* ... */ }
}

pub struct SysCfgSession<'a> {
    handle: *mut NiSysCfgHandle,
    api: &'a NiSysCfg,
}

impl<'a> SysCfgSession<'a> {
    pub fn discover_devices(&self) -> NiResult<Vec<DiscoveredDevice>> { /* ... */ }
    pub fn get_device_health(&self, device_name: &str) -> NiResult<DeviceHealth> { /* ... */ }
}
```

---

## Deployment

### Configuration Files

**Edge Config (`config/edge.yaml`):**
```yaml
node:
  id: "test-cell-01"
  name: "Test Cell 1"
  hub_address: "192.168.1.100:8080"

api:
  syscfg: { enabled: true, poll_interval_secs: 10 }
  daqmx: { enabled: true, poll_interval_secs: 5 }
  visa: { enabled: true, poll_interval_secs: 15 }
  xnet: { enabled: true, poll_interval_secs: 2 }

prediction:
  enabled: true
  models: [threshold, ewma_anomaly, trend_prediction]

buffer:
  enabled: true
  max_size_mb: 100
  persist_path: "./data/buffer.db"
```

**Hub Config (`config/hub.yaml`):**
```yaml
server:
  host: "0.0.0.0"
  port: 8080
  websocket_port: 8081

database:
  path: "./data/nimon.db"
  retention_days: 30

auth:
  enabled: true
  jwt_secret: "${NIMON_JWT_SECRET}"
```

### Service Installation

**Windows (NSSM):**
```powershell
nssm install NIMonEdge "C:\Program Files\NIMon\bin\nimon-edge.exe"
nssm set NIMonEdge AppParameters "--config config/edge.yaml"
net start NIMonEdge
```

**Linux (systemd):**
```ini
[Unit]
Description=NIMon Edge Collector
After=network.target

[Service]
Type=simple
ExecStart=/opt/nimon/bin/nimon-edge --config /opt/nimon/config/edge.yaml
Restart=always

[Install]
WantedBy=multi-user.target
```

### CLI Admin Tool

```bash
nimon-cli status                          # System overview
nimon-cli devices list --status warning   # List warning devices
nimon-cli actions run reset --device id   # Execute action
nimon-cli alerts ack 123                  # Acknowledge alert
nimon-cli edges push-config cell-01       # Push config to edge
nimon-cli export metrics --device id      # Export data
```

---

## REST API

```
# Devices
GET    /api/v1/devices
GET    /api/v1/devices/{id}
GET    /api/v1/devices/{id}/metrics
POST   /api/v1/devices/{id}/actions

# Edges
GET    /api/v1/edges
GET    /api/v1/edges/{id}
POST   /api/v1/edges/{id}/config
POST   /api/v1/edges/{id}/restart

# Alerts
GET    /api/v1/alerts
POST   /api/v1/alerts/{id}/acknowledge

# Predictions
GET    /api/v1/predictions

# System
GET    /api/v1/status
GET    /api/v1/health

# WebSocket
WS     /ws
```

---

## Development Roadmap

| Phase | Features | Duration |
|-------|----------|----------|
| **Phase 1** | Core monitoring, NI-SysCfg integration, basic alerts | 4-6 weeks |
| **Phase 2** | WebSocket hub, Leptos dashboard, multi-edge | 4-6 weeks |
| **Phase 3** | Prediction engine (threshold + EWMA), self-healing actions | 3-4 weeks |
| **Phase 4** | Additional NI APIs (DAQmx, VISA, XNET), ML predictions | 4-6 weeks |
| **Phase 5** | Polish, documentation, installer packages | 2-3 weeks |

---

## Success Criteria

1. **Performance**: <100ms latency for edge-local predictions, <1s for hub-level analysis
2. **Reliability**: 99.9% uptime for hub, automatic failover for edge disconnections
3. **Scalability**: Support 100+ edge nodes, 1000+ devices
4. **Resource Usage**: <50MB RAM per edge node, <200MB RAM for hub
5. **Prediction Accuracy**: >80% true positive rate for critical failures
