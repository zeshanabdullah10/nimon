//! Device identity, health classification and simulation helpers
//!
//! Pure functions used by the device manager (kept separate so they can
//! be unit tested without actors or NI drivers), plus the
//! [`DeviceRegistry`] the action runtime uses to map hub device IDs to
//! local hardware.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use nimon_core::{Device, DeviceType, HealthStatus, MetricValue};
use nimon_ni::syscfg::{DeviceHealth, DiscoveredDevice};

/// Current temperature thresholds (C)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    pub warning: f64,
    pub critical: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            warning: nimon_core::DEFAULT_TEMP_WARNING_C,
            critical: nimon_core::DEFAULT_TEMP_CRITICAL_C,
        }
    }
}

/// Where a managed device comes from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceSource {
    /// NI-SysCfg sweep
    SysCfg,
    /// NI-VISA resource discovery
    Visa,
    /// Simulated (no NI software available)
    Simulated,
    /// Added explicitly through `AddDevice` (never auto-removed)
    Manual,
}

/// What the action runtime needs to know about a managed device
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDevice {
    /// NI-DAQmx device name (the NI MAX alias), when known
    pub daqmx_name: Option<String>,
    pub is_simulated: bool,
    pub source: DeviceSource,
}

/// Devices currently managed by this edge, keyed by hub device ID.
/// Written by the device manager, read by the action runtime.
#[derive(Debug, Clone, Default)]
pub struct DeviceRegistry(Arc<RwLock<HashMap<String, LocalDevice>>>);

impl DeviceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, device_id: String, device: LocalDevice) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(device_id, device);
    }

    pub fn remove(&self, device_id: &str) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(device_id);
    }

    pub fn get(&self, device_id: &str) -> Option<LocalDevice> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(device_id)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.0.read().unwrap_or_else(PoisonError::into_inner).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Attempt to classify a device product name into a DeviceType
