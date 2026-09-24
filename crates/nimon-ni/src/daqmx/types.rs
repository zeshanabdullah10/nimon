//! Types for NI-DAQmx API

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use nimon_core::{HealthStatus, MetricValue};

/// A DAQ device discovered via NI-DAQmx
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaqDevice {
    /// DAQmx device name (e.g., "Dev1", "PXI1Slot3")
    pub device_name: String,
    /// Product type name (e.g., "NI PXIe-6363")
    pub product_name: String,
    /// Product number: DAQmxGetDevProductNum, the numeric hardware ID,
    /// formatted as hex (e.g. "0x7262"); empty if unavailable
    pub product_number: String,
    /// Serial number of the device (hex, as shown in NI MAX)
    pub serial_number: String,
    /// Whether DAQmx reports the device as simulated
    #[serde(default)]
    pub is_simulated: bool,
}

impl DaqDevice {
    /// Create a new DaqDevice with the given name
    pub fn new(device_name: String) -> Self {
        Self {
            device_name,
            product_name: String::new(),
            product_number: String::new(),
            serial_number: String::new(),
            is_simulated: false,
        }
    }
}

/// Health information for a DAQ device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaqHealth {
    /// Device temperature in degrees Celsius (DAQmxGetCalDevTemp)
    pub temperature: Option<f64>,
    /// Whether the device self-test passed. Only set by an explicit
    /// self-test; health polling never runs one.
    pub self_test_passed: Option<bool>,
    /// 5V power supply voltage. NI-DAQmx has no generic power-supply
    /// query, so this is always `None` from this crate (kept for API
    /// compatibility / external producers).
    pub voltage_5v: Option<f64>,
    /// 3.3V power supply voltage (see `voltage_5v`)
    pub voltage_3v3: Option<f64>,
    /// User+ power supply voltage (see `voltage_5v`)
    pub voltage_user: Option<f64>,
    /// User- power supply voltage (see `voltage_5v`)
    pub voltage_negative_user: Option<f64>,
    /// Whether the device answered a basic DAQmx query (`None` = unknown)
    #[serde(default)]
    pub is_reachable: Option<bool>,
    /// Error message if health check failed
    pub error_message: Option<String>,
    /// Additional metrics collected from the device
    pub metrics: HashMap<String, MetricValue>,
}

impl DaqHealth {
    /// Create an empty DaqHealth with all fields as None
    pub fn new() -> Self {
        Self {
            temperature: None,
            self_test_passed: None,
            voltage_5v: None,
            voltage_3v3: None,
            voltage_user: None,
            voltage_negative_user: None,
            is_reachable: None,
            error_message: None,
            metrics: HashMap::new(),
        }
    }

    /// Convert health into a (HealthStatus, HashMap<String, MetricValue>) tuple
    ///
    /// This is the primary interface for consumers that need to turn raw health
    /// data into a status and metrics map for DeviceStatusUpdate messages.
    /// Non-finite readings are dropped (they would serialise as `null`).
    pub fn to_status_and_metrics(self) -> (HealthStatus, HashMap<String, MetricValue>) {
        let mut metrics = self.metrics;
        metrics.retain(|_, v| !matches!(v, MetricValue::Float(f) if !f.is_finite()));
        let finite = |v: Option<f64>| v.filter(|x| x.is_finite());
        let temperature = finite(self.temperature);
        let voltage_5v = finite(self.voltage_5v);
        let voltage_3v3 = finite(self.voltage_3v3);

        if let Some(temp) = temperature {
            metrics.insert("temperature_celsius".to_string(), MetricValue::Float(temp));
        }
        if let Some(passed) = self.self_test_passed {
            metrics.insert("self_test_passed".to_string(), MetricValue::Boolean(passed));
        }
        if let Some(reachable) = self.is_reachable {
            metrics.insert("is_reachable".to_string(), MetricValue::Boolean(reachable));
        }
        for (key, value) in [
            ("voltage_5v", voltage_5v),
            ("voltage_3v3", voltage_3v3),
            ("voltage_user", finite(self.voltage_user)),
            ("voltage_neg_user", finite(self.voltage_negative_user)),
        ] {
            if let Some(v) = value {
                metrics.insert(key.to_string(), MetricValue::Float(v));
            }
        }

        let voltage_warning = voltage_5v.is_some_and(|v| !(4.75..=5.25).contains(&v))
            || voltage_3v3.is_some_and(|v| !(3.15..=3.45).contains(&v));

        let status = if self.is_reachable == Some(false) {
            HealthStatus::Offline
        } else if self.error_message.is_some() || self.self_test_passed == Some(false) {
            HealthStatus::Error
        } else if self.is_reachable.is_none()
            && self.self_test_passed.is_none()
            && temperature.is_none()
        {
            // No health data at all -- treat as unknown/offline
            HealthStatus::Offline
        } else if voltage_warning {
            HealthStatus::Warning
        } else {
            HealthStatus::Healthy
        };

        (status, metrics)
    }
}

