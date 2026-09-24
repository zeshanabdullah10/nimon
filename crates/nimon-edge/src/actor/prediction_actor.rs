//! Prediction engine actor
//!
//! Runs the configured prediction models on every device status update
//! and forwards predictions plus the status itself to the hub connector.
//!
//! Model state is kept per (device, metric): readings of different
//! devices never mix. State is created lazily on the first reading and
//! dropped when the device is removed. Threshold changes are applied to
//! the existing models in place (no warm-up is lost).

use actix::prelude::*;
use chrono::Utc;
use std::collections::HashMap;

use crate::prediction::models::{
    DEFAULT_MIN_STD, DEFAULT_TREND_REEMIT_MINUTES, DEFAULT_TREND_WINDOW_MINUTES, MAX_TREND_SAMPLES,
};
use crate::prediction::{
    EwmaAnomalyDetector, ModelKind, ModelUpdate, Prediction, PredictionModel, ThresholdPredictor,
    TrendPredictor,
};
use nimon_core::actor::messages::DeviceRemoved;
use nimon_core::actor::{DeviceStatusUpdate, PredictionResult};
use nimon_core::{PredictionType, DEFAULT_TEMP_CRITICAL_C, DEFAULT_TEMP_WARNING_C};

use crate::actor::hub_connector::HubConnectorActor;

/// Metrics that get prediction models
pub const MODELED_METRICS: &[&str] = &["temperature"];

/// Settings of the prediction engine
#[derive(Debug, Clone)]
pub struct PredictionSettings {
    /// Run models at all (false = pass status updates through)
    pub enabled: bool,
    /// Models created for each (device, metric)
    pub models: Vec<ModelKind>,
    /// EWMA smoothing factor (0.0 to 1.0)
    pub ewma_alpha: f64,
    /// EWMA z-score threshold
    pub ewma_threshold: f64,
    /// Minimum samples before EWMA anomaly detection activates
    pub ewma_min_samples: usize,
    /// EWMA standard-deviation floor (C)
    pub ewma_min_std: f64,
    /// Trend predictor sample cap (memory bound)
    pub trend_window_size: usize,
    /// Trend regression window (minutes of history, time based)
    pub trend_window_minutes: u64,
    /// Trend rate-of-change threshold (C per minute)
    pub trend_threshold_rate: f64,
    /// Minimum minutes between repeated trend predictions
    pub trend_reemit_minutes: u64,
    /// Critical temperature threshold (C)
    pub temperature_critical: f64,
    /// Warning temperature threshold (C)
    pub temperature_warning: f64,
}

impl Default for PredictionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            models: vec![ModelKind::Threshold, ModelKind::Ewma, ModelKind::Trend],
            ewma_alpha: 0.3,
            ewma_threshold: 2.0,
            ewma_min_samples: 5,
            ewma_min_std: DEFAULT_MIN_STD,
            trend_window_size: MAX_TREND_SAMPLES,
            trend_window_minutes: DEFAULT_TREND_WINDOW_MINUTES,
            trend_threshold_rate: 1.0,
            trend_reemit_minutes: DEFAULT_TREND_REEMIT_MINUTES as u64,
            temperature_critical: DEFAULT_TEMP_CRITICAL_C,
            temperature_warning: DEFAULT_TEMP_WARNING_C,
        }
    }
}

impl PredictionSettings {
    /// Settings from the `prediction:` section of the edge YAML. Unknown
    /// model names are logged and ignored.
    pub fn from_config(config: &crate::config::PredictionConfig) -> Self {
        let mut models = Vec::new();
        for name in &config.models {
            match ModelKind::parse(name) {
                Some(kind) if !models.contains(&kind) => models.push(kind),
                Some(_) => {}
                None => tracing::warn!(
                    "Unknown prediction model '{name}' ignored (known: threshold, ewma_anomaly, trend_prediction)"
                ),
            }
        }
        Self {
            enabled: config.enabled,
            models,
            ewma_min_std: config.ewma_min_std,
            trend_reemit_minutes: config.trend_reemit_minutes,
            trend_window_minutes: config.trend_window_minutes.max(1),
            temperature_critical: config.temperature_critical,
            temperature_warning: config.temperature_warning,
            ..Self::default()
        }
    }

    fn build_models(&self) -> Vec<Box<dyn PredictionModel>> {
        self.models
            .iter()
            .map(|kind| -> Box<dyn PredictionModel> {
                match kind {
                    ModelKind::Ewma => Box::new(
                        EwmaAnomalyDetector::new(
                            self.ewma_alpha,
                            self.ewma_threshold,
                            self.ewma_min_samples,
                        )
                        .with_min_std(self.ewma_min_std),
                    ),
                    ModelKind::Trend => Box::new(
                        TrendPredictor::new(
                            self.trend_window_size,
                            self.trend_threshold_rate,
                            self.temperature_critical,
                            PredictionType::Overheating,
                        )
                        .with_reemit_minutes(self.trend_reemit_minutes)
                        .with_window_minutes(self.trend_window_minutes),
                    ),
                    ModelKind::Threshold => Box::new(ThresholdPredictor::new(
                        self.temperature_critical,
                        self.temperature_warning,
                        PredictionType::Overheating,
                    )),
                }
            })
            .collect()
    }
}

