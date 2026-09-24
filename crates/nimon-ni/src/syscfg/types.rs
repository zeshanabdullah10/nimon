//! Types for NI-SysCfg API

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use nimon_core::{HealthStatus, MetricValue};

/// A device discovered by NI-SysCfg
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredDevice {
    /// Product name (e.g., "PXIe-8880")
    pub product_name: String,
    /// Serial number
    pub serial_number: String,
    /// NI MAX device name (DAQmx user alias, e.g. "cDAQ_9205_AI")
    pub alias: Option<String>,
    /// Slot number within the chassis (modules only)
    pub slot: Option<i32>,
    /// GUID of the parent chassis/bus (matches chassis resource GUID)
    pub parent_link: Option<String>,
    /// Number of slots (chassis only)
    pub num_slots: Option<i32>,
    /// Whether NI reports this device as simulated
    pub is_simulated: bool,
    /// IP address (if network-connected)
    pub ip_address: Option<String>,
    /// Whether the device is reachable
    pub is_reachable: bool,
    /// Device temperature in Celsius (if available)
    pub temperature: Option<f64>,
    /// Firmware version
    pub firmware_version: Option<String>,
    /// Driver version. Always `None` from NI-SysCfg: nisyscfg.h has no
    /// per-resource driver version property (only HasDriver yes/no).
    pub driver_version: Option<String>,
    /// NI-SysCfg IsChassis: true for chassis (PXI/cDAQ/cRIO backplanes),
    /// false for modules/devices
    #[serde(default)]
    pub is_chassis: bool,
    /// First expert's resource name (e.g. the DAQmx/VISA resource name)
    #[serde(default)]
    pub resource_name: Option<String>,
}

impl DiscoveredDevice {
    /// Create a new discovered device with minimal information
    pub fn new(product_name: String, serial_number: String) -> Self {
        Self {
            product_name,
            serial_number,
            alias: None,
            slot: None,
            parent_link: None,
            num_slots: None,
            is_simulated: false,
            ip_address: None,
            is_reachable: true,
            temperature: None,
            firmware_version: None,
            driver_version: None,
            is_chassis: false,
            resource_name: None,
        }
    }
}

/// A named sensor reading (e.g. "TempSensor1: 42.5 C")
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    pub name: String,
    pub reading: f64,
    pub upper_critical: Option<f64>,
}

/// Station-level system information (NI MAX "system" page)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemInfo {
    pub hostname: Option<String>,
    pub product: Option<String>,
    pub operating_system: Option<String>,
    pub os_version: Option<String>,
    pub serial_number: Option<String>,
    /// Physical memory in MB
    pub memory_total_mb: Option<f64>,
    pub memory_free_mb: Option<f64>,
    /// Primary disk in MB
    pub disk_total_mb: Option<f64>,
    pub disk_free_mb: Option<f64>,
}

/// Health information for a device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceHealth {
    /// Whether the device is reachable
    pub is_reachable: bool,
    /// Device temperature in Celsius (primary/aggregate)
    pub temperature: Option<f64>,
    /// Named temperature sensors (per-sensor readings and thresholds)
    pub sensors: Vec<SensorReading>,
    /// Whether self-test passed
    pub self_test_passed: Option<bool>,
    /// Error message if any
    pub error_message: Option<String>,
    /// Additional metrics collected from the device
    pub metrics: HashMap<String, MetricValue>,
}

impl DeviceHealth {
    /// Create a DeviceHealth for an unreachable device
    pub fn unreachable() -> Self {
        Self {
            is_reachable: false,
            temperature: None,
            sensors: Vec::new(),
            self_test_passed: None,
            error_message: Some("Device not found or not reachable".to_string()),
            metrics: HashMap::new(),
        }
    }

    /// Convert health into a (HealthStatus, HashMap<String, MetricValue>) tuple
    ///
    /// This is the primary interface for consumers that need to turn raw health
    /// data into a status and metrics map for DeviceStatusUpdate messages.
    ///
    /// Non-finite readings (NaN/Inf) are dropped: serde_json encodes them
    /// as `null`, which makes the hub reject the whole status message.
    pub fn to_status_and_metrics(self) -> (HealthStatus, HashMap<String, MetricValue>) {
        let mut metrics = self.metrics;
        metrics.retain(|_, v| !matches!(v, MetricValue::Float(f) if !f.is_finite()));
        let temperature = self.temperature.filter(|t| t.is_finite());

        // Populate temperature metric
        if let Some(temp) = temperature {
            metrics.insert("temperature".to_string(), MetricValue::Float(temp));
        }

        // Named temperature sensors as individual metrics
        for sensor in self.sensors.iter().filter(|s| s.reading.is_finite()) {
            metrics.insert(
                format!("temperature[{}]", sensor.name),
                MetricValue::Float(sensor.reading),
            );
        }

        // Populate reachability metric
        metrics.insert(
            "is_reachable".to_string(),
            MetricValue::Boolean(self.is_reachable),
        );

        // Populate self-test metric
        if let Some(passed) = self.self_test_passed {
            metrics.insert("self_test_passed".to_string(), MetricValue::Boolean(passed));
        }

        // Determine overall health status
        let status = if !self.is_reachable {
            HealthStatus::Offline
        } else if self.error_message.is_some() {
            HealthStatus::Error
        } else if let Some(false) = self.self_test_passed {
            HealthStatus::Error
        } else if let Some(temp) = temperature {
            if temp > 75.0 {
                HealthStatus::Error
            } else if temp > 65.0 {
                HealthStatus::Warning
            } else {
                HealthStatus::Healthy
            }
        } else {
            HealthStatus::Healthy
        };

        (status, metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovered_device_serde() {
        let device = DiscoveredDevice {
            product_name: "PXIe-8880".to_string(),
            serial_number: "12345678".to_string(),
            alias: Some("MyPXI".to_string()),
            slot: Some(4),
            parent_link: None,
            num_slots: None,
            is_simulated: false,
            ip_address: Some("192.168.1.100".to_string()),
            is_reachable: true,
            temperature: Some(45.5),
            firmware_version: Some("1.2.3".to_string()),
            driver_version: Some("23.0.0".to_string()),
            is_chassis: false,
            resource_name: Some("PXI1Slot4".to_string()),
        };

        let json = serde_json::to_string(&device).unwrap();
        let parsed: DiscoveredDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.product_name, device.product_name);
        assert_eq!(parsed.temperature, device.temperature);
        assert_eq!(parsed.resource_name, device.resource_name);
    }

    #[test]
    fn test_discovered_device_deserializes_without_new_fields() {
        let json = r#"{"product_name":"cDAQ-9178","serial_number":"1","alias":null,
            "slot":null,"parent_link":null,"num_slots":8,"is_simulated":false,
            "ip_address":null,"is_reachable":true,"temperature":null,
            "firmware_version":null,"driver_version":null}"#;
        let parsed: DiscoveredDevice = serde_json::from_str(json).unwrap();
        assert!(!parsed.is_chassis);
        assert!(parsed.resource_name.is_none());
    }

