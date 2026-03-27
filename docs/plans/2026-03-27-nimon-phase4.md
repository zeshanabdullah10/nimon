# NIMon Phase 4: NI API Integration & Persistence

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace simulated device discovery/polling with real NI-SysCfg API calls, add NI-VISA and NI-DAQmx FFI bindings, implement missing database repositories, and wire alerts to auto-trigger remediation actions.

**Architecture:** Edge DeviceManagerActor calls real NI APIs via nimon-ni safe wrappers instead of generating simulated data. Hub persists alerts, predictions, and action history to SQLite. AlertManager automatically triggers ActionExecutor when critical alerts fire.

**Tech Stack:** libloading (FFI), sqlx (persistence), actix (actors), serde (serialization)

---

## Task 1: Complete NI-SysCfg Integration

**Files:**
- Modify: `crates/nimon-ni/src/syscfg/safe.rs`
- Modify: `crates/nimon-ni/src/syscfg/types.rs`
- Modify: `crates/nimon-edge/src/actor/device_manager.rs`

**Goal:** Implement `get_device_health()` in SysCfgSession and wire DeviceManagerActor to use real NI-SysCfg discovery.

**Step 1: Implement get_device_health in safe.rs**

Read `crates/nimon-ni/src/syscfg/safe.rs` and replace the TODO stub `get_device_health()` with a real implementation that queries device properties:

```rust
pub fn get_device_health(&self, device_name: &str) -> NimonResult<DeviceHealth> {
    // Create a session scoped to a specific device
    let mut session_handle: *mut syscfg::ffi::NiSysCfgSessionHandle = std::ptr::null_mut();
    let hostname = std::ffi::CString::new("").unwrap();
    let username = std::ffi::CString::new("").unwrap();
    let password = std::ffi::CString::new("").unwrap();

    unsafe {
        let status = (self.initialize)(
            hostname.as_ptr(),
            username.as_ptr(),
            password.as_ptr(),
            &mut session_handle as *mut _,
        );
        check_status("NiSysCfgInitialize", status)?;

        // Find hardware
        let mut enum_handle: *mut syscfg::ffi::NiSysCfgEnumHandle = std::ptr::null_mut();
        let filter = std::ffi::CString::new(format!("name=={}", device_name)).unwrap();
        let status = (self.find_hardware)(
            session_handle,
            syscfg::ffi::NISYSCFG_SIMPLE_SEARCH,
            filter.as_ptr(),
            &mut enum_handle as *mut _,
        );
        if status != 0 {
            let _ = (self.close_handle)(session_handle as *mut _);
            return Ok(DeviceHealth {
                is_reachable: false,
                temperature: None,
                self_test_passed: None,
                error_message: Some(format!("Device {} not found", device_name)),
            });
        }

        // Get resource
        let mut resource_handle: *mut syscfg::ffi::NiSysCfgResourceHandle = std::ptr::null_mut();
        let status = (self.next_resource)(
            session_handle,
            enum_handle,
            &mut resource_handle as *mut _,
        );
        if status != 0 {
            let _ = (self.close_handle)(enum_handle as *mut _);
            let _ = (self.close_handle)(session_handle as *mut _);
            return Ok(DeviceHealth {
                is_reachable: false,
                temperature: None,
                self_test_passed: None,
                error_message: Some("Failed to enumerate resource".to_string()),
            });
        }

        // Query properties
        let health = self.query_resource_health(resource_handle, device_name);

        // Cleanup
        let _ = (self.close_handle)(resource_handle as *mut _);
        let _ = (self.close_handle)(enum_handle as *mut _);
        let _ = (self.close_handle)(session_handle as *mut _);

        Ok(health)
    }
}
```

Add a private helper `query_resource_health` that reads IS_REACHABLE, TEMPERATURE properties from the resource handle using `get_property`.

**Step 2: Add metrics to DeviceHealth**

In `crates/nimon-ni/src/syscfg/types.rs`, extend DeviceHealth with a metrics HashMap:

```rust
use std::collections::HashMap;
use nimon_core::MetricValue;

pub struct DeviceHealth {
    pub is_reachable: bool,
    pub temperature: Option<f64>,
    pub self_test_passed: Option<bool>,
    pub error_message: Option<String>,
    pub metrics: HashMap<String, MetricValue>,
}

impl DeviceHealth {
    pub fn unreachable(device_name: &str) -> Self {
        Self {
            is_reachable: false,
            temperature: None,
            self_test_passed: None,
            error_message: Some(format!("Device {} not reachable", device_name)),
            metrics: HashMap::new(),
        }
    }

    pub fn to_status_and_metrics(&self) -> (nimon_core::HealthStatus, HashMap<String, MetricValue>) {
        let status = if !self.is_reachable {
            nimon_core::HealthStatus::Offline
        } else if self.error_message.is_some() {
            nimon_core::HealthStatus::Error
        } else if self.temperature.map_or(false, |t| t > 70.0) {
            nimon_core::HealthStatus::Warning
        } else {
            nimon_core::HealthStatus::Healthy
        };

        let mut metrics = self.metrics.clone();
        if let Some(temp) = self.temperature {
            metrics.insert("temperature".to_string(), MetricValue::Float(temp));
        }
        if let Some(passed) = self.self_test_passed {
            metrics.insert("self_test".to_string(), MetricValue::Boolean(passed));
        }

        (status, metrics)
    }
}
```

**Step 3: Wire DeviceManagerActor to use real discovery**

Read `crates/nimon-edge/src/actor/device_manager.rs`. Replace the simulated `discover_devices()` with real NI-SysCfg calls:

