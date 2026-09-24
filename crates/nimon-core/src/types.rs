//! Core type definitions for NIMon

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Device types supported by NIMon.
///
/// The serde wire form equals the `Display`/DB form for every variant
/// (`power_supply` for `PowerSupply`; the old `powersupply` spelling is
/// still accepted on input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    Daq,
    Pxi,
    CDaq,
    Visa,
    Xnet,
    Gpib,
    #[serde(rename = "power_supply", alias = "powersupply")]
    PowerSupply,
}

impl DeviceType {
    /// Every variant, in declaration order.
    pub const ALL: [DeviceType; 7] = [
        DeviceType::Daq,
        DeviceType::Pxi,
        DeviceType::CDaq,
        DeviceType::Visa,
        DeviceType::Xnet,
        DeviceType::Gpib,
        DeviceType::PowerSupply,
    ];

    /// Parse from the wire/DB string form (also accepts `powersupply`).
    /// Unknown values map to `Daq`.
    pub fn parse(s: &str) -> Self {
        match s {
            "pxi" => DeviceType::Pxi,
            "cdaq" => DeviceType::CDaq,
            "visa" => DeviceType::Visa,
            "xnet" => DeviceType::Xnet,
            "gpib" => DeviceType::Gpib,
            "power_supply" | "powersupply" => DeviceType::PowerSupply,
            _ => DeviceType::Daq,
        }
    }
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

impl MetricValue {
    /// Numeric value when it is a finite number (`Float` that is not
    /// NaN/Inf, or `Integer`). Strings and booleans yield `None`.
    pub fn as_f64_finite(&self) -> Option<f64> {
        match self {
            MetricValue::Float(v) if v.is_finite() => Some(*v),
            MetricValue::Integer(v) => Some(*v as f64),
            _ => None,
        }
    }

    /// True for a `Float` holding NaN or +/-Inf.
    pub fn is_non_finite(&self) -> bool {
        matches!(self, MetricValue::Float(v) if !v.is_finite())
    }
}

/// Remove non-finite floats (NaN, +/-Inf) from a metrics map in place.
///
/// serde_json serializes non-finite floats as `null`, which peers cannot
/// read back as a `MetricValue`; senders should sanitize before sending.
pub fn sanitize_metrics(metrics: &mut HashMap<String, MetricValue>) {
    metrics.retain(|_, v| !v.is_non_finite());
}

/// Lenient serde `deserialize_with` for metric maps: entries whose value
/// is `null` or otherwise not a valid `MetricValue` are skipped instead
/// of failing the whole containing message.
pub fn deserialize_metrics_lenient<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, MetricValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: HashMap<String, serde_json::Value> = HashMap::deserialize(deserializer)?;
    Ok(metrics_from_json_map(raw))
}

/// Parse a JSON object of metrics leniently (see
/// [`deserialize_metrics_lenient`]). Invalid JSON or a non-object yields
/// an empty map.
pub fn parse_metrics_lenient(json: &str) -> HashMap<String, MetricValue> {
    match serde_json::from_str::<HashMap<String, serde_json::Value>>(json) {
        Ok(raw) => metrics_from_json_map(raw),
        Err(_) => HashMap::new(),
    }
}

fn metrics_from_json_map(raw: HashMap<String, serde_json::Value>) -> HashMap<String, MetricValue> {
    raw.into_iter()
        .filter(|(_, v)| !v.is_null())
        .filter_map(|(k, v)| {
            serde_json::from_value::<MetricValue>(v)
                .ok()
                .map(|m| (k, m))
        })
        .collect()
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
    #[serde(deserialize_with = "deserialize_metrics_lenient")]
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

/// Prediction lifecycle status (wire/DB form is lowercase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PredictionStatus {
    Active,
    Confirmed,
    Dismissed,
    /// Expired/cleared (written by `PredictionRepository::resolve`/`expire_stale`)
    Resolved,
}

impl PredictionStatus {
    /// The wire/DB string form.
    pub fn as_str(&self) -> &'static str {
        match self {
            PredictionStatus::Active => "active",
            PredictionStatus::Confirmed => "confirmed",
            PredictionStatus::Dismissed => "dismissed",
            PredictionStatus::Resolved => "resolved",
        }
    }

    /// Parse from the wire/DB string form, erroring on unknown values.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "active" => Ok(PredictionStatus::Active),
            "confirmed" => Ok(PredictionStatus::Confirmed),
            "dismissed" => Ok(PredictionStatus::Dismissed),
            "resolved" => Ok(PredictionStatus::Resolved),
            other => Err(format!("Unknown prediction status: {}", other)),
        }
    }
}