    #[test]
    fn test_non_finite_readings_are_dropped_from_metrics() {
        let mut extra = HashMap::new();
        extra.insert("bogus".to_string(), MetricValue::Float(f64::INFINITY));
        let health = DeviceHealth {
            is_reachable: true,
            temperature: Some(f64::NAN),
            sensors: vec![
                SensorReading {
                    name: "CPU".into(),
                    reading: f64::NAN,
                    upper_critical: None,
                },
                SensorReading {
                    name: "Board".into(),
                    reading: 40.0,
                    upper_critical: None,
                },
            ],
            self_test_passed: None,
            error_message: None,
            metrics: extra,
        };
        let (status, metrics) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Healthy);
        assert!(!metrics.contains_key("temperature"));
        assert!(!metrics.contains_key("temperature[CPU]"));
        assert!(!metrics.contains_key("bogus"));
        assert!(metrics.contains_key("temperature[Board]"));
        // the whole map must serialise without nulls
        let json = serde_json::to_string(&metrics).unwrap();
        assert!(!json.contains("null"));
    }

    #[test]
    fn test_device_health_default() {
        let health = DeviceHealth {
            is_reachable: true,
            temperature: None,
            sensors: Vec::new(),
            self_test_passed: None,
            error_message: None,
            metrics: HashMap::new(),
        };
        assert!(health.is_reachable);
        assert!(health.temperature.is_none());
    }

    #[test]
    fn test_device_health_unreachable() {
        let health = DeviceHealth::unreachable();
        assert!(!health.is_reachable);
        assert!(health.error_message.is_some());
        assert!(health.metrics.is_empty());
    }

    #[test]
    fn test_device_health_to_status_and_metrics_healthy() {
        let health = DeviceHealth {
            is_reachable: true,
            temperature: Some(42.0),
            sensors: Vec::new(),
            self_test_passed: Some(true),
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, metrics) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Healthy);
        assert!(matches!(
            metrics.get("temperature"),
            Some(MetricValue::Float(v)) if *v == 42.0
        ));
        assert!(matches!(
            metrics.get("is_reachable"),
            Some(MetricValue::Boolean(true))
        ));
    }

    #[test]
    fn test_device_health_to_status_and_metrics_offline() {
        let health = DeviceHealth::unreachable();
        let (status, metrics) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Offline);
        assert!(matches!(
            metrics.get("is_reachable"),
            Some(MetricValue::Boolean(false))
        ));
    }

    #[test]
    fn test_device_health_to_status_and_metrics_warning() {
        let health = DeviceHealth {
            is_reachable: true,
            temperature: Some(70.0),
            sensors: Vec::new(),
            self_test_passed: Some(true),
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Warning);
    }

    #[test]
    fn test_device_health_to_status_and_metrics_error_temp() {
        let health = DeviceHealth {
            is_reachable: true,
            temperature: Some(80.0),
            sensors: Vec::new(),
            self_test_passed: Some(true),
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Error);
    }

    #[test]
    fn test_device_health_to_status_and_metrics_self_test_fail() {
        let health = DeviceHealth {
            is_reachable: true,
            temperature: Some(30.0),
            sensors: Vec::new(),
            self_test_passed: Some(false),
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Error);
    }

    #[test]
    fn test_device_health_to_status_and_metrics_preserves_extra_metrics() {
        let mut extra = HashMap::new();
        extra.insert("fan_speed".to_string(), MetricValue::Integer(3000));
        let health = DeviceHealth {
            is_reachable: true,
            temperature: Some(50.0),
            sensors: Vec::new(),
            self_test_passed: None,
            error_message: None,
            metrics: extra,
        };
        let (_status, metrics) = health.to_status_and_metrics();
        assert!(matches!(
            metrics.get("fan_speed"),
            Some(MetricValue::Integer(v)) if *v == 3000
        ));
    }
}