```rust
fn discover_devices(&mut self) -> Vec<nimon_core::Device> {
    match nimon_ni::syscfg::safe::NiSysCfg::load() {
        Ok(api) => {
            match api.create_session() {
                Ok(session) => {
                    let discovered = session.discover_devices();
                    info!("Discovered {} NI devices", discovered.len());
                    discovered.into_iter().map(|d| {
                        nimon_core::Device {
                            id: d.serial_number.clone().unwrap_or_else(|| ulid::Ulid::new().to_string()),
                            edge_id: self.edge_id.clone(),
                            device_name: d.product_name.clone(),
                            device_type: Self::classify_device(&d.product_name),
                            model: Some(d.product_name),
                            serial_number: d.serial_number,
                            firmware_version: d.firmware_version,
                            driver_version: d.driver_version,
                            ip_address: d.ip_address,
                            slot: None,
                            chassis: None,
                        }
                    }).collect()
                }
                Err(e) => {
                    warn!("Failed to create NI-SysCfg session: {}", e);
                    self.fallback_simulated_devices()
                }
            }
        }
        Err(e) => {
            warn!("NI-SysCfg not available: {}", e);
            info!("Using simulated devices (NI drivers not installed)");
            self.fallback_simulated_devices()
        }
    }
}

fn classify_device(product_name: &str) -> nimon_core::DeviceType {
    let name = product_name.to_uppercase();
    if name.contains("PXI") || name.contains("CHASSIS") {
        nimon_core::DeviceType::Pxi
    } else if name.contains("CDAQ") || name.contains("COMPACTDAQ") {
        nimon_core::DeviceType::CDaq
    } else if name.contains("DAQ") {
        nimon_core::DeviceType::Daq
    } else if name.contains("GPIB") {
        nimon_core::DeviceType::Gpib
    } else if name.contains("XNET") || name.contains("CAN") {
        nimon_core::DeviceType::Xnet
    } else if name.contains("POWER") || name.contains("PS") || name.contains("DC") {
        nimon_core::DeviceType::PowerSupply
    } else {
        nimon_core::DeviceType::Visa
    }
}

fn fallback_simulated_devices(&self) -> Vec<nimon_core::Device> {
    // Keep existing simulated devices for development/testing
    vec![
        nimon_core::Device {
            id: "SIM-DAQ-001".to_string(),
            edge_id: self.edge_id.clone(),
            device_name: "Simulated DAQ".to_string(),
            device_type: nimon_core::DeviceType::Daq,
            model: Some("Simulated".to_string()),
            serial_number: None,
            firmware_version: None,
            driver_version: None,
            ip_address: None,
            slot: None,
            chassis: None,
        },
    ]
}
```

**Step 4: Wire DeviceActor to use real health polling**

In `crates/nimon-edge/src/actor/device_actor.rs`, replace simulated polling:

```rust
fn poll_device(&mut self) {
    if let Some(recipient) = &self.status_recipient {
        let device_id = self.device.id.clone();
        let edge_id = self.device.edge_id.clone();

        let (status, metrics) = match nimon_ni::syscfg::safe::NiSysCfg::load()
            .ok()
            .and_then(|api| api.create_session().ok())
        {
            Some(session) => {
                match session.get_device_health(&self.device.device_name) {
                    Ok(health) => health.to_status_and_metrics(),
                    Err(_) => {
                        // Fallback to simulated data if health check fails
                        (nimon_core::HealthStatus::Healthy, self.simulated_metrics())
                    }
                }
            }
            None => (nimon_core::HealthStatus::Healthy, self.simulated_metrics()),
        };

        let msg = DeviceStatusUpdate {
            device_id,
            edge_id,
            status,
            metrics,
            timestamp: chrono::Utc::now(),
        };
        let _ = recipient.send(msg);
    }
}

fn simulated_metrics(&self) -> HashMap<String, MetricValue> {
    let temp = 40.0 + (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() % 30) as f64 * 0.5;
    let mut m = HashMap::new();
    m.insert("temperature".to_string(), MetricValue::Float(temp));
    m
}
```

**Step 5: Run tests**

Run: `cargo test --workspace`
Expected: All tests pass (simulated fallback means no NI hardware required)

**Step 6: Commit**

```bash
git add crates/nimon-ni/src/syscfg/ crates/nimon-edge/src/actor/
git commit -m "feat(ni): wire device discovery and health polling to NI-SysCfg API"
```

---

## Task 2: NI-VISA FFI Bindings

**Files:**
- Create: `crates/nimon-ni/src/visa/mod.rs`
- Create: `crates/nimon-ni/src/visa/ffi.rs`
- Create: `crates/nimon-ni/src/visa/types.rs`
- Create: `crates/nimon-ni/src/visa/safe.rs`
- Modify: `crates/nimon-ni/src/lib.rs`

**Goal:** Add NI-VISA FFI bindings following the same pattern as NI-SysCfg.

**Step 1: Create visa/ffi.rs**

