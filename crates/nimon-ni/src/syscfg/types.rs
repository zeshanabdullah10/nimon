//! Types for NI-SysCfg API

use serde::{Deserialize, Serialize};

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
        };
        assert!(health.is_reachable);
        assert!(health.temperature.is_none());
    }
}
