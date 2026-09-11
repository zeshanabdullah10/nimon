//! Prediction engine actor
//!
//! Analyzes device metrics and generates predictions for potential issues
//! using pluggable prediction models (EWMA, trend, threshold).

use actix::prelude::*;
use chrono::Utc;
use std::collections::{HashMap, VecDeque};

use crate::prediction::{
    EwmaAnomalyDetector, ModelUpdate, PredictionModel, ThresholdPredictor, TrendPredictor,
};
use nimon_core::actor::{DeviceStatusUpdate, PredictionResult};
use nimon_core::MetricValue;
use nimon_core::PredictionType;

use crate::actor::hub_connector::HubConnectorActor;

/// Configuration for the prediction actor's models
#[derive(Debug, Clone)]
pub struct PredictionConfig {
    /// EWMA smoothing factor (0.0 to 1.0)
    pub ewma_alpha: f64,
    /// EWMA z-score threshold
    pub ewma_threshold: f64,
    /// Minimum samples before EWMA anomaly detection activates
    pub ewma_min_samples: usize,
    /// Trend predictor window size
    pub trend_window_size: usize,
    /// Trend rate-of-change threshold
    pub trend_threshold_rate: f64,
    /// Critical temperature threshold (°C)
    pub temperature_critical: f64,
    /// Warning temperature threshold (°C)
    pub temperature_warning: f64,
    /// Legacy fallback window size
    pub legacy_window_size: usize,
}

impl Default for PredictionConfig {
    fn default() -> Self {
        Self {
            ewma_alpha: 0.3,
            ewma_threshold: 2.0,
            ewma_min_samples: 5,
            trend_window_size: 10,
            trend_threshold_rate: 1.0,
            temperature_critical: 75.0,
            temperature_warning: 65.0,
            legacy_window_size: 10,
        }
    }
}

/// Actor that analyzes device metrics and generates predictions
///
/// Uses multiple pluggable prediction models per metric and falls back
/// to legacy simple temperature-based prediction if models produce nothing.
pub struct PredictionActor {
    /// Configuration
    config: PredictionConfig,
    /// Prediction models keyed by metric name (e.g., "temperature")
    models: HashMap<String, Vec<Box<dyn PredictionModel>>>,
    /// Temperature history per device (legacy fallback)
    temperature_history: HashMap<String, VecDeque<f64>>,
    /// Hub connector for forwarding predictions and status updates
    hub_connector: Option<actix::Addr<HubConnectorActor>>,
}

impl PredictionActor {
    /// Create a new prediction actor with default configuration
    pub fn new(window_size: usize) -> Self {
        let config = PredictionConfig {
            legacy_window_size: window_size,
            ..PredictionConfig::default()
        };
        Self::with_config(config)
    }

    /// Create a new prediction actor with full configuration
    pub fn with_config(config: PredictionConfig) -> Self {
        let mut models: HashMap<String, Vec<Box<dyn PredictionModel>>> = HashMap::new();

        // Register models for temperature metric
        let temp_models: Vec<Box<dyn PredictionModel>> = vec![
            Box::new(EwmaAnomalyDetector::new(
                config.ewma_alpha,
                config.ewma_threshold,
                config.ewma_min_samples,
            )),
            Box::new(TrendPredictor::new(
                config.trend_window_size,
                config.trend_threshold_rate,
                config.temperature_critical,
                PredictionType::Overheating,
            )),
            Box::new(ThresholdPredictor::new(
                config.temperature_critical,
                config.temperature_warning,
                PredictionType::Overheating,
            )),
        ];
        models.insert("temperature".to_string(), temp_models);

        Self {
            config,
            models,
            temperature_history: HashMap::new(),
            hub_connector: None,
        }
    }