```rust
//! Raw FFI bindings for NI-VISA (visa32.dll)

use std::os::raw::{c_char, c_int, c_ulong, c_void};

/// VISA session handle
#[repr(C)]
pub struct ViSession {
    _opaque: [u8; 0],
}

/// VISA object handle (for find operations)
#[repr(C)]
pub struct ViObject {
    _opaque: [u8; 0],
}

// Status codes
pub const VI_SUCCESS: c_long = 0;
pub const VI_ERROR_RESOURCE_NOT_FOUND: c_long = -1073807343;
pub const VI_ERROR_RSRC_NFOUND: c_long = -1073807343;
pub const VI_ERROR_TIMEOUT: c_long = -1073807339;

pub type ViStatus = c_long;
pub type ViRsrc = *const c_char;
pub type ViBuf = *mut c_char;
pub type ViUInt32 = c_ulong;
pub type ViAttr = c_uint;

// Attribute IDs
pub const VI_ATTR_RSRC_NAME: ViAttr = 0xBFFF0004;
pub const VI_ATTR_RSRC_CLASS: ViAttr = 0xBFFF0003;
pub const VI_ATTR_RSRC_IMPL_VERSION: ViAttr = 0x3FFF0004;
pub const VI_ATTR_TMO_VALUE: ViAttr = 0x3FFF003A;
pub const VI_ATTR_INTF_TYPE: ViAttr = 0x3FFF0001;
pub const VI_ATTR_INTF_INST_NAME: ViAttr = 0x3FFF0011;

// Interface type constants
pub const VI_INTF_TCPIP: c_uint = 6;
pub const VI_INTF_GPIB: c_uint = 1;
pub const VI_INTF_ASRL: c_uint = 4;
pub const VI_INTF_USB: c_uint = 3;
pub const VI_INTF_PXI: c_uint = 5;

pub type ViOpenDefaultRM = unsafe extern "system" fn(*mut ViSession) -> ViStatus;
pub type ViClose = unsafe extern "system" fn(ViObject) -> ViStatus;
pub type ViFindRsrc = unsafe extern "system" fn(ViSession, ViRsrc, *mut ViUInt32, ViBuf, ViUInt32) -> ViStatus;
pub type ViFindNext = unsafe extern "system" fn(ViSession, ViBuf) -> ViStatus;
pub type ViOpen = unsafe extern "system" fn(ViSession, ViRsrc, ViUInt32, ViUInt32, *mut ViSession) -> ViStatus;
pub type ViWrite = unsafe extern "system" fn(ViSession, ViBuf, ViUInt32, *mut ViUInt32) -> ViStatus;
pub type ViRead = unsafe extern "system" fn(ViSession, ViBuf, ViUInt32, *mut ViUInt32) -> ViStatus;
pub type ViGetAttribute = unsafe extern "system" fn(ViSession, ViAttr, *mut c_void) -> ViStatus;
pub type ViSetAttribute = unsafe extern "system" fn(ViSession, ViAttr, ViAttr) -> ViStatus;
```

**Step 2: Create visa/types.rs**

```rust
//! NI-VISA discovered instrument types

use std::collections::HashMap;
use nimon_core::MetricValue;

/// A discovered VISA instrument
#[derive(Debug, Clone)]
pub struct VisaInstrument {
    pub resource_name: String,
    pub interface_type: String,  // "TCPIP", "GPIB", "ASRL", "USB", "PXI"
    pub description: Option<String>,
    pub is_reachable: bool,
    pub response_time_ms: Option<u64>,
}

impl VisaInstrument {
    pub fn new(resource_name: String, interface_type: String) -> Self {
        Self {
            resource_name,
            interface_type,
            description: None,
            is_reachable: true,
            response_time_ms: None,
        }
    }
}

/// Health check result for a VISA instrument
#[derive(Debug, Clone)]
pub struct VisaHealth {
    pub is_reachable: bool,
    pub idn_response: Option<String>,
    pub response_time_ms: Option<u64>,
    pub timeout_count: u32,
    pub error_message: Option<String>,
    pub metrics: HashMap<String, MetricValue>,
}

impl VisaHealth {
    pub fn to_status_and_metrics(&self) -> (nimon_core::HealthStatus, HashMap<String, MetricValue>) {
        let status = if !self.is_reachable {
            nimon_core::HealthStatus::Offline
        } else if self.error_message.is_some() {
            nimon_core::HealthStatus::Error
        } else if self.timeout_count > 5 {
            nimon_core::HealthStatus::Warning
        } else {
            nimon_core::HealthStatus::Healthy
        };

        let mut metrics = self.metrics.clone();
        if let Some(rt) = self.response_time_ms {
            metrics.insert("response_time_ms".to_string(), MetricValue::Integer(rt as i64));
        }
        metrics.insert("timeout_count".to_string(), MetricValue::Integer(self.timeout_count as i64));

        (status, metrics)
    }
}
```

**Step 3: Create visa/safe.rs**

