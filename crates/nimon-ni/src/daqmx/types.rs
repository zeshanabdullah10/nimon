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
    /// Product number (e.g., "6363")
    pub product_number: String,
    /// Serial number of the device
    pub serial_number: String,
}

impl DaqDevice {
    /// Create a new DaqDevice with the given name
    pub fn new(device_name: String) -> Self {
        Self {
            device_name,
            product_name: String::new(),
            product_number: String::new(),
            serial_number: String::new(),
        }
    }
}

/// Health information for a DAQ device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaqHealth {
    /// Device temperature in degrees Celsius
    pub temperature: Option<f64>,
    /// Whether the device self-test passed (0 = pass)
    pub self_test_passed: Option<bool>,
    /// 5V power supply voltage
    pub voltage_5v: Option<f64>,
    /// 3.3V power supply voltage
    pub voltage_3v3: Option<f64>,
    /// User+ power supply voltage
    pub voltage_user: Option<f64>,
    /// User- power supply voltage
    pub voltage_negative_user: Option<f64>,
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
            error_message: None,
            metrics: HashMap::new(),
        }
    }

    /// Convert health into a (HealthStatus, HashMap<String, MetricValue>) tuple
    ///
    /// This is the primary interface for consumers that need to turn raw health
    /// data into a status and metrics map for DeviceStatusUpdate messages.
    pub fn to_status_and_metrics(self) -> (HealthStatus, HashMap<String, MetricValue>) {
        let mut metrics = self.metrics;

        // Populate temperature metric
        if let Some(temp) = self.temperature {
            metrics.insert("temperature_celsius".to_string(), MetricValue::Float(temp));
        }

        // Populate self-test result metric
        if let Some(passed) = self.self_test_passed {
            metrics.insert("self_test_passed".to_string(), MetricValue::Boolean(passed));
        }

        // Populate power supply voltage metrics
        if let Some(v) = self.voltage_5v {
            metrics.insert("voltage_5v".to_string(), MetricValue::Float(v));
        }
        if let Some(v) = self.voltage_3v3 {
            metrics.insert("voltage_3v3".to_string(), MetricValue::Float(v));
        }
        if let Some(v) = self.voltage_user {
            metrics.insert("voltage_user".to_string(), MetricValue::Float(v));
        }
        if let Some(v) = self.voltage_negative_user {
            metrics.insert("voltage_neg_user".to_string(), MetricValue::Float(v));
        }

        // Determine overall health status
        let status = if self.error_message.is_some() {
            HealthStatus::Error
        } else if self.self_test_passed == Some(false) {
            HealthStatus::Error
        } else if self.self_test_passed.is_none() && self.temperature.is_none() {
            // No health data at all -- treat as unknown/offline
            HealthStatus::Offline
        } else {
            // Check for voltage warnings
            let voltage_warning = self.voltage_5v.map_or(false, |v| v < 4.75 || v > 5.25)
                || self.voltage_3v3.map_or(false, |v| v < 3.15 || v > 3.45);
            if voltage_warning {
                HealthStatus::Warning
            } else {
                HealthStatus::Healthy
            }
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
        };

        let json = serde_json::to_string(&device).unwrap();
        let parsed: DaqDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.device_name, device.device_name);
        assert_eq!(parsed.product_name, device.product_name);
        assert_eq!(parsed.product_number, device.product_number);
        assert_eq!(parsed.serial_number, device.serial_number);
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
            Some(MetricValue::Boolean(v)) if *v == true
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