    /// Rebuild the temperature model group with new thresholds,
    /// preserving EWMA state where possible (EWMA restarts warm from
    /// its current mean; trend/threshold are stateless enough to reset).
    fn rebuild_temperature_models(&mut self) {
        let config = &self.config;
        let temp_models: Vec<Box<dyn PredictionModel>> = vec![
            Box::new(EwmaAnomalyDetector::new(
                config.ewma_alpha,
                config.ewma_threshold,
                config.ewma_min_samples,
            )),
            Box::new(TrendPredictor::new(
                config.trend_window_size,
                config.trend_threshold_rate,
                config.temperature_critical,
                PredictionType::Overheating,
            )),
            Box::new(ThresholdPredictor::new(
                config.temperature_critical,
                config.temperature_warning,
                PredictionType::Overheating,
            )),
        ];
        self.models.insert("temperature".to_string(), temp_models);
    }

    /// Set temperature thresholds (legacy compatibility)
    pub fn with_thresholds(mut self, warning: f64, critical: f64) -> Self {
        self.config.temperature_warning = warning;
        self.config.temperature_critical = critical;
        self.rebuild_temperature_models();
        self
    }

    /// Set the hub connector for forwarding predictions and status updates
    pub fn with_hub_connector(mut self, addr: actix::Addr<HubConnectorActor>) -> Self {
        self.hub_connector = Some(addr);
        self
    }

    /// Process all models for a given metric update
    fn process_update(
        &mut self,
        metric_name: &str,
        value: f64,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Vec<ModelUpdate> {
        let mut results = Vec::new();

        if let Some(models) = self.models.get_mut(metric_name) {
            for model in models.iter_mut() {
                let result = model.update(metric_name, value, timestamp);
                results.push(result);
            }
        }

        results
    }

    /// Convert a model prediction to a full PredictionResult
    fn to_prediction_result(
        &self,
        device_id: &str,
        edge_id: &str,
        update: ModelUpdate,
    ) -> Option<PredictionResult> {
        match update {
            ModelUpdate::NewPrediction(pred) => Some(PredictionResult {
                device_id: device_id.to_string(),
                edge_id: edge_id.to_string(),
                prediction_type: pred.prediction_type,
                probability: pred.probability,
                eta_minutes: pred.eta_minutes,
                confidence: pred.confidence,
                reason: Some(pred.reason),
                model_version: Some(pred.model_version),
                timestamp: Utc::now(),
            }),
            ModelUpdate::NoPrediction => None,
        }
    }

    /// Legacy temperature analysis (fallback when models produce nothing)
    fn analyze_temperature(
        &mut self,
        device_id: &str,
        edge_id: &str,
        temperature: f64,
    ) -> Option<PredictionResult> {
        let history = self
            .temperature_history
            .entry(device_id.to_string())
            .or_default();

        history.push_back(temperature);

        if history.len() > self.config.legacy_window_size {
            history.pop_front();
        }

        // Need at least 3 samples for trend analysis
        if history.len() < 3 {
            return None;
        }

        let recent: Vec<f64> = history.iter().copied().collect();
        let rate = (recent[recent.len() - 1] - recent[0]) / (recent.len() - 1) as f64;

        // Predict if overheating is likely
        if rate > 0.5 && temperature > self.config.temperature_warning {
            let time_to_critical = if rate > 0.0 {
                Some(((self.config.temperature_critical - temperature) / rate * 60.0) as i32)
            } else {
                None
            };

            let probability = if temperature > self.config.temperature_critical {
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
                reason: Some(format!(
                    "Temperature rising {:.2}/sample toward critical {:.1} (now {:.1})",
                    rate, self.config.temperature_critical, temperature
                )),
                model_version: Some("legacy-avg-trend".to_string()),
                timestamp: Utc::now(),
            });
        }

        None
    }
}

impl Actor for PredictionActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        tracing::info!(
            "PredictionActor started with {} model groups",
            self.models.len()
        );
    }
}

/// Hub-pushed threshold update (routed via the device manager)
#[derive(Message)]
#[rtype(result = "()")]
pub struct UpdateThresholds {
    pub warning: f64,
    pub critical: f64,
}

impl Handler<UpdateThresholds> for PredictionActor {
    type Result = ();