```rust
//! Safe Rust wrapper for NI-VISA

use libloading::{Library, Symbol};
use tracing::{debug, warn};

use nimon_core::NimonResult;

use super::ffi::*;
use super::types::*;

/// DLL name for NI-VISA
const VISA_DLL: &str = "visa64.dll";

/// Safe wrapper for NI-VISA
pub struct NiVisa {
    library: Library,
    open_default_rm: Symbol<ViOpenDefaultRM>,
    close: Symbol<ViClose>,
    find_rsrc: Symbol<ViFindRsrc>,
    find_next: Symbol<ViFindNext>,
    open: Symbol<ViOpen>,
    write: Symbol<ViWrite>,
    read: Symbol<ViRead>,
    get_attribute: Symbol<ViGetAttribute>,
}

impl NiVisa {
    pub fn load() -> NimonResult<Self> {
        let library = unsafe { Library::new(VISA_DLL) }
            .map_err(|e| nimon_core::NimonError::NiApiError(format!(
                "Failed to load {}: {}", VISA_DLL, e
            )))?;

        Ok(Self {
            open_default_rm: unsafe { library.get(b"viOpenDefaultRM")? },
            close: unsafe { library.get(b"viClose")? },
            find_rsrc: unsafe { library.get(b"viFindRsrc")? },
            find_next: unsafe { library.get(b"viFindNext")? },
            open: unsafe { library.get(b"viOpen")? },
            write: unsafe { library.get(b"viWrite")? },
            read: unsafe { library.get(b"viRead")? },
            get_attribute: unsafe { library.get(b"viGetAttribute")? },
        })
    }

    pub fn is_available() -> bool {
        unsafe { Library::new(VISA_DLL).is_ok() }
    }

    pub fn create_session(&self) -> NimonResult<VisaSession> {
        let mut session: ViSession = ViSession { _opaque: [] };
        let status = unsafe { (self.open_default_rm)(&mut session) };
        check_visa_status("viOpenDefaultRM", status)?;
        Ok(VisaSession { session, api: self })
    }
}

/// A VISA resource manager session
pub struct VisaSession<'a> {
    session: ViSession,
    api: &'a NiVisa,
}

impl<'a> VisaSession<'a> {
    /// Discover all VISA instruments
    pub fn discover_instruments(&self) -> Vec<VisaInstrument> {
        let mut instruments = Vec::new();
        let mut find_handle: ViUInt32 = 0;
        let mut buffer = [0u8; 256];

        let status = unsafe {
            (self.api.find_rsrc)(
                self.session,
                b"?*INSTR\0".as_ptr() as *const i8,
                &mut find_handle,
                buffer.as_mut_ptr() as *mut i8,
                256,
            )
        };

        if status != VI_SUCCESS {
            debug!("viFindRsrc returned {}: no VISA instruments found", status);
            return instruments;
        }

        // First instrument found
        if let Ok(name) = c_string_from_buffer(&buffer) {
            instruments.push(VisaInstrument::new(name, "UNKNOWN".to_string()));
        }

        // Find remaining
        loop {
            let status = unsafe {
                (self.api.find_next)(self.session, buffer.as_mut_ptr() as *mut i8)
            };
            if status != VI_SUCCESS {
                break;
            }
            if let Ok(name) = c_string_from_buffer(&buffer) {
                instruments.push(VisaInstrument::new(name, "UNKNOWN".to_string()));
            }
        }

        instruments
    }

    /// Check health of a VISA instrument
    pub fn get_instrument_health(&self, resource_name: &str) -> NimonResult<VisaHealth> {
        let resource_cstr = std::ffi::CString::new(resource_name)?;
        let mut instr_session: ViSession = ViSession { _opaque: [] };

        // Open instrument
        let status = unsafe {
            (self.api.open)(
                self.session,
                resource_cstr.as_ptr(),
                0, // no exclusive access
                3000, // 3 second timeout
                &mut instr_session,
            )
        };

        if status != VI_SUCCESS {
            return Ok(VisaHealth {
                is_reachable: false,
                idn_response: None,
                response_time_ms: None,
                timeout_count: 0,
                error_message: Some(format!("Cannot open: status {}", status)),
                metrics: std::collections::HashMap::new(),
            });
        }

        // Send *IDN? query
        let idn_cmd = std::ffi::CString::new("*IDN?\n").unwrap();
        let mut ret_count: ViUInt32 = 0;
        let start = std::time::Instant::now();

        let write_status = unsafe {
            (self.api.write)(
                instr_session,
                idn_cmd.as_ptr() as *mut i8,
                7,
                &mut ret_count,
            )
        };

        let response_time = start.elapsed().as_millis() as u64;

        let idn_response = if write_status == VI_SUCCESS {
            let mut read_buffer = [0u8; 256];
            let read_status = unsafe {
                (self.api.read)(
                    instr_session,
                    read_buffer.as_mut_ptr() as *mut i8,
                    256,
                    &mut ret_count,
                )
            };
            if read_status == VI_SUCCESS {
                Some(String::from_utf8_lossy(&read_buffer[..ret_count as usize]).trim().to_string())
            } else {
                None
            }
        } else {
            None
        };

        // Close instrument session
        unsafe {
            let _ = (self.api.close)(instr_session);
        }

        Ok(VisaHealth {
            is_reachable: true,
            idn_response,
            response_time_ms: Some(response_time),
            timeout_count: 0,
            error_message: None,
            metrics: std::collections::HashMap::new(),
        })
    }
}

impl<'a> Drop for VisaSession<'a> {
    fn drop(&mut self) {
        unsafe {
            let _ = (self.api.close)(self.session);
        }
    }
}

fn check_visa_status(api_name: &str, status: i32) -> NimonResult<()> {
    if status >= 0 {
        Ok(())
    } else {
        Err(nimon_core::NimonError::NiApiError(format!(
            "{} failed with status {}", api_name, status
        )))
    }
}

fn c_string_from_buffer(buffer: &[u8]) -> Result<String, std::string::FromUtf8Error> {
    let len = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    String::from_utf8(buffer[..len].to_vec())
}
```

**Step 4: Create visa/mod.rs**

```rust
//! NI-VISA instrument communication

pub mod ffi;
pub mod safe;
pub mod types;

pub use safe::{NiVisa, VisaSession};
pub use types::{VisaHealth, VisaInstrument};
```

**Step 5: Update nimon-ni lib.rs**

Read `crates/nimon-ni/src/lib.rs` and add `pub mod visa;`

**Step 6: Run tests**

Run: `cargo test -p nimon-ni`
Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/nimon-ni/src/visa/ crates/nimon-ni/src/lib.rs
git commit -m "feat(ni): add NI-VISA FFI bindings and safe wrapper"
```

---

## Task 3: NI-DAQmx FFI Bindings

**Files:**
- Create: `crates/nimon-ni/src/daqmx/mod.rs`
- Create: `crates/nimon-ni/src/daqmx/ffi.rs`
- Create: `crates/nimon-ni/src/daqmx/types.rs`
- Create: `crates/nimon-ni/src/daqmx/safe.rs`
- Modify: `crates/nimon-ni/src/lib.rs`

**Goal:** Add NI-DAQmx FFI bindings for device discovery and health monitoring.

**Step 1: Create daqmx/ffi.rs**

```rust
//! Raw FFI bindings for NI-DAQmx (nicaiu.dll)

use std::os::raw::{c_char, c_double, c_int, c_void};

pub type DaqmxStatus = c_int;

// Status codes
pub const DAQMX_SUCCESS: c_int = 0;
pub const DAQMX_ERROR_DEVICE_NOT_FOUND: c_int = -200220;

