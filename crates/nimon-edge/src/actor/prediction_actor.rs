//! Prediction engine actor
//!
//! Analyzes device metrics and generates predictions for potential issues.

use actix::prelude::*;
use chrono::Utc;
use std::collections::{HashMap, VecDeque};

use nimon_core::actor::{DeviceStatusUpdate, PredictionResult, PredictionType};
use nimon_core::MetricValue;

/// Actor that analyzes device metrics and generates predictions
pub struct PredictionActor {
    /// History window size (number of samples)
    window_size: usize,
    /// Temperature history per device
    temperature_history: HashMap<String, VecDeque<f64>>,
    /// Warning threshold (°C)
    temperature_warning: f64,
    /// Critical threshold (°C)
    temperature_critical: f64,
}

impl PredictionActor {
    /// Create a new prediction actor with specified window size
    pub fn new(window_size: usize) -> Self {
        Self {
            window_size,
            temperature_history: HashMap::new(),
            temperature_warning: 65.0,
            temperature_critical: 75.0,
        }
    }

    /// Set temperature thresholds
    pub fn with_thresholds(mut self, warning: f64, critical: f64) -> Self {
        self.temperature_warning = warning;
        self.temperature_critical = critical;
        self
    }

    /// Analyze temperature trend and generate prediction
    fn analyze_temperature(&mut self, device_id: &str, edge_id: &str, temperature: f64) -> Option<PredictionResult> {
        let history = self.temperature_history.entry(device_id.to_string()).or_default();

        // Add new reading
        history.push_back(temperature);

        // Trim to window size
        if history.len() > self.window_size {
            history.pop_front();
        }

        // Need at least 3 samples for trend analysis
        if history.len() < 3 {
            return None;
        }

        // Calculate rate of change (degrees per sample)
        let recent: Vec<f64> = history.iter().copied().collect();
        let rate = (recent[recent.len() - 1] - recent[0]) / (recent.len() - 1) as f64;

        // Predict if overheating is likely
        if rate > 0.5 && temperature > self.temperature_warning {
            let time_to_critical = if rate > 0.0 {
                Some(((self.temperature_critical - temperature) / rate * 60.0) as i32)
            } else {
                None
            };

            let probability = if temperature > self.temperature_critical {
                0.95
            } else if rate > 1.0 {
                0.8
            } else {
                0.5 + (rate * 0.3)
            };

            return Some(PredictionResult {
                device_id: device_id.to_string(),
                edge_id: edge_id.to_string(),
                prediction_type: PredictionType::Overheating,
                probability: probability.min(1.0),
                eta_minutes: time_to_critical,
                confidence: 0.7,
                timestamp: Utc::now(),
            });
        }

        None
    }
}

impl Actor for PredictionActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        tracing::info!("PredictionActor started");
    }
}

impl Handler<DeviceStatusUpdate> for PredictionActor {
    type Result = ();

    fn handle(&mut self, msg: DeviceStatusUpdate, _ctx: &mut Self::Context) -> Self::Result {
        // Extract temperature from metrics
        if let Some(MetricValue::Float(temp)) = msg.metrics.get("temperature") {
            if let Some(_prediction) = self.analyze_temperature(&msg.device_id, &msg.edge_id, *temp) {
                // In production, would send prediction to hub or alert manager
                tracing::debug!("Prediction generated: {:?}", _prediction);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_core::HealthStatus;
    use std::collections::HashMap;

    fn make_update(device_id: &str, temp: f64) -> DeviceStatusUpdate {
        let mut metrics = HashMap::new();
        metrics.insert("temperature".to_string(), MetricValue::Float(temp));
        DeviceStatusUpdate {
            device_id: device_id.to_string(),
            edge_id: "edge-1".to_string(),
            status: HealthStatus::Healthy,
            metrics,
            timestamp: Utc::now(),
        }
    }

    #[actix::test]
    async fn test_prediction_actor_starts() {
        let actor = PredictionActor::new(10);
        let _addr = actor.start();
        // Actor started successfully
    }

    #[actix::test]
    async fn test_handle_status_update() {
        let actor = PredictionActor::new(5);
        let addr = actor.start();

        // Feed increasing temperatures
        for temp in [40.0, 50.0, 60.0, 68.0, 72.0] {
            let _ = addr.send(make_update("dev-1", temp)).await;
        }

        // Final update triggers prediction (logged internally)
        let _ = addr.send(make_update("dev-1", 74.0)).await;
    }
}
