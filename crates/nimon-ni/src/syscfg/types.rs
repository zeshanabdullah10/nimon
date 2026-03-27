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
    /// IP address (if network-connected)
    pub ip_address: Option<String>,
    /// Whether the device is reachable
    pub is_reachable: bool,
    /// Device temperature in Celsius (if available)
    pub temperature: Option<f64>,
    /// Firmware version
    pub firmware_version: Option<String>,
    /// Driver version
    pub driver_version: Option<String>,
}

impl DiscoveredDevice {
    /// Create a new discovered device with minimal information
    pub fn new(product_name: String, serial_number: String) -> Self {
        Self {
            product_name,
            serial_number,
            ip_address: None,
            is_reachable: true,
            temperature: None,
            firmware_version: None,
            driver_version: None,
        }
    }
}

/// Health information for a device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceHealth {
    /// Whether the device is reachable
    pub is_reachable: bool,
    /// Device temperature in Celsius
    pub temperature: Option<f64>,
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
            self_test_passed: None,
            error_message: Some("Device not found or not reachable".to_string()),
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
            metrics.insert("temperature".to_string(), MetricValue::Float(temp));
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
        } else if let Some(temp) = self.temperature {
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
            ip_address: Some("192.168.1.100".to_string()),
            is_reachable: true,
            temperature: Some(45.5),
            firmware_version: Some("1.2.3".to_string()),
            driver_version: Some("23.0.0".to_string()),
        };

        let json = serde_json::to_string(&device).unwrap();
        let parsed: DiscoveredDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.product_name, device.product_name);
        assert_eq!(parsed.temperature, device.temperature);
    }

    #[test]
    fn test_device_health_default() {
        let health = DeviceHealth {
            is_reachable: true,
            temperature: None,
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
            Some(MetricValue::Boolean(v)) if *v == true
        ));
    }

    #[test]
    fn test_device_health_to_status_and_metrics_offline() {
        let health = DeviceHealth::unreachable();
        let (status, metrics) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Offline);
        assert!(matches!(
            metrics.get("is_reachable"),
            Some(MetricValue::Boolean(v)) if *v == false
        ));
    }

    #[test]
    fn test_device_health_to_status_and_metrics_warning() {
        let health = DeviceHealth {
            is_reachable: true,
            temperature: Some(70.0),
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