// Opaque handles
#[repr(C)]
pub struct TaskHandle {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct DaqmxDevice {
    _opaque: [u8; 0],
}

// Function pointer types
pub type DAQmxGetSysDevNames = unsafe extern "system" fn(*mut c_char, u32) -> DaqmxStatus;
pub type DAQmxGetDevProductType = unsafe extern "system" fn(*const c_char, *mut c_char, u32) -> DaqmxStatus;
pub type DAQmxGetDevSerialNum = unsafe extern "system" fn(*const c_char, *mut u32) -> DaqmxStatus;
pub type DAQmxGetDevTemperature = unsafe extern "system" fn(*const c_char, *mut c_double) -> DaqmxStatus;
pub type DAQmxGetDevSelfTestResult = unsafe extern "system" fn(*const c_char, *mut c_int) -> DaqmxStatus;
pub type DAQmxGetDevAIPowerSupplyVoltages = unsafe extern "system" fn(
    *const c_char, *mut c_double, *mut c_double, *mut c_double, *mut c_double,
) -> DaqmxStatus;
pub type DAQmxGetDevProductNumber = unsafe extern "system" fn(*const c_char, *mut u32) -> DaqmxStatus;
pub type DAQmxResetDevice = unsafe extern "system" fn(*const c_char) -> DaqmxStatus;
```

**Step 2: Create daqmx/types.rs**

```rust
//! NI-DAQmx device types

use std::collections::HashMap;
use nimon_core::MetricValue;

/// A discovered DAQ device
#[derive(Debug, Clone)]
pub struct DaqDevice {
    pub product_name: String,
    pub product_number: Option<u32>,
    pub serial_number: Option<u32>,
    pub device_name: String,
}

/// DAQ device health information
#[derive(Debug, Clone)]
pub struct DaqHealth {
    pub temperature: Option<f64>,
    pub self_test_passed: Option<bool>,
    pub voltage_5v: Option<f64>,
    pub voltage_3v3: Option<f64>,
    pub voltage_user: Option<f64>,
    pub voltage_negative_user: Option<f64>,
    pub error_message: Option<String>,
    pub metrics: HashMap<String, MetricValue>,
}

impl DaqHealth {
    pub fn to_status_and_metrics(&self) -> (nimon_core::HealthStatus, HashMap<String, MetricValue>) {
        let status = if self.error_message.is_some() {
            nimon_core::HealthStatus::Error
        } else if self.self_test_passed == Some(false) {
            nimon_core::HealthStatus::Error
        } else if self.temperature.map_or(false, |t| t > 70.0) {
            nimon_core::HealthStatus::Warning
        } else {
            nimon_core::HealthStatus::Healthy
        };

        let mut metrics = self.metrics.clone();
        if let Some(t) = self.temperature {
            metrics.insert("temperature".to_string(), MetricValue::Float(t));
        }
        if let Some(v) = self.voltage_5v {
            metrics.insert("voltage_5v".to_string(), MetricValue::Float(v));
        }
        if let Some(v) = self.voltage_3v3 {
            metrics.insert("voltage_3v3".to_string(), MetricValue::Float(v));
        }
        if let Some(passed) = self.self_test_passed {
            metrics.insert("self_test".to_string(), MetricValue::Boolean(passed));
        }

        (status, metrics)
    }
}
```

**Step 3: Create daqmx/safe.rs**

```rust
//! Safe wrapper for NI-DAQmx

use libloading::{Library, Symbol};
use tracing::warn;

use nimon_core::NimonResult;

use super::ffi::*;
use super::types::*;

const DAQMX_DLL: &str = "nicaiu.dll";

/// Safe wrapper for NI-DAQmx
pub struct NiDaqMx {
    library: Library,
    get_sys_dev_names: Symbol<DAQmxGetSysDevNames>,
    get_product_type: Symbol<DAQmxGetDevProductType>,
    get_serial_num: Symbol<DAQmxGetDevSerialNum>,
    get_temperature: Symbol<DAQmxGetDevTemperature>,
    get_self_test: Symbol<DAQmxGetDevSelfTestResult>,
    get_ai_power: Symbol<DAQmxGetDevAIPowerSupplyVoltages>,
    get_product_number: Symbol<DAQmxGetDevProductNumber>,
    reset_device: Symbol<DAQmxResetDevice>,
}

impl NiDaqMx {
    pub fn load() -> NimonResult<Self> {
        let library = unsafe { Library::new(DAQMX_DLL) }
            .map_err(|e| nimon_core::NimonError::NiApiError(format!(
                "Failed to load {}: {}", DAQMX_DLL, e
            )))?;

        Ok(Self {
            get_sys_dev_names: unsafe { library.get(b"DAQmxGetSysDevNames")? },
            get_product_type: unsafe { library.get(b"DAQmxGetDevProductTypeName")? },
            get_serial_num: unsafe { library.get(b"DAQmxGetDevSerialNum")? },
            get_temperature: unsafe { library.get(b"DAQmxGetDevTemperature")? },
            get_self_test: unsafe { library.get(b"DAQmxGetDevSelfTestResult")? },
            get_ai_power: unsafe { library.get(b"DAQmxGetDevAIPowerSupplyVoltages")? },
            get_product_number: unsafe { library.get(b"DAQmxGetDevProductNumber")? },
            reset_device: unsafe { library.get(b"DAQmxResetDevice")? },
        })
    }

    pub fn is_available() -> bool {
        unsafe { Library::new(DAQMX_DLL).is_ok() }
    }

    /// Discover all DAQ devices
    pub fn discover_devices(&self) -> Vec<DaqDevice> {
        let mut devices = Vec::new();
        let mut names_buffer = [0u8; 4096];

        let status = unsafe {
            (self.get_sys_dev_names)(
                names_buffer.as_mut_ptr() as *mut i8,
                4096,
            )
        };

        if status != DAQMX_SUCCESS {
            warn!("DAQmxGetSysDevNames failed: {}", status);
            return devices;
        }

        // Parse comma-separated device names
        let names_str = String::from_utf8_lossy(&names_buffer);
        for name in names_str.split(',').filter(|s| !s.is_empty()) {
            let name = name.trim();
            let device = self.get_device_info(name);
            devices.push(device);
        }

        devices
    }