impl Default for DaqHealth {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daq_device_new() {
        let device = DaqDevice::new("Dev1".to_string());
        assert_eq!(device.device_name, "Dev1");
        assert!(device.product_name.is_empty());
        assert!(device.product_number.is_empty());
        assert!(device.serial_number.is_empty());
    }

    #[test]
    fn test_daq_device_with_fields() {
        let device = DaqDevice {
            device_name: "PXI1Slot3".to_string(),
            product_name: "NI PXIe-6363".to_string(),
            product_number: "6363".to_string(),
            serial_number: "01234567".to_string(),
            is_simulated: false,
        };
        assert_eq!(device.device_name, "PXI1Slot3");
        assert_eq!(device.product_name, "NI PXIe-6363");
        assert_eq!(device.product_number, "6363");
        assert_eq!(device.serial_number, "01234567");
    }

    #[test]
    fn test_daq_device_serde() {
        let device = DaqDevice {
            device_name: "Dev1".to_string(),
            product_name: "NI USB-6009".to_string(),
            product_number: "6009".to_string(),
            serial_number: "01ABCDEF".to_string(),
            is_simulated: true,
        };

        let json = serde_json::to_string(&device).unwrap();
        let parsed: DaqDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.device_name, device.device_name);
        assert_eq!(parsed.product_name, device.product_name);
        assert_eq!(parsed.product_number, device.product_number);
        assert_eq!(parsed.serial_number, device.serial_number);
    }

    #[test]
    fn test_daq_health_reachability() {
        let mut h = DaqHealth::new();
        h.is_reachable = Some(true);
        // reachable device without a temperature sensor is healthy, not offline
        let (status, metrics) = h.clone().to_status_and_metrics();
        assert_eq!(status, HealthStatus::Healthy);
        assert!(matches!(
            metrics.get("is_reachable"),
            Some(MetricValue::Boolean(true))
        ));

        h.is_reachable = Some(false);
        h.error_message = Some("Device query failed".into());
        let (status, _) = h.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Offline);
    }

    #[test]
    fn test_daq_health_non_finite_dropped() {
        let mut h = DaqHealth::new();
        h.is_reachable = Some(true);
        h.temperature = Some(f64::NAN);
        h.voltage_5v = Some(f64::INFINITY);
        h.metrics
            .insert("x".into(), MetricValue::Float(f64::NEG_INFINITY));
        let (status, metrics) = h.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Healthy);
        assert!(!metrics.contains_key("temperature_celsius"));
        assert!(!metrics.contains_key("voltage_5v"));
        assert!(!metrics.contains_key("x"));
    }

    #[test]
    fn test_daq_health_deserializes_without_is_reachable() {
        let json = r#"{"temperature":40.0,"self_test_passed":null,"voltage_5v":null,
            "voltage_3v3":null,"voltage_user":null,"voltage_negative_user":null,
            "error_message":null,"metrics":{}}"#;
        let h: DaqHealth = serde_json::from_str(json).unwrap();
        assert!(h.is_reachable.is_none());
    }

    #[test]
    fn test_daq_health_new() {
        let health = DaqHealth::new();
        assert!(health.temperature.is_none());
        assert!(health.self_test_passed.is_none());
        assert!(health.voltage_5v.is_none());
        assert!(health.error_message.is_none());
        assert!(health.metrics.is_empty());
    }

    #[test]
    fn test_daq_health_default() {
        let health = DaqHealth::default();
        assert!(health.temperature.is_none());
    }

    #[test]
    fn test_daq_health_to_status_healthy() {
        let health = DaqHealth {
            temperature: Some(42.5),
            self_test_passed: Some(true),
            voltage_5v: Some(5.0),
            voltage_3v3: Some(3.3),
            voltage_user: None,
            voltage_negative_user: None,
            is_reachable: None,
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, metrics) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Healthy);
        assert!(matches!(
            metrics.get("temperature_celsius"),
            Some(MetricValue::Float(v)) if *v == 42.5
        ));
        assert!(matches!(
            metrics.get("self_test_passed"),
            Some(MetricValue::Boolean(true))
        ));
        assert!(matches!(
            metrics.get("voltage_5v"),
            Some(MetricValue::Float(v)) if *v == 5.0
        ));
    }

    #[test]
    fn test_daq_health_to_status_error_self_test() {
        let health = DaqHealth {
            temperature: Some(42.5),
            self_test_passed: Some(false),
            voltage_5v: Some(5.0),
            voltage_3v3: Some(3.3),
            voltage_user: None,
            voltage_negative_user: None,
            is_reachable: None,
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Error);
    }

    #[test]
    fn test_daq_health_to_status_error_message() {
        let health = DaqHealth {
            temperature: None,
            self_test_passed: None,
            voltage_5v: None,
            voltage_3v3: None,
            voltage_user: None,
            voltage_negative_user: None,
            is_reachable: None,
            error_message: Some("Device not responding".to_string()),
            metrics: HashMap::new(),
        };
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Error);
    }

    #[test]
    fn test_daq_health_to_status_offline_no_data() {
        let health = DaqHealth::new();
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Offline);
    }

    #[test]
    fn test_daq_health_to_status_warning_voltage_5v_low() {
        let health = DaqHealth {
            temperature: Some(42.5),
            self_test_passed: Some(true),
            voltage_5v: Some(4.5), // Below 4.75 threshold
            voltage_3v3: Some(3.3),
            voltage_user: None,
            voltage_negative_user: None,
            is_reachable: None,
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Warning);
    }

    #[test]
    fn test_daq_health_to_status_warning_voltage_5v_high() {
        let health = DaqHealth {
            temperature: Some(42.5),
            self_test_passed: Some(true),
            voltage_5v: Some(5.5), // Above 5.25 threshold
            voltage_3v3: Some(3.3),
            voltage_user: None,
            voltage_negative_user: None,
            is_reachable: None,
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Warning);
    }

    #[test]
    fn test_daq_health_to_status_warning_voltage_3v3_low() {
        let health = DaqHealth {
            temperature: Some(42.5),
            self_test_passed: Some(true),
            voltage_5v: Some(5.0),
            voltage_3v3: Some(3.0), // Below 3.15 threshold
            voltage_user: None,
            voltage_negative_user: None,
            is_reachable: None,
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Warning);
    }

    #[test]
    fn test_daq_health_to_status_preserves_extra_metrics() {
        let mut extra = HashMap::new();
        extra.insert("custom_metric".to_string(), MetricValue::Integer(42));
        let health = DaqHealth {
            temperature: Some(42.5),
            self_test_passed: Some(true),
            voltage_5v: None,
            voltage_3v3: None,
            voltage_user: None,
            voltage_negative_user: None,
            is_reachable: None,
            error_message: None,
            metrics: extra,
        };
        let (_status, metrics) = health.to_status_and_metrics();
        assert!(matches!(
            metrics.get("custom_metric"),
            Some(MetricValue::Integer(v)) if *v == 42
        ));
    }

    #[test]
    fn test_daq_health_to_status_temperature_only() {
        let health = DaqHealth {
            temperature: Some(55.0),
            self_test_passed: None,
            voltage_5v: None,
            voltage_3v3: None,
            voltage_user: None,
            voltage_negative_user: None,
            is_reachable: None,
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, metrics) = health.to_status_and_metrics();
        // Has temperature data, so not offline. No errors or warnings.
        assert_eq!(status, HealthStatus::Healthy);
        assert!(metrics.contains_key("temperature_celsius"));
    }

    #[test]
    fn test_daq_health_serde() {
        let health = DaqHealth {
            temperature: Some(42.5),
            self_test_passed: Some(true),
            voltage_5v: Some(5.0),
            voltage_3v3: Some(3.3),
            voltage_user: Some(2.5),
            voltage_negative_user: Some(-2.5),
            is_reachable: None,
            error_message: None,
            metrics: HashMap::new(),
        };
        let json = serde_json::to_string(&health).unwrap();
        let parsed: DaqHealth = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.temperature, health.temperature);
        assert_eq!(parsed.self_test_passed, health.self_test_passed);
        assert_eq!(parsed.voltage_5v, health.voltage_5v);
        assert_eq!(parsed.voltage_user, health.voltage_user);
        assert_eq!(parsed.voltage_negative_user, health.voltage_negative_user);
    }
}