type MetricModels = HashMap<String, Vec<Box<dyn PredictionModel>>>;

/// Actor that analyzes device metrics and generates predictions
pub struct PredictionActor {
    settings: PredictionSettings,
    /// device id -> metric -> models
    devices: HashMap<String, MetricModels>,
    /// Hub connector for forwarding predictions and status updates
    hub_connector: Option<Addr<HubConnectorActor>>,
}

impl PredictionActor {
    pub fn new(settings: PredictionSettings) -> Self {
        Self {
            settings,
            devices: HashMap::new(),
            hub_connector: None,
        }
    }

    /// Set the hub connector for forwarding predictions and status updates
    pub fn with_hub_connector(mut self, addr: Addr<HubConnectorActor>) -> Self {
        self.hub_connector = Some(addr);
        self
    }

    /// Number of devices with model state
    pub fn tracked_devices(&self) -> usize {
        self.devices.len()
    }

    /// Current (warning, critical) thresholds
    pub fn thresholds(&self) -> (f64, f64) {
        (
            self.settings.temperature_warning,
            self.settings.temperature_critical,
        )
    }

    /// Feed one reading of one device's metric through its models
    /// (created lazily) and return the predictions produced.
    pub fn process_update(
        &mut self,
        device_id: &str,
        metric_name: &str,
        value: f64,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Vec<Prediction> {
        if !self.settings.enabled || self.settings.models.is_empty() {
            return Vec::new();
        }
        if !self.devices.contains_key(device_id) {
            self.devices.insert(device_id.to_string(), HashMap::new());
        }
        let metrics = self.devices.get_mut(device_id).expect("inserted above");
        if !metrics.contains_key(metric_name) {
            metrics.insert(metric_name.to_string(), self.settings.build_models());
        }
        let models = metrics.get_mut(metric_name).expect("inserted above");

        let mut out = Vec::new();
        for model in models.iter_mut() {
            if let ModelUpdate::NewPrediction(p) = model.update(metric_name, value, timestamp) {
                out.push(p);
            }
        }
        out
    }

    /// Drop all model state of a device
    pub fn remove_device(&mut self, device_id: &str) -> bool {
        self.devices.remove(device_id).is_some()
    }

    /// Update thresholds on every existing model in place
    pub fn set_thresholds(&mut self, warning: f64, critical: f64) {
        self.settings.temperature_warning = warning;
        self.settings.temperature_critical = critical;
        for metrics in self.devices.values_mut() {
            for models in metrics.values_mut() {
                for model in models.iter_mut() {
                    model.set_thresholds(warning, critical);
                }
            }
        }
    }

    fn to_result(device_id: &str, edge_id: &str, pred: Prediction) -> PredictionResult {
        PredictionResult {
            device_id: device_id.to_string(),
            edge_id: edge_id.to_string(),
            prediction_type: pred.prediction_type,
            probability: pred.probability,
            eta_minutes: pred.eta_minutes,
            confidence: pred.confidence,
            reason: Some(pred.reason),
            model_version: Some(pred.model_version),
            timestamp: Utc::now(),
        }
    }
}

impl Actor for PredictionActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        tracing::info!(
            "PredictionActor started (enabled={}, models={:?}, thresholds={:.1}/{:.1} C)",
            self.settings.enabled,
            self.settings.models,
            self.settings.temperature_warning,
            self.settings.temperature_critical
        );
    }
}

/// Threshold update (routed via the device manager)
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
            "Prediction thresholds updated in place: warning={:.1}, critical={:.1}",
            msg.warning,
            msg.critical
        );
        self.set_thresholds(msg.warning, msg.critical);
    }
}

impl Handler<DeviceStatusUpdate> for PredictionActor {
    type Result = ();

    fn handle(&mut self, msg: DeviceStatusUpdate, _ctx: &mut Self::Context) -> Self::Result {
        for metric in MODELED_METRICS {
            let Some(value) = msg.metrics.get(*metric).and_then(|v| v.as_f64_finite()) else {
                continue;
            };
            for pred in self.process_update(&msg.device_id, metric, value, msg.timestamp) {
                tracing::debug!(
                    "Prediction: device={}, type={:?}, p={:.2}, model={}",
                    msg.device_id,
                    pred.prediction_type,
                    pred.probability,
                    pred.model_version
                );
                if let Some(ref hub) = self.hub_connector {
                    hub.do_send(Self::to_result(&msg.device_id, &msg.edge_id, pred));
                }
            }
        }

        if let Some(ref hub) = self.hub_connector {
            hub.do_send(msg);
        }
    }
}

/// Device removal: drop its model state, then tell the hub (routed
/// through this actor so it cannot overtake that device's last status)
impl Handler<DeviceRemoved> for PredictionActor {
    type Result = ();