    fn get_device_info(&self, device_name: &str) -> DaqDevice {
        let name_cstr = std::ffi::CString::new(device_name).unwrap();

        let mut product_name = [0u8; 256];
        let mut serial_num: u32 = 0;
        let mut product_number: u32 = 0;

        unsafe {
            (self.get_product_type)(
                name_cstr.as_ptr(),
                product_name.as_mut_ptr() as *mut i8,
                256,
            );
            (self.get_serial_num)(name_cstr.as_ptr(), &mut serial_num);
            (self.get_product_number)(name_cstr.as_ptr(), &mut product_number);
        }

        let product = String::from_utf8_lossy(&product_name)
            .trim_end_matches('\0')
            .to_string();

        DaqDevice {
            product_name: product,
            product_number: if product_number > 0 { Some(product_number) } else { None },
            serial_number: if serial_num > 0 { Some(serial_num) } else { None },
            device_name: device_name.to_string(),
        }
    }

    /// Get health info for a DAQ device
    pub fn get_device_health(&self, device_name: &str) -> NimonResult<DaqHealth> {
        let name_cstr = std::ffi::CString::new(device_name)?;

        let mut temperature: c_double = 0.0;
        let mut self_test: c_int = 0;
        let mut v5: c_double = 0.0;
        let mut v3: c_double = 0.0;
        let mut v_user: c_double = 0.0;
        let mut v_neg_user: c_double = 0.0;

        let temp_status = unsafe { (self.get_temperature)(name_cstr.as_ptr(), &mut temperature) };
        let test_status = unsafe { (self.get_self_test)(name_cstr.as_ptr(), &mut self_test) };
        let _ = unsafe {
            (self.get_ai_power)(name_cstr.as_ptr(), &mut v5, &mut v3, &mut v_user, &mut v_neg_user)
        };

        Ok(DaqHealth {
            temperature: if temp_status == DAQMX_SUCCESS { Some(temperature) } else { None },
            self_test_passed: if test_status == DAQMX_SUCCESS { Some(self_test == 0) } else { None },
            voltage_5v: Some(v5),
            voltage_3v3: Some(v3),
            voltage_user: Some(v_user),
            voltage_negative_user: Some(v_neg_user),
            error_message: if test_status == DAQMX_SUCCESS && self_test != 0 {
                Some(format!("Self-test failed with code {}", self_test))
            } else {
                None
            },
            metrics: HashMap::new(),
        })
    }