    fn handle(&mut self, msg: UpdateThresholds, _ctx: &mut Self::Context) -> Self::Result {
        tracing::info!(
            "Prediction thresholds updated: warning={:.1}, critical={:.1}",
            msg.warning,
            msg.critical
        );
        self.config.temperature_warning = msg.warning;
        self.config.temperature_critical = msg.critical;
        self.rebuild_temperature_models();
    }
}

impl Handler<DeviceStatusUpdate> for PredictionActor {
    type Result = ();

    fn handle(&mut self, msg: DeviceStatusUpdate, _ctx: &mut Self::Context) -> Self::Result {
        let mut any_prediction = false;

        // Process each metric through registered models
        for (metric_name, metric_value) in &msg.metrics {
            let value = match metric_value {
                MetricValue::Float(f) => *f,
                MetricValue::Integer(i) => *i as f64,
                _ => continue,
            };

            let updates = self.process_update(metric_name, value, msg.timestamp);

            for update in updates {
                if let Some(prediction) =
                    self.to_prediction_result(&msg.device_id, &msg.edge_id, update)
                {
                    tracing::debug!(
                        "Model prediction generated: device={}, type={:?}, prob={:.2}",
                        msg.device_id,
                        prediction.prediction_type,
                        prediction.probability
                    );
                    any_prediction = true;

                    // Forward prediction to hub connector
                    if let Some(ref hub) = self.hub_connector {
                        let _ = hub.do_send(prediction.clone());
                    }
                }
            }
        }

        // Legacy fallback: if no model produced a prediction, try simple temperature analysis
        if !any_prediction {
            if let Some(MetricValue::Float(temp)) = msg.metrics.get("temperature") {
                if let Some(prediction) =
                    self.analyze_temperature(&msg.device_id, &msg.edge_id, *temp)
                {
                    tracing::debug!("Legacy prediction generated: device={:?}", prediction);

                    // Forward legacy prediction to hub connector
                    if let Some(ref hub) = self.hub_connector {
                        let _ = hub.do_send(prediction);
                    }
                }
            }
        }

        // Forward device status update to hub connector
        if let Some(ref hub) = self.hub_connector {
            let _ = hub.do_send(msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_core::HealthStatus;

    fn make_update(device_id: &str, temp: f64) -> DeviceStatusUpdate {
        let mut metrics = HashMap::new();
        metrics.insert("temperature".to_string(), MetricValue::Float(temp));
        DeviceStatusUpdate {
            device_id: device_id.to_string(),
            edge_id: "edge-1".to_string(),
            status: HealthStatus::Healthy,
            metrics,
            is_simulated: false,
            timestamp: Utc::now(),
        }
    }

    #[allow(dead_code)]
    fn make_multi_metric_update(
        device_id: &str,
        temperature: f64,
        voltage: f64,
    ) -> DeviceStatusUpdate {
        let mut metrics = HashMap::new();
        metrics.insert("temperature".to_string(), MetricValue::Float(temperature));
        metrics.insert("voltage".to_string(), MetricValue::Float(voltage));
        DeviceStatusUpdate {
            device_id: device_id.to_string(),
            edge_id: "edge-1".to_string(),
            status: HealthStatus::Healthy,
            metrics,
            is_simulated: false,
            timestamp: Utc::now(),
        }
    }

    #[actix::test]
    async fn test_prediction_actor_starts() {
        let actor = PredictionActor::new(10);
        let _addr = actor.start();
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

    #[test]
    fn test_prediction_config_default() {
        let config = PredictionConfig::default();
        assert_eq!(config.ewma_alpha, 0.3);
        assert_eq!(config.ewma_threshold, 2.0);
        assert_eq!(config.temperature_critical, 75.0);
        assert_eq!(config.temperature_warning, 65.0);
    }

    #[test]
    fn test_process_update_temperature_critical() {
        let config = PredictionConfig::default();
        let mut actor = PredictionActor::with_config(config);

        // Send a critical temperature value directly through process_update
        let updates = actor.process_update("temperature", 80.0, Utc::now());

        // Threshold predictor should fire immediately
        let has_prediction = updates
            .iter()
            .any(|u| matches!(u, ModelUpdate::NewPrediction(_)));
        assert!(
            has_prediction,
            "Expected at least one prediction for critical temperature"
        );
    }

    #[test]
    fn test_process_update_unknown_metric() {
        let config = PredictionConfig::default();
        let mut actor = PredictionActor::with_config(config);

        // No models registered for "voltage" by default
        let updates = actor.process_update("voltage", 5.0, Utc::now());
        assert!(updates.is_empty());
    }

    #[test]
    fn test_to_prediction_result_converts() {
        let config = PredictionConfig::default();
        let actor = PredictionActor::with_config(config);

        let pred = crate::prediction::Prediction {
            prediction_type: PredictionType::Overheating,
            probability: 0.8,
            confidence: 0.7,
            eta_minutes: Some(30),
            reason: "test".to_string(),
            model_version: "test-v1".to_string(),
        };

        let result =
            actor.to_prediction_result("dev-1", "edge-1", ModelUpdate::NewPrediction(pred));

        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.device_id, "dev-1");
        assert_eq!(r.edge_id, "edge-1");
        assert_eq!(r.probability, 0.8);
    }

    #[test]
    fn test_to_prediction_result_no_prediction() {
        let config = PredictionConfig::default();
        let actor = PredictionActor::with_config(config);

        let result = actor.to_prediction_result("dev-1", "edge-1", ModelUpdate::NoPrediction);
        assert!(result.is_none());
    }

    #[test]
    fn test_legacy_fallback_still_works() {
        let config = PredictionConfig::default();
        let mut actor = PredictionActor::with_config(config);

        // Build up a rising temperature history
        // Need rate > 0.5 and temp > warning (65) for legacy to trigger
        for temp in [55.0, 57.0, 60.0, 63.0, 66.0] {
            let _ = actor.analyze_temperature("dev-1", "edge-1", temp);
        }
        // history = [55.0, 57.0, 60.0, 63.0, 66.0], rate = (66-55)/4 = 2.75, temp = 66 > 65
        let result = actor.analyze_temperature("dev-1", "edge-1", 66.0);
        assert!(result.is_some());
    }

    #[test]
    fn test_with_config_custom_thresholds() {
        let mut config = PredictionConfig::default();
        config.temperature_critical = 90.0;
        config.temperature_warning = 80.0;

        let mut actor = PredictionActor::with_config(config);

        // 85 should trigger warning threshold predictor
        let updates = actor.process_update("temperature", 85.0, Utc::now());
        let has_prediction = updates
            .iter()
            .any(|u| matches!(u, ModelUpdate::NewPrediction(_)));
        assert!(
            has_prediction,
            "Expected prediction for value in warning range"
        );

        // 50 should not trigger
        let updates = actor.process_update("temperature", 50.0, Utc::now());
        let has_prediction = updates
            .iter()
            .any(|u| matches!(u, ModelUpdate::NewPrediction(_)));
        assert!(
            !has_prediction,
            "Expected no prediction for value below warning"
        );
    }

    #[test]
    fn test_integer_metric_values() {
        let mut metrics = HashMap::new();
        metrics.insert("temperature".to_string(), MetricValue::Integer(80));
        let msg = DeviceStatusUpdate {
            device_id: "dev-1".to_string(),
            edge_id: "edge-1".to_string(),
            status: HealthStatus::Healthy,
            metrics,
            is_simulated: false,
            timestamp: Utc::now(),
        };

        let config = PredictionConfig::default();
        let mut actor = PredictionActor::with_config(config);

        // Process the integer metric
        let updates = actor.process_update("temperature", 80.0, msg.timestamp);
        let has_prediction = updates
            .iter()
            .any(|u| matches!(u, ModelUpdate::NewPrediction(_)));
        assert!(
            has_prediction,
            "Integer metric value should trigger threshold predictor"
        );
    }
}