pub fn classify_device(product_name: &str) -> DeviceType {
    let name = product_name.to_uppercase();
    // Check more specific patterns first to avoid false matches
    if name.contains("CDAQ") || name.contains("COMPACTDAQ") {
        DeviceType::CDaq
    } else if name.contains("XNET") || name.contains("NI-XNET") {
        DeviceType::Xnet
    } else if name.contains("GPIB") {
        DeviceType::Gpib
    } else if name.contains("VISA") {
        DeviceType::Visa
    } else if name.contains("PS") || name.contains("POWER") {
        DeviceType::PowerSupply
    } else if name.contains("PXI") || name.contains("PXIE") {
        DeviceType::Pxi
    } else {
        DeviceType::Daq
    }
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// Write the stable identity key of a discovered device into `out`:
/// NI MAX alias, else serial number, else expert resource name, else
/// `product#n` where `n` counts devices of that product that have none of
/// the above (in enumeration order). `fallback_counts` must be cleared
/// once per sweep.
pub fn identity_key(
    d: &DiscoveredDevice,
    fallback_counts: &mut HashMap<String, u32>,
    out: &mut String,
) {
    out.clear();
    if let Some(alias) = non_empty(d.alias.as_deref()) {
        out.push_str(alias);
    } else if let Some(serial) = non_empty(Some(d.serial_number.as_str())) {
        out.push_str(serial);
    } else if let Some(resource) = non_empty(d.resource_name.as_deref()) {
        out.push_str(resource);
    } else {
        let n = match fallback_counts.get_mut(d.product_name.as_str()) {
            Some(n) => {
                *n += 1;
                *n
            }
            None => {
                fallback_counts.insert(d.product_name.clone(), 1);
                1
            }
        };
        out.push_str(&d.product_name);
        out.push('#');
        out.push_str(&n.to_string());
    }
}

/// Hub device ID for a local key (`edge-id:key`)
pub fn device_id(edge_id: &str, key: &str, out: &mut String) {
    out.clear();
    out.push_str(edge_id);
    out.push(':');
    out.push_str(key);
}

/// Build a Device from a discovery result
pub fn device_from_discovered(edge_id: &str, id: &str, d: &DiscoveredDevice) -> Device {
    let alias = non_empty(d.alias.as_deref()).map(str::to_string);
    Device {
        id: id.to_string(),
        edge_id: edge_id.to_string(),
        device_name: alias.unwrap_or_else(|| d.product_name.clone()),
        device_type: classify_device(&d.product_name),
        model: Some(d.product_name.clone()),
        serial_number: non_empty(Some(d.serial_number.as_str())).map(str::to_string),
        firmware_version: d.firmware_version.clone(),
        driver_version: d.driver_version.clone(),
        ip_address: d.ip_address.clone(),
        slot: d.slot,
        chassis: d.parent_link.clone(),
        is_simulated: d.is_simulated,
    }
}

/// Registry entry for a SysCfg device: the DAQmx name is the NI MAX alias
pub fn local_from_discovered(d: &DiscoveredDevice) -> LocalDevice {
    LocalDevice {
        daqmx_name: non_empty(d.alias.as_deref()).map(str::to_string),
        is_simulated: d.is_simulated,
        source: DeviceSource::SysCfg,
    }
}

/// Health status from the raw inputs using the *current* thresholds
pub fn classify_health(
    reachable: bool,
    has_error: bool,
    self_test_failed: bool,
    temperature: Option<f64>,
    thresholds: Thresholds,
) -> HealthStatus {
    if !reachable {
        HealthStatus::Offline
    } else if has_error || self_test_failed {
        HealthStatus::Error
    } else {
        match temperature.filter(|t| t.is_finite()) {
            Some(t) if t >= thresholds.critical => HealthStatus::Error,
            Some(t) if t >= thresholds.warning => HealthStatus::Warning,
            _ => HealthStatus::Healthy,
        }
    }
}

/// Convert NI-SysCfg health into status + metrics. nimon-ni classifies
/// temperatures with fixed defaults; the status is recomputed here with
/// the edge's current thresholds.
pub fn status_from_health(
    health: DeviceHealth,
    thresholds: Thresholds,
) -> (HealthStatus, HashMap<String, MetricValue>) {
    let reachable = health.is_reachable;
    let has_error = health.error_message.is_some();
    let self_test_failed = health.self_test_passed == Some(false);
    let (_, mut metrics) = health.to_status_and_metrics();
    nimon_core::sanitize_metrics(&mut metrics);
    let temperature = metrics.get("temperature").and_then(|v| v.as_f64_finite());
    let status = classify_health(
        reachable,
        has_error,
        self_test_failed,
        temperature,
        thresholds,
    );
    (status, metrics)
}

/// Interface prefix of a VISA resource name (`TCPIP0::...` -> `TCPIP`)
pub fn visa_interface(resource: &str) -> &str {
    resource
        .split("::")
        .next()
        .unwrap_or("")
        .trim_end_matches(|c: char| c.is_ascii_digit())
}

/// Device record for a VISA resource
pub fn visa_device(edge_id: &str, id: &str, resource: &str, description: Option<&str>) -> Device {
    let interface = visa_interface(resource);
    Device {
        id: id.to_string(),
        edge_id: edge_id.to_string(),
        device_name: description.unwrap_or(resource).to_string(),
        device_type: if interface.eq_ignore_ascii_case("GPIB") {
            DeviceType::Gpib
        } else {
            DeviceType::Visa
        },
        model: description.map(str::to_string),
        serial_number: None,
        firmware_version: None,
        driver_version: None,
        ip_address: None,
        slot: None,
        chassis: None,
        is_simulated: false,
    }
}

/// Bounded, slowly mean-reverting random walk for simulated temperatures.
/// Stays mostly around 55 C with a spread of ~4 C, so it occasionally
/// crosses the default 65 C warning threshold (and rarely more).
#[derive(Debug, Clone)]
pub struct SimWalk {
    value: f64,
    rng: u64,
}

impl SimWalk {
    const MEAN: f64 = 55.0;
    const REVERSION: f64 = 0.01;
    const STEP: f64 = 1.0;
    const MIN: f64 = 30.0;
    const MAX: f64 = 85.0;

    pub fn new(seed: u64) -> Self {
        let mut walk = Self {
            value: Self::MEAN,
            rng: seed | 1,
        };
        // de-correlate devices: random start within +-5 C
        walk.value += (walk.uniform() - 0.5) * 10.0;
        walk
    }

    /// Seed from a device id and the current time
    pub fn seeded(id: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let seed = id.bytes().fold(nanos ^ 0xCBF2_9CE4_8422_2325, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100_0000_01B3)
        });
        Self::new(seed)
    }

    fn uniform(&mut self) -> f64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let r = self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (r >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// Advance one sample and return the new temperature
    pub fn next_value(&mut self) -> f64 {
        let step = (self.uniform() * 2.0 - 1.0) * Self::STEP;
        let pull = Self::REVERSION * (Self::MEAN - self.value);
        self.value = (self.value + step + pull).clamp(Self::MIN, Self::MAX);
        // one decimal like real sensors
        self.value = (self.value * 10.0).round() / 10.0;
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovered(product: &str, serial: &str, alias: Option<&str>) -> DiscoveredDevice {
        let mut d = DiscoveredDevice::new(product.to_string(), serial.to_string());
        d.alias = alias.map(str::to_string);
        d
    }

    fn key_of(d: &DiscoveredDevice, counts: &mut HashMap<String, u32>) -> String {
        let mut s = String::new();
        identity_key(d, counts, &mut s);
        s
    }

    #[test]
    fn test_classify_device() {
        assert_eq!(classify_device("PXIe-8880"), DeviceType::Pxi);
        assert_eq!(classify_device("PXI-1042Q"), DeviceType::Pxi);
        assert_eq!(classify_device("cDAQ-9178"), DeviceType::CDaq);
        assert_eq!(classify_device("CompactDAQ-9189"), DeviceType::CDaq);
        assert_eq!(classify_device("USB-6343"), DeviceType::Daq);
        assert_eq!(classify_device("PCI-6221"), DeviceType::Daq);
        assert_eq!(classify_device("NI-XNET"), DeviceType::Xnet);
        assert_eq!(classify_device("PXIe-8510"), DeviceType::Pxi);
        assert_eq!(classify_device("GPIB-USB-HS"), DeviceType::Gpib);
        assert_eq!(classify_device("NIPSPS-4010"), DeviceType::PowerSupply);
        assert_eq!(classify_device("UNKNOWN-MODEL"), DeviceType::Daq);
    }

    #[test]
    fn test_identity_key_priority() {
        let mut counts = HashMap::new();
        assert_eq!(
            key_of(&discovered("NI 9205", "01A2", Some("Mod1")), &mut counts),
            "Mod1"
        );
        assert_eq!(
            key_of(&discovered("NI 9205", "01A2", Some("  ")), &mut counts),
            "01A2"
        );
        let mut d = discovered("PXIe-4081", "", None);
        d.resource_name = Some("PXI1Slot3".into());
        assert_eq!(key_of(&d, &mut counts), "PXI1Slot3");
    }

    #[test]
    fn test_identity_key_fallback_counts_per_product() {
        let mut counts = HashMap::new();
        let a = discovered("PXIe-1082", "", None);
        let b = discovered("cRIO-9045", "", None);
        assert_eq!(key_of(&a, &mut counts), "PXIe-1082#1");
        assert_eq!(key_of(&b, &mut counts), "cRIO-9045#1");
        assert_eq!(key_of(&a, &mut counts), "PXIe-1082#2");
        counts.clear();
        assert_eq!(key_of(&a, &mut counts), "PXIe-1082#1");
    }

    #[test]
    fn test_device_from_discovered() {
        let mut d = discovered("PXIe-6368", "01ABCDEF", Some("PXI1Slot2"));
        d.slot = Some(2);
        let mut id = String::new();
        device_id("edge-1", "PXI1Slot2", &mut id);
        assert_eq!(id, "edge-1:PXI1Slot2");
        let dev = device_from_discovered("edge-1", &id, &d);
        assert_eq!(dev.device_name, "PXI1Slot2");
        assert_eq!(dev.serial_number.as_deref(), Some("01ABCDEF"));
        assert_eq!(dev.model.as_deref(), Some("PXIe-6368"));
        assert_eq!(dev.slot, Some(2));
        let local = local_from_discovered(&d);
        assert_eq!(local.daqmx_name.as_deref(), Some("PXI1Slot2"));
        assert_eq!(local.source, DeviceSource::SysCfg);
        // no alias -> no DAQmx name, no fake serial
        let d = discovered("PXIe-1082", "", None);
        assert!(local_from_discovered(&d).daqmx_name.is_none());
        assert!(device_from_discovered("e", "e:x", &d)
            .serial_number
            .is_none());
    }

    #[test]
    fn test_classify_health_uses_thresholds() {
        let th = Thresholds {
            warning: 50.0,
            critical: 60.0,
        };
        assert_eq!(
            classify_health(true, false, false, Some(40.0), th),
            HealthStatus::Healthy
        );
        assert_eq!(
            classify_health(true, false, false, Some(55.0), th),
            HealthStatus::Warning
        );
        assert_eq!(
            classify_health(true, false, false, Some(60.0), th),
            HealthStatus::Error
        );
        assert_eq!(
            classify_health(false, false, false, Some(20.0), th),
            HealthStatus::Offline
        );
        assert_eq!(
            classify_health(true, true, false, None, th),
            HealthStatus::Error
        );
        assert_eq!(
            classify_health(true, false, true, None, th),
            HealthStatus::Error
        );
        assert_eq!(
            classify_health(true, false, false, Some(f64::NAN), th),
            HealthStatus::Healthy
        );
    }

    #[test]
    fn test_status_from_health_overrides_ni_defaults() {
        let health = DeviceHealth {
            is_reachable: true,
            temperature: Some(70.0), // nimon-ni would say Warning (65/75)
            sensors: Vec::new(),
            self_test_passed: None,
            error_message: None,
            metrics: HashMap::new(),
        };
        let th = Thresholds {
            warning: 80.0,
            critical: 90.0,
        };
        let (status, metrics) = status_from_health(health.clone(), th);
        assert_eq!(status, HealthStatus::Healthy);
        assert!(metrics.contains_key("temperature"));
        let (status, _) = status_from_health(
            health,
            Thresholds {
                warning: 60.0,
                critical: 68.0,
            },
        );
        assert_eq!(status, HealthStatus::Error);
        let (status, _) = status_from_health(DeviceHealth::unreachable(), th);
        assert_eq!(status, HealthStatus::Offline);
    }

    #[test]
    fn test_visa_helpers() {
        assert_eq!(visa_interface("TCPIP0::10.0.0.5::inst0::INSTR"), "TCPIP");
        assert_eq!(visa_interface("GPIB0::1::INSTR"), "GPIB");
        assert_eq!(visa_interface(""), "");
        let d = visa_device("e", "e:GPIB0::1::INSTR", "GPIB0::1::INSTR", None);
        assert_eq!(d.device_type, DeviceType::Gpib);
        assert_eq!(d.device_name, "GPIB0::1::INSTR");
        let d = visa_device("e", "e:x", "USB0::1::INSTR", Some("Keysight 34461A"));
        assert_eq!(d.device_type, DeviceType::Visa);
        assert_eq!(d.device_name, "Keysight 34461A");
    }

    #[test]
    fn test_sim_walk_bounded_and_reaches_warning() {
        let mut walk = SimWalk::new(12345);
        let (mut min, mut max) = (f64::MAX, f64::MIN);
        let mut above_warning = 0;
        for _ in 0..20_000 {
            let v = walk.next_value();
            min = min.min(v);
            max = max.max(v);
            if v >= 65.0 {
                above_warning += 1;
            }
        }
        assert!(min >= SimWalk::MIN && max <= SimWalk::MAX);
        assert!(above_warning > 0, "never reached the warning band");
        assert!(
            above_warning < 2_000,
            "should be occasional: {above_warning}"
        );
        // slow: one step is at most STEP + reversion pull (+ rounding)
        let mut w = SimWalk::new(1);
        let mut prev = w.value();
        for _ in 0..1000 {
            let v = w.next_value();
            assert!((v - prev).abs() <= 1.5, "{prev} -> {v}");
            prev = v;
        }
    }

    #[test]
    fn test_registry() {
        let r = DeviceRegistry::new();
        assert!(r.is_empty());
        r.insert(
            "e:Dev1".into(),
            LocalDevice {
                daqmx_name: Some("Dev1".into()),
                is_simulated: false,
                source: DeviceSource::SysCfg,
            },
        );
        let clone = r.clone();
        assert_eq!(
            clone.get("e:Dev1").unwrap().daqmx_name.as_deref(),
            Some("Dev1")
        );
        r.remove("e:Dev1");
        assert!(clone.get("e:Dev1").is_none());
    }
}