    /// Reset a DAQ device
    pub fn reset_device(&self, device_name: &str) -> NimonResult<()> {
        let name_cstr = std::ffi::CString::new(device_name)?;
        let status = unsafe { (self.reset_device)(name_cstr.as_ptr()) };
        if status == DAQMX_SUCCESS {
            Ok(())
        } else {
            Err(nimon_core::NimonError::NiApiError(format!(
                "DAQmxResetDevice failed: {}", status
            )))
        }
    }
}
```

**Step 4: Create daqmx/mod.rs**

```rust
//! NI-DAQmx device monitoring

pub mod ffi;
pub mod safe;
pub mod types;

pub use safe::NiDaqMx;
pub use types::{DaqDevice, DaqHealth};
```

**Step 5: Update nimon-ni lib.rs**

Add `pub mod daqmx;`

**Step 6: Run tests**

Run: `cargo test -p nimon-ni`
Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/nimon-ni/src/daqmx/ crates/nimon-ni/src/lib.rs
git commit -m "feat(ni): add NI-DAQmx FFI bindings for device discovery and health"
```

---

## Task 4: Database Repositories for Alerts, Predictions, Actions

**Files:**
- Create: `crates/nimon-core/src/db/alert_repo.rs`
- Create: `crates/nimon-core/src/db/prediction_repo.rs`
- Create: `crates/nimon-core/src/db/action_repo.rs`
- Modify: `crates/nimon-core/src/db/mod.rs`

**Goal:** Implement repository structs for persisting alerts, predictions, and action history to SQLite.

**Step 1: Create alert_repo.rs**

```rust
//! Alert repository for SQLite persistence

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use nimon_core::NimonResult;

/// Alert record from database
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AlertRecord {
    pub id: i64,
    pub device_id: Option<String>,
    pub edge_id: Option<String>,
    pub rule_name: String,
    pub severity: String,
    pub message: String,
    pub channels: Option<String>,
    pub status: String,
    pub action_taken: Option<String>,
    pub action_result: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

pub struct AlertRepository {
    pool: SqlitePool,
}

impl AlertRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(
        &self,
        device_id: Option<&str>,
        edge_id: Option<&str>,
        rule_name: &str,
        severity: &str,
        message: &str,
    ) -> NimonResult<i64> {
        let result = sqlx::query(
            "INSERT INTO alerts (device_id, edge_id, rule_name, severity, message, status)
             VALUES (?, ?, ?, ?, ?, 'pending')"
        )
        .bind(device_id)
        .bind(edge_id)
        .bind(rule_name)
        .bind(severity)
        .bind(message)
        .execute(&self.pool)
        .await
        .map_err(|e| nimon_core::NimonError::DatabaseError(e.to_string()))?;

        Ok(result.last_insert_rowid())
    }

    pub async fn resolve(&self, alert_id: i64) -> NimonResult<()> {
        sqlx::query("UPDATE alerts SET status = 'resolved', resolved_at = ? WHERE id = ?")
            .bind(Utc::now().to_rfc3339())
            .bind(alert_id)
            .execute(&self.pool)
            .await
            .map_err(|e| nimon_core::NimonError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    pub async fn list_active(&self) -> NimonResult<Vec<AlertRecord>> {
        let records = sqlx::query_as::<_, AlertRecord>(
            "SELECT * FROM alerts WHERE status != 'resolved' ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| nimon_core::NimonError::DatabaseError(e.to_string()))?;

        Ok(records)
    }

    pub async fn list_by_device(&self, device_id: &str, limit: i64) -> NimonResult<Vec<AlertRecord>> {
        let records = sqlx::query_as::<_, AlertRecord>(
            "SELECT * FROM alerts WHERE device_id = ? ORDER BY created_at DESC LIMIT ?"
        )
        .bind(device_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| nimon_core::NimonError::DatabaseError(e.to_string()))?;

        Ok(records)
    }
}
```

**Step 2: Create prediction_repo.rs**

```rust
//! Prediction repository for SQLite persistence

use sqlx::SqlitePool;

use nimon_core::NimonResult;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PredictionRecord {
    pub id: i64,
    pub device_id: String,
    pub edge_id: String,
    pub prediction_type: String,
    pub probability: f64,
    pub eta_minutes: Option<i32>,
    pub features: Option<String>,
    pub model_version: Option<String>,
    pub status: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

pub struct PredictionRepository {
    pool: SqlitePool,
}

impl PredictionRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(
        &self,
        device_id: &str,
        edge_id: &str,
        prediction_type: &str,
        probability: f64,
        eta_minutes: Option<i32>,
        model_version: Option<&str>,
    ) -> NimonResult<i64> {
        let result = sqlx::query(
            "INSERT INTO predictions (device_id, edge_id, prediction_type, probability, eta_minutes, model_version, status)
             VALUES (?, ?, ?, ?, ?, ?, 'active')"
        )
        .bind(device_id)
        .bind(edge_id)
        .bind(prediction_type)
        .bind(probability)
        .bind(eta_minutes)
        .bind(model_version)
        .execute(&self.pool)
        .await
        .map_err(|e| nimon_core::NimonError::DatabaseError(e.to_string()))?;

        Ok(result.last_insert_rowid())
    }

    pub async fn list_active(&self) -> NimonResult<Vec<PredictionRecord>> {
        let records = sqlx::query_as::<_, PredictionRecord>(
            "SELECT * FROM predictions WHERE status = 'active' ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| nimon_core::NimonError::DatabaseError(e.to_string()))?;

        Ok(records)
    }

    pub async fn dismiss(&self, prediction_id: i64) -> NimonResult<()> {
        sqlx::query("UPDATE predictions SET status = 'dismissed' WHERE id = ?")
            .bind(prediction_id)
            .execute(&self.pool)
            .await
            .map_err(|e| nimon_core::NimonError::DatabaseError(e.to_string()))?;
        Ok(())
    }
}
```

**Step 3: Create action_repo.rs**

```rust
//! Action history repository for SQLite persistence

use sqlx::SqlitePool;

use nimon_core::NimonResult;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActionRecord {
    pub id: i64,
    pub alert_id: Option<i64>,
    pub device_id: String,
    pub action_id: String,
    pub action_type: String,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub output: Option<String>,
    pub duration_ms: Option<i64>,
    pub success: bool,
    pub retry_count: i32,
    pub executed_at: String,
}

pub struct ActionRepository {
    pool: SqlitePool,
}

impl ActionRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(
        &self,
        alert_id: Option<i64>,
        device_id: &str,
        action_id: &str,
        action_type: &str,
        command: Option<&str>,
        exit_code: Option<i32>,
        output: Option<&str>,
        duration_ms: Option<u64>,
        success: bool,
        retry_count: i32,
    ) -> NimonResult<i64> {
        let result = sqlx::query(
            "INSERT INTO action_history (alert_id, device_id, action_id, action_type, command, exit_code, output, duration_ms, success, retry_count)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(alert_id)
        .bind(device_id)
        .bind(action_id)
        .bind(action_type)
        .bind(command)
        .bind(exit_code)
        .bind(output)
        .bind(duration_ms.map(|d| d as i64))
        .bind(success)
        .bind(retry_count)
        .execute(&self.pool)
        .await
        .map_err(|e| nimon_core::NimonError::DatabaseError(e.to_string()))?;

        Ok(result.last_insert_rowid())
    }

    pub async fn list_by_device(&self, device_id: &str, limit: i64) -> NimonResult<Vec<ActionRecord>> {
        let records = sqlx::query_as::<_, ActionRecord>(
            "SELECT * FROM action_history WHERE device_id = ? ORDER BY executed_at DESC LIMIT ?"
        )
        .bind(device_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| nimon_core::NimonError::DatabaseError(e.to_string()))?;

        Ok(records)
    }
}
```

**Step 4: Update db/mod.rs**

Read `crates/nimon-core/src/db/mod.rs` and add:
```rust
pub mod alert_repo;
pub mod prediction_repo;
pub mod action_repo;

pub use alert_repo::{AlertRecord, AlertRepository};
pub use prediction_repo::{PredictionRecord, PredictionRepository};
pub use action_repo::{ActionRecord, ActionRepository};
```

**Step 5: Run tests**

Run: `cargo test -p nimon-core`
Expected: All tests pass

**Step 6: Commit**

```bash
git add crates/nimon-core/src/db/
git commit -m "feat(core): add alert, prediction, and action history repositories"
```

---

## Task 5: Wire Alerts to Auto-Actions

**Files:**
- Modify: `crates/nimon-hub/src/alert/manager.rs`
- Modify: `crates/nimon-hub/src/server/mod.rs`

**Goal:** When critical alerts fire, automatically trigger configured remediation actions.

**Step 1: Add action mapping to AlertManager**

Read `crates/nimon-hub/src/alert/manager.rs`. Add an `action_rules` field and auto-execute logic:

In AlertManager struct, add:
```rust
action_executor: Option<actix::Addr<ActionExecutor>>,
```

Add a method to link the executor:
```rust
pub fn with_action_executor(mut self, addr: actix::Addr<ActionExecutor>) -> Self {
    self.action_executor = Some(addr);
    self
}
```

In `process_evaluation()`, after creating an alert, check if it's critical and auto-execute:

```rust
// Auto-execute action for critical alerts
if alert.severity == AlertSeverity::Critical {
    if let Some(executor) = &self.action_executor {
        let action = Action::restart_service(
            format!("auto-restart-{}", ctx.device_id),
            format!("Auto restart for {}", ctx.device_id),
            "nimon-edge".to_string(),
        );
        let action_ctx = ActionContext {
            edge_id: ctx.edge_id.clone(),
            device_id: ctx.device_id.clone(),
            alert_id: alert.id.clone(),
            attempt: 1,
            variables: HashMap::new(),
        };
        executor.do_send(crate::action::executor::ExecuteAction {
            action,
            context: action_ctx,
        });
        info!("Auto-action triggered for critical alert: {}", alert.id);
    }
}
```

**Step 2: Wire ActionExecutor in hub server run()**

Read `crates/nimon-hub/src/server/mod.rs`. In the `run()` function, start the ActionExecutor and link it to AlertManager:

```rust
// Start action executor
let action_executor = ActionExecutor::new().start();

// Start alert manager with action executor
let mut alert_manager = AlertManager::new(
    AlertManagerConfig::default(),
    state.sessions().clone(),
);
alert_manager = alert_manager.with_action_executor(action_executor.clone());
let alert_manager_addr = alert_manager.start();
```

**Step 3: Run tests**

Run: `cargo test -p nimon-hub`
Expected: All tests pass

**Step 4: Commit**

```bash
git add crates/nimon-hub/src/alert/manager.rs crates/nimon-hub/src/server/mod.rs
git commit -m "feat(hub): auto-execute remediation actions on critical alerts"
```

---

## Task 6: Persist Alerts and Predictions to Database

**Files:**
- Modify: `crates/nimon-hub/src/alert/manager.rs`
- Modify: `crates/nimon-hub/src/server/mod.rs`

**Goal:** Persist generated alerts and received predictions to the SQLite database.

**Step 1: Add database pool to AlertManager**

Read `crates/nimon-hub/src/alert/manager.rs`. Add database pool and repository fields:

```rust
use nimon_core::db::{AlertRepository, PredictionRepository, SqlitePool};

pub struct AlertManager {
    config: AlertManagerConfig,
    rules: Vec<AlertRule>,
    active_alerts: DashMap<String, ActiveAlert>,
    sessions: SessionStore,
    notification_tx: tokio::sync::mpsc::UnboundedSender<Alert>,
    alert_repo: Option<AlertRepository>,
    prediction_repo: Option<PredictionRepository>,
}
```

Update `new()` to accept pool:
```rust
pub fn new(config: AlertManagerConfig, sessions: SessionStore, pool: Option<SqlitePool>) -> Self {
    let alert_repo = pool.as_ref().map(|p| AlertRepository::new(p.clone()));
    let prediction_repo = pool.as_ref().map(|p| PredictionRepository::new(p.clone()));
    // ...
}
```

**Step 2: Persist alerts on creation**

In `process_evaluation()`, after creating each alert, persist it:
```rust
// Persist to database
if let Some(repo) = &self.alert_repo {
    if let Err(e) = tokio::spawn(repo.insert(
        Some(&ctx.device_id),
        Some(&ctx.edge_id),
        &rule.name,
        &alert.severity.to_string(),
        &alert.message,
    )).await {
        warn!("Failed to persist alert: {}", e);
    }
}
```

Similarly in the `PredictionResult` handler, persist predictions.

**Step 3: Add pool to server run()**

Read `crates/nimon-hub/src/server/mod.rs`. Initialize SQLite pool and pass to AlertManager:

```rust
let pool = SqlitePool::connect("./data/nimon.db")
    .await
    .expect("Failed to connect to database");

// Ensure schema exists
nimon_core::db::init_database(&pool).await.expect("Failed to init database");
```

Pass pool to AlertManager constructor.

**Step 4: Add REST endpoint for prediction history**

Add to the router:
```rust
.route("/api/predictions", get(get_predictions_handler))
```

Handler:
```rust
async fn get_predictions_handler(
    State(state): State<HubState>,
) -> impl IntoResponse {
    // Query from database via prediction repo
    Json(serde_json::json!({ "predictions": [], "total": 0 }))
}
```

**Step 5: Run tests**

Run: `cargo test --workspace`
Expected: All tests pass

**Step 6: Commit**

```bash
git add crates/nimon-hub/src/ crates/nimon-core/src/db/
git commit -m "feat(hub): persist alerts and predictions to SQLite database"
```

---

## Phase 4 Summary

After Phase 4:
1. ✅ Real NI-SysCfg device discovery and health polling (with simulated fallback)
2. ✅ NI-VISA FFI bindings (instrument discovery, *IDN? health check)
3. ✅ NI-DAQmx FFI bindings (device discovery, temperature, self-test, power supply voltages)
4. ✅ Database repositories for alerts, predictions, and action history
5. ✅ Auto-execute remediation actions on critical alerts
6. ✅ Persist alerts and predictions to SQLite

---

## Running Tests

```bash
cargo test --workspace
```

---

## Implementation Notes

- All NI API calls have graceful fallbacks to simulated data when NI drivers are not installed
- NI-VISA and NI-DAQmx follow the same safe wrapper pattern as NI-SysCfg (load DLL → get symbols → safe methods)
- Database persistence uses the existing SQLite schema from Phase 1
- Auto-actions are triggered only for Critical severity alerts to prevent false-positive remediation
- The action executor is optional - AlertManager works without it
