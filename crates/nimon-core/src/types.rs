//! Core type definitions for NIMon

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Device types supported by NIMon
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    Daq,
    Pxi,
    CDaq,
    Visa,
    Xnet,
    Gpib,
    PowerSupply,
}

impl std::fmt::Display for DeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceType::Daq => write!(f, "daq"),
            DeviceType::Pxi => write!(f, "pxi"),
            DeviceType::CDaq => write!(f, "cdaq"),
            DeviceType::Visa => write!(f, "visa"),
            DeviceType::Xnet => write!(f, "xnet"),
            DeviceType::Gpib => write!(f, "gpib"),
            DeviceType::PowerSupply => write!(f, "power_supply"),
        }
    }
}

/// Health status of a device
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Warning,
    Error,
    Offline,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Warning => write!(f, "warning"),
            HealthStatus::Error => write!(f, "error"),
            HealthStatus::Offline => write!(f, "offline"),
        }
    }
}

impl HealthStatus {
    /// Parse from the wire/DB string form. Unknown values map to `Offline`.
    pub fn parse(s: &str) -> Self {
        match s {
            "healthy" => HealthStatus::Healthy,
            "warning" => HealthStatus::Warning,
            "error" => HealthStatus::Error,
            _ => HealthStatus::Offline,
        }
    }
}

/// Edge node status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeStatus {
    Online,
    Offline,
    Degraded,
}

impl std::fmt::Display for EdgeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EdgeStatus::Online => write!(f, "online"),
            EdgeStatus::Offline => write!(f, "offline"),
            EdgeStatus::Degraded => write!(f, "degraded"),
        }
    }
}

/// Alert severity levels (the single severity enum for the whole workspace)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "info"),
            Self::Warning => write!(f, "warning"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

impl Severity {
    /// Parse from the wire/DB string form, erroring on unknown values.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "info" => Ok(Severity::Info),
            "warning" => Ok(Severity::Warning),
            "critical" => Ok(Severity::Critical),
            other => Err(format!("Unknown severity: {}", other)),
        }
    }
}

/// Metric value types
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetricValue {
    Float(f64),
    Integer(i64),
    String(String),
    Boolean(bool),
}

/// A device discovered or managed by NIMon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub edge_id: String,
    pub device_name: String,
    pub device_type: DeviceType,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub firmware_version: Option<String>,
    pub driver_version: Option<String>,
    pub ip_address: Option<String>,
    pub slot: Option<i32>,
    pub chassis: Option<String>,
    /// True when the device comes from a simulated/NI MAX-simulated source
    #[serde(default)]
    pub is_simulated: bool,
}

/// Current status of a device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub device_id: String,
    pub status: HealthStatus,
    pub last_poll: DateTime<Utc>,
    pub metrics: HashMap<String, MetricValue>,
    pub error_message: Option<String>,
    pub error_count: i32,
    pub uptime_seconds: i64,
}

/// An edge node in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeNode {
    pub id: String,
    pub name: String,
    pub hostname: Option<String>,
    pub ip_address: Option<String>,
    pub last_seen: Option<DateTime<Utc>>,
    pub status: EdgeStatus,
}

/// A metric data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub device_id: String,
    pub timestamp: DateTime<Utc>,
    pub metric_name: String,
    pub metric_value: f64,
}

/// Prediction types (the single prediction-type enum for the whole workspace)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredictionType {
    Overheating,
    ConnectionFailure,
    PowerSupplyFailure,
    BusDegradation,
    FirmwareIssue,
    Custom(String),
}

impl std::fmt::Display for PredictionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PredictionType::Overheating => write!(f, "overheating"),
            PredictionType::ConnectionFailure => write!(f, "connection_failure"),
            PredictionType::PowerSupplyFailure => write!(f, "power_supply_failure"),
            PredictionType::BusDegradation => write!(f, "bus_degradation"),
            PredictionType::FirmwareIssue => write!(f, "firmware_issue"),
            PredictionType::Custom(name) => write!(f, "{}", name),
        }
    }
}

/// A prediction generated by the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prediction {
    pub id: i64,
    pub device_id: String,
    pub edge_id: String,
    pub prediction_type: PredictionType,
    pub probability: f64,
    pub eta_minutes: Option<i32>,
    pub status: PredictionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PredictionStatus {
    Active,
    Confirmed,
    Dismissed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_type_serde() {
        let dt = DeviceType::Daq;
        let json = serde_json::to_string(&dt).unwrap();
        assert_eq!(json, "\"daq\"");
        let parsed: DeviceType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, dt);
    }

    #[test]
    fn test_health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(HealthStatus::Warning.to_string(), "warning");
    }

    #[test]
    fn test_severity_wire_format() {
        // Golden fixture: the wire strings must stay stable
        assert_eq!(serde_json::to_string(&Severity::Info).unwrap(), "\"info\"");
        assert_eq!(
            serde_json::to_string(&Severity::Warning).unwrap(),
            "\"warning\""
        );
        assert_eq!(
            serde_json::to_string(&Severity::Critical).unwrap(),
            "\"critical\""
        );
        assert_eq!(
            serde_json::from_str::<Severity>("\"critical\"").unwrap(),
            Severity::Critical
        );
    }

    #[test]
    fn test_prediction_type_wire_format() {
        assert_eq!(
            serde_json::to_string(&PredictionType::Overheating).unwrap(),
            "\"overheating\""
        );
        assert_eq!(
            serde_json::to_string(&PredictionType::ConnectionFailure).unwrap(),
            "\"connection_failure\""
        );
        // Newtype variant serializes externally tagged: {"custom": "..."}
        assert_eq!(
            serde_json::to_string(&PredictionType::Custom("custom_x".into())).unwrap(),
            "{\"custom\":\"custom_x\"}"
        );
        assert_eq!(
            serde_json::from_str::<PredictionType>("\"overheating\"").unwrap(),
            PredictionType::Overheating
        );
        assert_eq!(
            serde_json::from_str::<PredictionType>("{\"custom\":\"x\"}").unwrap(),
            PredictionType::Custom("x".to_string())
        );
    }

    #[test]
    fn test_metric_value_untagged() {
        let v = serde_json::from_str::<MetricValue>("42.5").unwrap();
        assert_eq!(v, MetricValue::Float(42.5));
        // Note: untagged deserialization prefers the Float variant, so a
        // JSON integer round-trips as Float(7.0) — the wire stays numeric.
        let v = serde_json::from_str::<MetricValue>("7").unwrap();
        assert!(matches!(v, MetricValue::Float(7.0)));
        let v = serde_json::from_str::<MetricValue>("true").unwrap();
        assert_eq!(v, MetricValue::Boolean(true));
    }
}
