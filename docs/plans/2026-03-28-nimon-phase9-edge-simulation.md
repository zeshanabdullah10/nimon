# NIMon Phase 9: Edge Simulation

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a simulated edge node that connects to the hub via WebSocket, sends fake device status updates and predictions. This enables end-to-end testing without real NI hardware.

**Tech Stack:** tokio-tungstenite (WebSocket client), serde_json

---

## Design

**Simulated Devices**: Generate fake temperature, voltage, and health metrics
**Message Flow**: EdgeRegister → DeviceStatusUpdate (periodic) → PredictionResult (occasional)
**Polling Interval**: Send updates every 5 seconds
**Reconnection**: Auto-reconnect on disconnect with exponential backoff

---

## Task 1: Create Edge Simulator Binary

**Files:**
- Create: `crates/nimon-edge/src/simulator.rs`
- Modify: `crates/nimon-edge/src/main.rs`

**Step 1: Add dependencies**

Update `crates/nimon-edge/Cargo.toml`:
```toml
[dependencies]
# ... existing deps ...
tokio-tungstenite = "0.21"
futures-util = "0.3"
rand = "0.8"
```

**Step 2: Create simulator module**

`crates/nimon-edge/src/simulator.rs`:
```rust
use rand::Rng;
use chrono::Utc;
use nimon_core::actor::messages::{DeviceStatusUpdate, EdgeRegister, PredictionResult};
use nimon_core::{HealthStatus, MetricValue};
use std::collections::HashMap;

pub struct EdgeSimulator {
    edge_id: String,
    edge_name: String,
    device_ids: Vec<String>,
}

impl EdgeSimulator {
    pub fn new(edge_id: &str, name: &str, device_count: usize) -> Self {
        let device_ids = (0..device_count)
            .map(|i| format!("{}-device-{}", edge_id, i))
            .collect();

        Self {
            edge_id: edge_id.to_string(),
            edge_name: name.to_string(),
            device_ids,
        }
    }

    pub fn generate_register(&self) -> EdgeRegister {
        EdgeRegister {
            edge_id: self.edge_id.clone(),
            name: self.edge_name.clone(),
            hostname: "simulated-edge".to_string(),
            ip_address: "127.0.0.1".to_string(),
        }
    }

    pub fn generate_device_status(&self) -> Vec<DeviceStatusUpdate> {
        let mut rng = rand::thread_rng();
        let timestamp = Utc::now();

        self.device_ids.iter().map(|device_id| {
            let temperature = rng.gen_range(35.0..85.0);
            let voltage_5v = rng.gen_range(4.8..5.2);
            let voltage_3v3 = rng.gen_range(3.1..3.5);

            let mut metrics = HashMap::new();
            metrics.insert("temperature".to_string(), MetricValue::Float(temperature));
            metrics.insert("voltage_5v".to_string(), MetricValue::Float(voltage_5v));
            metrics.insert("voltage_3v3".to_string(), MetricValue::Float(voltage_3v3));

            let status = if temperature > 75.0 {
                HealthStatus::Error
            } else if temperature > 65.0 {
                HealthStatus::Warning
            } else {
                HealthStatus::Healthy
            };

            DeviceStatusUpdate {
                edge_id: self.edge_id.clone(),
                device_id: device_id.clone(),
                status,
                metrics,
                timestamp,
            }
        }).collect()
    }

    pub fn maybe_generate_prediction(&self) -> Option<PredictionResult> {
        let mut rng = rand::thread_rng();
        // 20% chance of prediction
        if rng.gen_bool(0.2) {
            let device_id = self.device_ids[rng.gen_range(0..self.device_ids.len())].clone();
            let pred_type = if rng.gen_bool(0.5) {
                nimon_core::PredictionType::Overheating
            } else {
                nimon_core::PredictionType::ConnectionFailure
            };

            Some(PredictionResult {
                edge_id: self.edge_id.clone(),
                device_id,
                prediction_type: pred_type,
                probability: rng.gen_range(0.7..0.99),
                eta_minutes: Some(rng.gen_range(5..60)),
                timestamp: Utc::now(),
            })
        } else {
            None
        }
    }
}
```

**Step 3: Create main simulation loop**

`crates/nimon-edge/src/main.rs`:
```rust
mod simulator;
use simulator::EdgeSimulator;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing::info!("Starting NIMon Edge Simulator");

    let simulator = EdgeSimulator::new("sim-edge-01", "Simulated Edge 1", 3);

    // Connect to hub and run simulation loop
    // (reuse existing WsClient connection logic)
    loop {
        tracing::info!("Connecting to hub...");
        match connect_and_run(&simulator).await {
            Ok(_) => tracing::info!("Disconnected normally"),
            Err(e) => tracing::error!("Connection error: {}", e),
        }
        tracing::info!("Reconnecting in 5 seconds...");
        sleep(Duration::from_secs(5)).await;
    }
}

async fn connect_and_run(simulator: &EdgeSimulator) -> anyhow::Result<()> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    use futures_util::{SinkExt, StreamExt};

    let url = "ws://localhost:9090/ws";
    let (ws_stream, _) = connect_async(url).await?;
    tracing::info!("Connected to hub");

    let (mut write, mut read) = ws_stream.split();

    // Send registration
    let register = simulator.generate_register();
    let msg = serde_json::to_string(&register)?;
    write.send(Message::Text(msg.into())).await?;

    // Main loop: send updates every 5 seconds
    loop {
        // Send device status updates
        for status in simulator.generate_device_status() {
            let msg = serde_json::to_string(&status)?;
            write.send(Message::Text(msg.into())).await?;
        }

        // Occasionally send predictions
        if let Some(pred) = simulator.maybe_generate_prediction() {
            let msg = serde_json::to_string(&pred)?;
            write.send(Message::Text(msg.into())).await?;
        }

        // Send heartbeat
        let heartbeat = serde_json::json!({
            "type": "heartbeat",
            "timestamp": chrono::Utc::now().to_rfc3339()
        });
        write.send(Message::Text(heartbeat.to_string().into())).await?;

        tokio::time::sleep(Duration::from_secs(5)).await;

        // Check for incoming messages (pong, etc.)
        tokio::select! {
            msg = read.next() => {
                if msg.is_none() {
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }

    Ok(())
}
```

---

## Task 2: Test End-to-End

**Step 1: Start hub**
```bash
cargo run -p nimon-hub --release -- config/test-hub.yaml
```

**Step 2: Run simulator**
```bash
cargo run -p nimon-edge --release -- --simulate
```

**Step 3: Verify**
- Dashboard should show 1 edge connected
- Alerts may appear for high temperature values
- CLI should show edge and device data

---

## Commit

```bash
git add crates/nimon-edge/src/simulator.rs
git add crates/nimon-edge/src/main.rs
git commit -m "feat(edge): add edge simulator for end-to-end testing"
```