impl std::fmt::Display for PredictionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
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
    fn test_device_type_serde_matches_display_for_all_variants() {
        for dt in DeviceType::ALL {
            let json = serde_json::to_string(&dt).unwrap();
            assert_eq!(
                json,
                format!("\"{}\"", dt),
                "serde/Display drift for {:?}",
                dt
            );
            assert_eq!(DeviceType::parse(&dt.to_string()), dt);
            assert_eq!(serde_json::from_str::<DeviceType>(&json).unwrap(), dt);
        }
        assert_eq!(
            serde_json::to_string(&DeviceType::PowerSupply).unwrap(),
            "\"power_supply\""
        );
        // Old spelling still accepted
        assert_eq!(
            serde_json::from_str::<DeviceType>("\"powersupply\"").unwrap(),
            DeviceType::PowerSupply
        );
        assert_eq!(DeviceType::parse("powersupply"), DeviceType::PowerSupply);
        assert_eq!(DeviceType::parse("???"), DeviceType::Daq);
    }

    #[test]
    fn test_prediction_status_roundtrip() {
        for s in [
            PredictionStatus::Active,
            PredictionStatus::Confirmed,
            PredictionStatus::Dismissed,
            PredictionStatus::Resolved,
        ] {
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(json, format!("\"{}\"", s));
            assert_eq!(PredictionStatus::parse(s.as_str()).unwrap(), s);
        }
        assert!(PredictionStatus::parse("bogus").is_err());
    }

    #[test]
    fn test_sanitize_metrics_removes_non_finite() {
        let mut m = HashMap::new();
        m.insert("ok".to_string(), MetricValue::Float(1.5));
        m.insert("nan".to_string(), MetricValue::Float(f64::NAN));
        m.insert("inf".to_string(), MetricValue::Float(f64::INFINITY));
        m.insert("ninf".to_string(), MetricValue::Float(f64::NEG_INFINITY));
        m.insert("int".to_string(), MetricValue::Integer(3));
        m.insert("s".to_string(), MetricValue::String("x".into()));
        m.insert("b".to_string(), MetricValue::Boolean(true));
        sanitize_metrics(&mut m);
        assert_eq!(m.len(), 4);
        assert!(m.contains_key("ok") && m.contains_key("int"));
        assert!(!m.contains_key("nan") && !m.contains_key("inf") && !m.contains_key("ninf"));
    }

    #[test]
    fn test_as_f64_finite() {
        assert_eq!(MetricValue::Float(2.5).as_f64_finite(), Some(2.5));
        assert_eq!(MetricValue::Integer(-4).as_f64_finite(), Some(-4.0));
        assert_eq!(MetricValue::Float(f64::NAN).as_f64_finite(), None);
        assert_eq!(MetricValue::Float(f64::INFINITY).as_f64_finite(), None);
        assert_eq!(MetricValue::String("1".into()).as_f64_finite(), None);
        assert_eq!(MetricValue::Boolean(true).as_f64_finite(), None);
    }

    #[test]
    fn test_parse_metrics_lenient_skips_bad_entries() {
        let m = parse_metrics_lenient(r#"{"a":1.5,"b":null,"c":[1,2],"d":"txt","e":true}"#);
        assert_eq!(m.len(), 3);
        assert_eq!(m.get("a"), Some(&MetricValue::Float(1.5)));
        assert!(!m.contains_key("b") && !m.contains_key("c"));
        assert!(parse_metrics_lenient("not json").is_empty());
        assert!(parse_metrics_lenient("[1]").is_empty());
    }

    #[test]
    fn test_device_status_lenient_metrics() {
        let json = r#"{"device_id":"d","status":"healthy","last_poll":"2024-01-01T00:00:00Z",
            "metrics":{"t":40.0,"bad":null},"error_message":null,"error_count":0,"uptime_seconds":0}"#;
        let st: DeviceStatus = serde_json::from_str(json).unwrap();
        assert_eq!(st.metrics.len(), 1);
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