    fn handle(&mut self, msg: DeviceRemoved, _ctx: &mut Self::Context) -> Self::Result {
        self.remove_device(&msg.device_id);
        if let Some(ref hub) = self.hub_connector {
            hub.do_send(msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use nimon_core::{HealthStatus, MetricValue};

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

    fn actor() -> PredictionActor {
        PredictionActor::new(PredictionSettings::default())
    }

    #[actix::test]
    async fn test_prediction_actor_handles_updates_without_hub() {
        let addr = actor().start();
        for temp in [40.0, 50.0, 60.0, 68.0, 72.0, 74.0] {
            addr.send(make_update("dev-1", temp)).await.unwrap();
        }
        addr.send(UpdateThresholds {
            warning: 70.0,
            critical: 80.0,
        })
        .await
        .unwrap();
        addr.send(DeviceRemoved {
            edge_id: "edge-1".into(),
            device_id: "dev-1".into(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
    }

    #[test]
    fn test_settings_default() {
        let s = PredictionSettings::default();
        assert_eq!(s.ewma_alpha, 0.3);
        assert_eq!(s.ewma_threshold, 2.0);
        assert_eq!(s.temperature_critical, 75.0);
        assert_eq!(s.temperature_warning, 65.0);
        assert_eq!(s.models.len(), 3);
    }

    #[test]
    fn test_settings_from_config_selects_models() {
        let cfg = crate::config::PredictionConfig {
            models: vec!["threshold".into(), "bogus".into(), "threshold".into()],
            temperature_warning: 60.0,
            temperature_critical: 70.0,
            ..Default::default()
        };
        let s = PredictionSettings::from_config(&cfg);
        assert_eq!(s.models, vec![ModelKind::Threshold]);
        assert_eq!(s.temperature_warning, 60.0);
        assert_eq!(s.temperature_critical, 70.0);
    }

    #[test]
    fn test_models_are_per_device() {
        let mut a = actor();
        let base = Utc::now();
        // device A warms up flat at 40; device B is hot and flat at 70
        for i in 0..10 {
            let ts = base + Duration::minutes(i);
            a.process_update("dev-a", "temperature", 40.0, ts);
            a.process_update("dev-b", "temperature", 70.0, ts);
        }
        assert_eq!(a.tracked_devices(), 2);
        // a normal reading of B must not look like an anomaly relative to A
        let ts = base + Duration::minutes(10);
        let preds = a.process_update("dev-b", "temperature", 70.0, ts);
        assert!(preds.is_empty(), "{preds:?}");
        // but a spike on A is an anomaly for A
        let preds = a.process_update("dev-a", "temperature", 48.0, ts);
        assert!(preds.iter().any(|p| p.model_version.starts_with("ewma")));
    }

    #[test]
    fn test_threshold_band_state_is_per_device() {
        let mut a = actor();
        let ts = Utc::now();
        // first device enters warning
        assert!(!a.process_update("d1", "temperature", 70.0, ts).is_empty());
        // second device entering warning is news too (not deduped by d1)
        assert!(!a.process_update("d2", "temperature", 70.0, ts).is_empty());
    }

    #[test]
    fn test_remove_device_drops_state() {
        let mut a = actor();
        a.process_update("d1", "temperature", 40.0, Utc::now());
        assert!(a.remove_device("d1"));
        assert!(!a.remove_device("d1"));
        assert_eq!(a.tracked_devices(), 0);
    }

    #[test]
    fn test_threshold_update_keeps_state() {
        let mut a = actor();
        let ts = Utc::now();
        assert!(!a.process_update("d1", "temperature", 70.0, ts).is_empty());
        a.set_thresholds(80.0, 90.0);
        assert_eq!(a.thresholds(), (80.0, 90.0));
        assert_eq!(a.tracked_devices(), 1, "models are not rebuilt");
        // 85 is in the new warning band -> transition fires
        let preds = a.process_update("d1", "temperature", 85.0, ts);
        assert!(preds.iter().any(|p| p.reason.contains("warning range")));
        // newly created models use the new thresholds too
        let preds = a.process_update("d2", "temperature", 72.0, ts);
        assert!(preds.is_empty());
    }

    #[test]
    fn test_disabled_prediction_passes_through() {
        let mut a = PredictionActor::new(PredictionSettings {
            enabled: false,
            ..PredictionSettings::default()
        });
        assert!(a
            .process_update("d1", "temperature", 99.0, Utc::now())
            .is_empty());
        assert_eq!(a.tracked_devices(), 0);
    }

    #[test]
    fn test_critical_temperature_fires_threshold() {
        let mut a = actor();
        let preds = a.process_update("d1", "temperature", 80.0, Utc::now());
        assert!(preds
            .iter()
            .any(|p| p.model_version.starts_with("threshold")));
    }

    #[test]
    fn test_to_result_converts() {
        let pred = Prediction {
            prediction_type: PredictionType::Overheating,
            probability: 0.8,
            confidence: 0.7,
            eta_minutes: Some(30),
            reason: "test".to_string(),
            model_version: "test-v1".to_string(),
        };
        let r = PredictionActor::to_result("dev-1", "edge-1", pred);
        assert_eq!(r.device_id, "dev-1");
        assert_eq!(r.edge_id, "edge-1");
        assert_eq!(r.probability, 0.8);
    }
}
