//! Types for NI-VISA API

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use nimon_core::{HealthStatus, MetricValue};

/// A VISA instrument discovered by the resource manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisaInstrument {
    /// VISA resource name (e.g., "TCPIP0::192.168.1.100::inst0::INSTR")
    pub resource_name: String,
    /// Interface type (TCPIP, GPIB, ASRL, USB, PXI, etc.)
    pub interface_type: String,
    /// Human-readable description
    pub description: Option<String>,
    /// Whether the instrument is reachable
    pub is_reachable: bool,
    /// Response time in milliseconds (from *IDN? query, if available)
    pub response_time_ms: Option<f64>,
}

impl VisaInstrument {
    /// Create a new VisaInstrument with the given resource name and interface type
    pub fn new(resource_name: String, interface_type: String) -> Self {
        Self {
            resource_name,
            interface_type,
            description: None,
            is_reachable: false,
            response_time_ms: None,
        }
    }
}

/// Health information for a VISA instrument
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisaHealth {
    /// Whether the instrument is reachable
    pub is_reachable: bool,
    /// Response from the *IDN? SCPI query (e.g., "Keysight,34461A,MY12345678,03.00-02.38-02.00")
    pub idn_response: Option<String>,
    /// Response time in milliseconds for the health check query
    pub response_time_ms: Option<f64>,
    /// Number of timeouts encountered during health check
    pub timeout_count: u32,
    /// Error message if health check failed
    pub error_message: Option<String>,
    /// Additional metrics collected from the instrument
    pub metrics: HashMap<String, MetricValue>,
}

impl VisaHealth {
    /// Create a VisaHealth for an unreachable instrument
    pub fn unreachable() -> Self {
        Self {
            is_reachable: false,
            idn_response: None,
            response_time_ms: None,
            timeout_count: 0,
            error_message: Some("Instrument not reachable".to_string()),
            metrics: HashMap::new(),
        }
    }

    /// Create a healthy VisaHealth from an *IDN? response
    pub fn healthy(idn_response: String, response_time_ms: f64) -> Self {
        Self {
            is_reachable: true,
            idn_response: Some(idn_response),
            response_time_ms: Some(response_time_ms),
            timeout_count: 0,
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

        // Populate reachability metric
        metrics.insert(
            "is_reachable".to_string(),
            MetricValue::Boolean(self.is_reachable),
        );

        // Populate response time metric
        if let Some(rt) = self.response_time_ms {
            metrics.insert("response_time_ms".to_string(), MetricValue::Float(rt));
        }

        // Populate timeout count metric
        if self.timeout_count > 0 {
            metrics.insert(
                "timeout_count".to_string(),
                MetricValue::Integer(self.timeout_count as i64),
            );
        }

        // Determine overall health status
        let status = if !self.is_reachable {
            HealthStatus::Offline
        } else if self.error_message.is_some() {
            HealthStatus::Error
        } else if self.timeout_count > 0 {
            HealthStatus::Warning
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
    fn test_visa_instrument_new() {
        let instr = VisaInstrument::new(
            "TCPIP0::192.168.1.100::inst0::INSTR".to_string(),
            "TCPIP".to_string(),
        );
        assert_eq!(instr.resource_name, "TCPIP0::192.168.1.100::inst0::INSTR");
        assert_eq!(instr.interface_type, "TCPIP");
        assert!(instr.description.is_none());
        assert!(!instr.is_reachable);
        assert!(instr.response_time_ms.is_none());
    }

    #[test]
    fn test_visa_instrument_serde() {
        let instr = VisaInstrument {
            resource_name: "GPIB0::1::INSTR".to_string(),
            interface_type: "GPIB".to_string(),
            description: Some("DMM".to_string()),
            is_reachable: true,
            response_time_ms: Some(45.2),
        };

        let json = serde_json::to_string(&instr).unwrap();
        let parsed: VisaInstrument = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.resource_name, instr.resource_name);
        assert_eq!(parsed.interface_type, instr.interface_type);
        assert_eq!(parsed.description, instr.description);
        assert!(parsed.is_reachable);
    }

    #[test]
    fn test_visa_health_unreachable() {
        let health = VisaHealth::unreachable();
        assert!(!health.is_reachable);
        assert!(health.idn_response.is_none());
        assert!(health.error_message.is_some());
        assert_eq!(health.timeout_count, 0);
        assert!(health.metrics.is_empty());
    }

    #[test]
    fn test_visa_health_healthy() {
        let health = VisaHealth::healthy(
            "Keysight,34461A,MY12345678,03.00-02.38-02.00".to_string(),
            23.5,
        );
        assert!(health.is_reachable);
        assert_eq!(
            health.idn_response,
            Some("Keysight,34461A,MY12345678,03.00-02.38-02.00".to_string())
        );
        assert_eq!(health.response_time_ms, Some(23.5));
        assert_eq!(health.timeout_count, 0);
        assert!(health.error_message.is_none());
    }

    #[test]
    fn test_visa_health_to_status_healthy() {
        let health = VisaHealth::healthy("Keysight,34461A,MY12345678,03.00".to_string(), 15.0);
        let (status, metrics) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Healthy);
        assert!(matches!(
            metrics.get("is_reachable"),
            Some(MetricValue::Boolean(v)) if *v == true
        ));
        assert!(matches!(
            metrics.get("response_time_ms"),
            Some(MetricValue::Float(v)) if *v == 15.0
        ));
    }

    #[test]
    fn test_visa_health_to_status_offline() {
        let health = VisaHealth::unreachable();
        let (status, metrics) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Offline);
        assert!(matches!(
            metrics.get("is_reachable"),
            Some(MetricValue::Boolean(v)) if *v == false
        ));
    }

    #[test]
    fn test_visa_health_to_status_error() {
        let health = VisaHealth {
            is_reachable: true,
            idn_response: None,
            response_time_ms: None,
            timeout_count: 0,
            error_message: Some("Timeout waiting for response".to_string()),
            metrics: HashMap::new(),
        };
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Error);
    }

    #[test]
    fn test_visa_health_to_status_warning_timeout() {
        let health = VisaHealth {
            is_reachable: true,
            idn_response: Some("Agilent,33220A,MY0012,1.00".to_string()),
            response_time_ms: Some(5000.0),
            timeout_count: 3,
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, metrics) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Warning);
        assert!(matches!(
            metrics.get("timeout_count"),
            Some(MetricValue::Integer(v)) if *v == 3
        ));
    }

    #[test]
    fn test_visa_health_to_status_preserves_extra_metrics() {
        let mut extra = HashMap::new();
        extra.insert("signal_strength".to_string(), MetricValue::Float(-42.5));
        let health = VisaHealth {
            is_reachable: true,
            idn_response: Some("Rohde,FSQ,1234,2.0".to_string()),
            response_time_ms: Some(10.0),
            timeout_count: 0,
            error_message: None,
            metrics: extra,
        };
        let (_status, metrics) = health.to_status_and_metrics();
        assert!(matches!(
            metrics.get("signal_strength"),
            Some(MetricValue::Float(v)) if *v == -42.5
        ));
    }

    #[test]
    fn test_visa_health_serde() {
        let health = VisaHealth::healthy("Vendor,Model,SN,Ver".to_string(), 12.3);
        let json = serde_json::to_string(&health).unwrap();
        let parsed: VisaHealth = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.idn_response, health.idn_response);
        assert_eq!(parsed.response_time_ms, health.response_time_ms);
    }
}
