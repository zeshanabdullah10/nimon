//! NIMon Edge Simulator
//!
//! Simulates an edge node connecting to the hub for end-to-end testing.

use anyhow::Result;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use nimon_core::actor::messages::{DeviceStatusUpdate, EdgeHeartbeat, EdgeRegister, PredictionResult};
use nimon_core::actor::messages::PredictionType as ActorPredictionType;
use nimon_core::protocol::WsMessage;
use nimon_core::{HealthStatus, MetricValue};
use rand::Rng;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};

struct EdgeSimulator {
    edge_id: String,
    edge_name: String,
    device_ids: Vec<String>,
}

impl EdgeSimulator {
    fn new(edge_id: &str, name: &str, device_count: usize) -> Self {
        let device_ids = (0..device_count)
            .map(|i| format!("{}-device-{}", edge_id, i))
            .collect();
        Self {
            edge_id: edge_id.to_string(),
            edge_name: name.to_string(),
            device_ids,
        }
    }

    fn generate_register(&self) -> EdgeRegister {
        EdgeRegister {
            edge_id: self.edge_id.clone(),
            name: self.edge_name.clone(),
            hostname: Some("simulated-edge".to_string()),
            ip_address: Some("127.0.0.1".to_string()),
        }
    }

    fn generate_device_status(&self) -> Vec<DeviceStatusUpdate> {
        let mut rng = rand::thread_rng();
        let timestamp = Utc::now();

        self.device_ids
            .iter()
            .map(|device_id| {
                let temperature = rng.gen_range(35.0..85.0);
                let voltage_5v = rng.gen_range(4.8..5.2);
                let voltage_3v3 = rng.gen_range(3.1..3.5);

                let mut metrics = HashMap::new();
                metrics.insert(
                    "temperature".to_string(),
                    MetricValue::Float(temperature),
                );
                metrics.insert("voltage_5v".to_string(), MetricValue::Float(voltage_5v));
                metrics.insert(
                    "voltage_3v3".to_string(),
                    MetricValue::Float(voltage_3v3),
                );

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
            })
            .collect()
    }

    fn generate_heartbeat(&self) -> EdgeHeartbeat {
        EdgeHeartbeat {
            edge_id: self.edge_id.clone(),
            timestamp: Utc::now(),
            device_count: self.device_ids.len(),
            status: "connected".to_string(),
        }
    }

    fn maybe_generate_prediction(&self) -> Option<PredictionResult> {
        let mut rng = rand::thread_rng();
        // 20% chance of prediction
        if rng.gen_bool(0.2) {
            let device_id = self.device_ids[rng.gen_range(0..self.device_ids.len())].clone();
            let pred_type = if rng.gen_bool(0.5) {
                ActorPredictionType::Overheating
            } else {
                ActorPredictionType::ConnectionFailure
            };

            Some(PredictionResult {
                edge_id: self.edge_id.clone(),
                device_id,
                prediction_type: pred_type,
                probability: rng.gen_range(0.7..0.99),
                eta_minutes: Some(rng.gen_range(5..60)),
                confidence: rng.gen_range(0.6..0.95),
                timestamp: Utc::now(),
            })
        } else {
            None
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    tracing::info!("Starting NIMon Edge Simulator");

    let simulator = EdgeSimulator::new("sim-edge-01", "Simulated Edge 1", 3);

    loop {
        tracing::info!("Connecting to hub at ws://localhost:9090/ws...");
        match connect_and_run(&simulator).await {
            Ok(_) => tracing::info!("Disconnected normally"),
            Err(e) => tracing::error!("Connection error: {}", e),
        }
        tracing::info!("Reconnecting in 5 seconds...");
        sleep(Duration::from_secs(5)).await;
    }
}

async fn connect_and_run(simulator: &EdgeSimulator) -> Result<()> {
    let url = "ws://localhost:9090/ws";
    let (ws_stream, _) = connect_async(url).await?;
    tracing::info!("Connected to hub");

    let (mut write, mut read) = ws_stream.split();

    // Send registration using nimon-core protocol
    let register = simulator.generate_register();
    let msg = WsMessage::edge_register(register);
    let json = msg.to_json()?;
    write.send(Message::Text(json.into())).await?;

    // Send ping immediately
    let ping = WsMessage::ping();
    let ping_json = ping.to_json()?;
    write.send(Message::Text(ping_json.into())).await?;

    tracing::info!("Registered as {} ({})", simulator.edge_name, simulator.edge_id);

    // Main loop: send updates every 5 seconds
    loop {
        // Send device status updates
        for status in simulator.generate_device_status() {
            let msg = WsMessage::device_status(status);
            let json = msg.to_json()?;
            write.send(Message::Text(json.into())).await?;
            tracing::debug!("Sent status for device");
        }

        // Occasionally send predictions
        if let Some(pred) = simulator.maybe_generate_prediction() {
            let msg = WsMessage::prediction(pred);
            let json = msg.to_json()?;
            write.send(Message::Text(json.into())).await?;
            tracing::info!("Sent prediction");
        }

        // Send heartbeat
        let heartbeat = simulator.generate_heartbeat();
        let msg = WsMessage::heartbeat(heartbeat);
        let json = msg.to_json()?;
        write.send(Message::Text(json.into())).await?;

        tracing::debug!("Heartbeat sent");

        // Wait 5 seconds before next update
        sleep(Duration::from_secs(5)).await;

        // Check for incoming messages
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        tracing::debug!("Received: {}", text);
                    }
                    Some(Ok(Message::Close(_))) => {
                        tracing::info!("Hub closed connection");
                        break;
                    }
                    Some(Err(e)) => {
                        tracing::warn!("WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        tracing::info!("Stream ended");
                        break;
                    }
                    _ => {}
                }
            }
            _ = sleep(Duration::from_secs(1)) => {}
        }
    }

    Ok(())
}
