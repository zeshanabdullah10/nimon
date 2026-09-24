//! Enhanced prediction models for device failure prediction
//!
//! This module provides pluggable prediction models that can analyze
//! device metrics and generate failure predictions:
//!
//! - [`EwmaAnomalyDetector`]: EWMA-based detection of rising anomalies
//! - [`TrendPredictor`]: Linear regression trend detection with ETA
//! - [`ThresholdPredictor`]: Simple threshold-based prediction
//!
//! # Example
//! ```no_run
//! use nimon_edge::prediction::{EwmaAnomalyDetector, ThresholdPredictor, PredictionModel};
//! use chrono::Utc;
//!
//! let mut detector = EwmaAnomalyDetector::default_params();
//! let result = detector.update("temperature", 25.5, Utc::now());
//! ```

pub mod models;

pub use models::{
    excess_probability, EwmaAnomalyDetector, ModelKind, ModelUpdate, Prediction, PredictionModel,
    ThresholdPredictor, TrendPredictor,
};
