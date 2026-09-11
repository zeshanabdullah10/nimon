//! Safe Rust wrapper for NI-SysCfg API
//!
//! This module provides safe, idiomatic Rust wrappers around the NI-SysCfg C API.
//!
//! The DLL is loaded exactly once per process and the resolved function
//! pointers are cached in a global â€” repeated `NiSysCfg::load()` calls are
//! cheap (this is on the hot polling path; LoadLibrary churn per poll is
//! both slow and keeps the loader lock busy).

use libloading::os::windows::{Library, Symbol};
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::raw::c_int;
use std::path::Path;
use std::sync::OnceLock;

use super::ffi::*;
use super::types::*;
use crate::common::check_status;
use crate::{NimonError, NimonResult};
/// Resolved NI-SysCfg entry points (plain fn pointers, Copy)
#[derive(Clone, Copy)]
struct SysCfgApi {
    initialize_session: NISysCfgInitializeSession,
    close_handle: NISysCfgCloseHandle,
    find_hardware: NISysCfgFindHardware,
    next_resource: NISysCfgNextResource,
    get_property: NISysCfgGetResourceProperty,
    get_indexed_property: NISysCfgGetResourceIndexedProperty,
    get_system_property: NISysCfgGetSystemProperty,
}

struct SysCfgLoaded {
    /// keeps the DLL alive for the process lifetime
    _library: Library,
    api: SysCfgApi,
}

static SYSCFG: OnceLock<Result<SysCfgLoaded, String>> = OnceLock::new();

fn syscfg() -> &'static Result<SysCfgLoaded, String> {
    SYSCFG.get_or_init(|| unsafe { load_once() })
}

unsafe fn load_once() -> Result<SysCfgLoaded, String> {
    let dll_path = NiSysCfg::find_dll().map_err(|e| e.to_string())?;
    let library =
        Library::new(&dll_path).map_err(|e| format!("Failed to load {dll_path:?}: {e}"))?;

    let resolve = |name: &[u8]| -> Result<usize, String> {
        let sym: Symbol<usize> = library
            .get(name)
            .map_err(|e| {
                format!(
                    "Symbol not found: {} ({e}). The installed niSysCfg.dll may be an incompatible version.",
                    String::from_utf8_lossy(name)
                )
            })?;
        Ok(*sym)
    };

    // resolve every entry point before moving the Library
    let f_init: NISysCfgInitializeSession =
        std::mem::transmute(resolve(b"NISysCfgInitializeSession")?);
    let f_close: NISysCfgCloseHandle = std::mem::transmute(resolve(b"NISysCfgCloseHandle")?);
    let f_find: NISysCfgFindHardware = std::mem::transmute(resolve(b"NISysCfgFindHardware")?);
    let f_next: NISysCfgNextResource = std::mem::transmute(resolve(b"NISysCfgNextResource")?);
    let f_prop: NISysCfgGetResourceProperty =
        std::mem::transmute(resolve(b"NISysCfgGetResourceProperty")?);
    let f_idx: NISysCfgGetResourceIndexedProperty =
        std::mem::transmute(resolve(b"NISysCfgGetResourceIndexedProperty")?);
    let f_sys: NISysCfgGetSystemProperty =
        std::mem::transmute(resolve(b"NISysCfgGetSystemProperty")?);
    let _ = resolve;

    Ok(SysCfgLoaded {
        _library: library,
        api: SysCfgApi {
            initialize_session: f_init,
            close_handle: f_close,
            find_hardware: f_find,
            next_resource: f_next,
            get_property: f_prop,
            get_indexed_property: f_idx,
            get_system_property: f_sys,
        },
    })
}

/// NI System Configuration API wrapper
///
/// Cheap to obtain: the DLL and entry points are cached process-wide.
#[derive(Clone, Copy)]
pub struct NiSysCfg {
    api: SysCfgApi,
}

impl NiSysCfg {
    /// Load the NI-SysCfg API (cached after the first successful call)
    ///
    /// # Errors
    /// Returns an error if:
    /// - The DLL cannot be found (NI software not installed)
    /// - The DLL cannot be loaded
    /// - Required symbols are missing from the DLL
    pub fn load() -> NimonResult<Self> {
        match syscfg() {
            Ok(loaded) => Ok(NiSysCfg { api: loaded.api }),
            Err(e) => Err(NimonError::Connection(e.clone())),
        }
    }

    /// Find the NI-SysCfg DLL location
    fn find_dll() -> NimonResult<std::path::PathBuf> {
        let candidates = [
            "C:\\Windows\\System32\\niSysCfg.dll",
            "C:\\Program Files\\National Instruments\\Shared\\niSysCfg.dll",
            "C:\\Program Files (x86)\\National Instruments\\Shared\\niSysCfg.dll",
        ];

        for path in &candidates {
            if Path::new(path).exists() {
                return Ok(path.into());
            }
        }

        Err(NimonError::Connection(
            "niSysCfg.dll not found. Please install NI System Configuration.".into(),
        ))
    }

    // (kept public-surface methods below)

    /// Check if NI-SysCfg is available on this system
    ///
    /// Returns true if the DLL can be found, false otherwise.
    /// This is useful for conditional feature enabling.
    pub fn is_available() -> bool {
        syscfg().is_ok()
    }

    /// Create a new NI-SysCfg session for the local system
    ///
    /// A session is required for most operations.
    /// The session will be automatically closed when dropped.
    pub fn create_session(&self) -> NimonResult<SysCfgSession> {
        let mut handle: *mut NiSysCfgSession = std::ptr::null_mut();

        unsafe {
            let status = (self.api.initialize_session)(
                std::ptr::null(), // target: NULL => localhost
                std::ptr::null(), // username: NULL => no credentials
                std::ptr::null(), // password: NULL => no credentials
                NISYSCFG_LOCALE_DEFAULT,
                NISYSCFG_BOOL_FALSE,  // TRUE here crashes NextResource on NI 26.3
                10_000,               // connect timeout ms
                std::ptr::null_mut(), // expert enum handle (optional)
                &mut handle,
            );
            check_status("NISysCfgInitializeSession", status)?;
        }

        Ok(SysCfgSession {
            handle,
            api: self.api,
        })
    }

    unsafe fn get_string_prop(
        api: SysCfgApi,
        resource: *mut NiSysCfgResource,
        property_id: c_int,
    ) -> Option<String> {
        let mut buffer = [0 as std::os::raw::c_char; NISYSCFG_SIMPLE_STRING_LENGTH];
        let status = (api.get_property)(resource, property_id, buffer.as_mut_ptr() as *mut c_void);
        if status != 0 {
            return None;
        }
        crate::common::c_str_to_string(buffer.as_ptr()).filter(|s| !s.is_empty())
    }

    unsafe fn get_int_prop(
        api: SysCfgApi,
        resource: *mut NiSysCfgResource,
        property_id: c_int,
    ) -> Option<c_int> {
        let mut value: c_int = 0;
        let status = (api.get_property)(resource, property_id, &mut value as *mut _ as *mut c_void);
        if status != 0 {
            None
        } else {
            Some(value)
        }
    }

    unsafe fn get_f64_prop(
        api: SysCfgApi,
        resource: *mut NiSysCfgResource,
        property_id: c_int,
    ) -> Option<f64> {
        let mut value: f64 = 0.0;
        let status = (api.get_property)(resource, property_id, &mut value as *mut _ as *mut c_void);
        if status != 0 {
            None
        } else {
            Some(value)
        }
    }

    /// Read an indexed string property (buffer convention, same as plain)
    unsafe fn get_indexed_string_prop(
        api: SysCfgApi,
        resource: *mut NiSysCfgResource,
        property_id: c_int,
        index: u32,
    ) -> Option<String> {
        let mut buffer = [0 as std::os::raw::c_char; NISYSCFG_SIMPLE_STRING_LENGTH];
        let status = (api.get_indexed_property)(
            resource,
            property_id,
            index,
            buffer.as_mut_ptr() as *mut c_void,
        );
        if status != 0 {
            return None;
        }
        crate::common::c_str_to_string(buffer.as_ptr()).filter(|s| !s.is_empty())
    }

    /// Read an indexed double property
    unsafe fn get_indexed_f64_prop(
        api: SysCfgApi,
        resource: *mut NiSysCfgResource,
        property_id: c_int,
        index: u32,
    ) -> Option<f64> {
        let mut value: f64 = 0.0;
        let status = (api.get_indexed_property)(
            resource,
            property_id,
            index,
            &mut value as *mut _ as *mut c_void,
        );
        if status != 0 {
            None
        } else {
            Some(value)
        }
    }

    /// Read a system string property (session-scoped, buffer convention)
    unsafe fn get_system_string_prop(
        api: SysCfgApi,
        session: *mut NiSysCfgSession,
        property_id: c_int,
    ) -> Option<String> {
        let mut buffer = [0 as std::os::raw::c_char; NISYSCFG_SIMPLE_STRING_LENGTH];
        let status =
            (api.get_system_property)(session, property_id, buffer.as_mut_ptr() as *mut c_void);
        if status != 0 {
            return None;
        }
        crate::common::c_str_to_string(buffer.as_ptr()).filter(|s| !s.is_empty())
    }

    /// Read a system double property (returned in KB per nisyscfg.h)
    unsafe fn get_system_f64_prop(
        api: SysCfgApi,
        session: *mut NiSysCfgSession,
        property_id: c_int,
    ) -> Option<f64> {
        let mut value: f64 = 0.0;
        let status =
            (api.get_system_property)(session, property_id, &mut value as *mut _ as *mut c_void);
        if status != 0 {
            None
        } else {
            Some(value)
        }
    }
}

/// RAII wrapper for SysCfg session
pub struct SysCfgSession {
    handle: *mut NiSysCfgSession,
    api: SysCfgApi,
}

impl SysCfgSession {
    /// Discover all NI devices on the system
    pub fn discover_devices(&self) -> NimonResult<Vec<DiscoveredDevice>> {
        let mut devices = Vec::new();

        for resource in self.enumerate_all()? {
            let device = match unsafe { self.extract_device_info(resource) } {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("Skipping device: failed to extract device info: {e}");
                    unsafe { (self.api.close_handle)(resource as *mut c_void) };
                    continue;
                }
            };
            unsafe { (self.api.close_handle)(resource as *mut c_void) };
            devices.push(device);
        }

        Ok(devices)
    }

    /// Single-pass sweep: discover every device and its health with ONE
    /// enumeration of the hardware. Far cheaper than per-device health
    /// queries, each of which enumerates all resources again.
    pub fn discover_with_health(&self) -> NimonResult<Vec<(DiscoveredDevice, DeviceHealth)>> {
        let mut results = Vec::new();

        for resource in self.enumerate_all()? {
            let item = match unsafe { self.extract_device_info(resource) } {
                Ok(d) => {
                    let health = unsafe { self.query_resource_health(resource) };
                    Some((d, health))
                }
                Err(e) => {
                    tracing::warn!("Skipping device: failed to extract device info: {e}");
                    None
                }
            };
            unsafe { (self.api.close_handle)(resource as *mut c_void) };
            if let Some(item) = item {
                results.push(item);
            }
        }

        Ok(results)
    }

    /// Get health information for a specific device
    ///
    /// Locates the resource whose product name (or serial number)
    /// matches `device_name`, then queries its health properties.
    /// Returns DeviceHealth::unreachable() if the device cannot be found.
    pub fn get_device_health(&self, device_name: &str) -> NimonResult<DeviceHealth> {
        let mut result = DeviceHealth::unreachable();
        let mut found = false;

        for resource in self.enumerate_all()? {
            if !found {
                let product = unsafe {
                    NiSysCfg::get_string_prop(self.api, resource, properties::PRODUCT_NAME)
                }
                .unwrap_or_default();
                let serial = unsafe {
                    NiSysCfg::get_string_prop(self.api, resource, properties::SERIAL_NUMBER)
                }
                .unwrap_or_default();

                if product == device_name || serial == device_name {
                    result = unsafe { self.query_resource_health(resource) };
                    found = true;
                }
            }

            unsafe { (self.api.close_handle)(resource as *mut c_void) };
        }

        if !found {
            tracing::debug!(
                "Device '{device_name}' not found via NI-SysCfg, treating as unreachable"
            );
        }
        Ok(result)
    }

    /// Enumerate all resources, returning raw resource handles.
    fn enumerate_all(&self) -> NimonResult<Vec<*mut NiSysCfgResource>> {
        let mut enum_handle: *mut NiSysCfgEnum = std::ptr::null_mut();
        let mut resources = Vec::new();

        unsafe {
            let status = (self.api.find_hardware)(
                self.handle,
                NISYSCFG_FILTER_MODE_MATCH_VALUES_ALL, // ignored: filter is NULL
                std::ptr::null_mut(),                  // filter: NULL => all resources
                std::ptr::null(),                      // expert names: NULL => all experts
                &mut enum_handle,
            );
            check_status("NISysCfgFindHardware", status)?;

            loop {
                let mut resource: *mut NiSysCfgResource = std::ptr::null_mut();
                let status = (self.api.next_resource)(self.handle, enum_handle, &mut resource);

                // A NULL resource handle is the only reliable
                // end-of-enumeration signal. Treating every non-zero status
                // as termination truncated the list whenever a benign
                // warning status arrived mid-enumeration.
                if resource.is_null() {
                    break;
                }
                if status != 0 {
                    // Error with a valid handle: release the enumeration
                    // handle and any resources already collected, then
                    // surface the failure.
                    for collected in &resources {
                        (self.api.close_handle)(*collected as *mut c_void);
                    }
                    (self.api.close_handle)(enum_handle as *mut c_void);
                    check_status("NISysCfgNextResource", status)?;
                }

                resources.push(resource);
            }

            // Clean up enumeration handle
            (self.api.close_handle)(enum_handle as *mut c_void);
        }

        Ok(resources)
    }

    /// Query health properties from a resource handle
    unsafe fn query_resource_health(&self, resource: *mut NiSysCfgResource) -> DeviceHealth {
        let is_reachable = NiSysCfg::get_int_prop(self.api, resource, properties::IS_PRESENT)
            .map(|present| present == NISYSCFG_IS_PRESENT_TYPE_PRESENT)
            .unwrap_or(false);
        let mut temperature = NiSysCfg::get_f64_prop(self.api, resource, properties::CURRENT_TEMP);

        // Named temperature sensors (count from the resource property,
        // then name/reading/threshold per index)
        let mut sensors = Vec::new();
        let count = NiSysCfg::get_int_prop(self.api, resource, properties::NUMBER_OF_TEMP_SENSORS)
            .unwrap_or(0);
        for index in 0..count.max(0) as u32 {
            let name = NiSysCfg::get_indexed_string_prop(
                self.api,
                resource,
                indexed_properties::TEMPERATURE_NAME,
                index,
            );
            let reading = NiSysCfg::get_indexed_f64_prop(
                self.api,
                resource,
                indexed_properties::TEMPERATURE_READING,
                index,
            );
            let upper = NiSysCfg::get_indexed_f64_prop(
                self.api,
                resource,
                indexed_properties::TEMPERATURE_UPPER_CRITICAL,
                index,
            );
            if let (Some(name), Some(reading)) = (name, reading) {
                sensors.push(SensorReading {
                    name,
                    reading,
                    upper_critical: upper.filter(|v| v.is_finite()),
                });
            }
        }

        // Devices without a plain CURRENT_TEMP still get an aggregate
        // temperature from their first sensor
        if temperature.is_none() {
            temperature = sensors
                .iter()
                .map(|s| s.reading)
                .fold(None::<f64>, |acc, r| {
                    Some(match acc {
                        Some(a) if a >= r => a,
                        _ => r,
                    })
                });
        }

        let metrics = HashMap::new();

        if !is_reachable {
            return DeviceHealth {
                is_reachable: false,
                temperature: None,
                sensors: Vec::new(),
                self_test_passed: None,
                error_message: Some("Device not reachable".to_string()),
                metrics,
            };
        }

        DeviceHealth {
            is_reachable: true,
            temperature,
            sensors,
            self_test_passed: None,
            error_message: None,
            metrics,
        }
    }

    unsafe fn extract_device_info(
        &self,
        resource: *mut NiSysCfgResource,
    ) -> NimonResult<DiscoveredDevice> {
        // Product name
        let product_name = NiSysCfg::get_string_prop(self.api, resource, properties::PRODUCT_NAME)
            .ok_or_else(|| {
                NimonError::Connection("Failed to read PRODUCT_NAME from resource".to_string())
            })?;

        // Serial number (string property in current NI versions)
        let serial_number =
            NiSysCfg::get_string_prop(self.api, resource, properties::SERIAL_NUMBER)
                .unwrap_or_default();

        // NI MAX device name: the first expert's user alias (DAQmx name)
        let alias = NiSysCfg::get_indexed_string_prop(
            self.api,
            resource,
            indexed_properties::EXPERT_USER_ALIAS,
            0,
        );

        let slot =
            NiSysCfg::get_int_prop(self.api, resource, properties::SLOT_NUMBER).filter(|s| *s >= 0);
        let parent_link =
            NiSysCfg::get_string_prop(self.api, resource, properties::CONNECTS_TO_LINK_NAME);
        let num_slots = NiSysCfg::get_int_prop(self.api, resource, properties::NUMBER_OF_SLOTS)
            .filter(|s| *s >= 0);
        let is_simulated = NiSysCfg::get_int_prop(self.api, resource, properties::IS_SIMULATED)
            .map(|v| v != 0)
            .unwrap_or(false);

        let ip_address = NiSysCfg::get_string_prop(self.api, resource, properties::TCP_IP_ADDRESS);
        let firmware_version =
            NiSysCfg::get_string_prop(self.api, resource, properties::FIRMWARE_REVISION);
        let is_reachable = NiSysCfg::get_int_prop(self.api, resource, properties::IS_PRESENT)
            .map(|present| present == NISYSCFG_IS_PRESENT_TYPE_PRESENT)
            .unwrap_or(false);
        let temperature = NiSysCfg::get_f64_prop(self.api, resource, properties::CURRENT_TEMP);

        Ok(DiscoveredDevice {
            product_name,
            serial_number,
            alias,
            slot,
            parent_link,
            num_slots,
            is_simulated,
            ip_address,
            is_reachable,
            temperature,
            firmware_version,
            // Not exposed by this NI-SysCfg version's resource properties
            driver_version: None,
        })
    }

    /// Station-level system information (hostname, OS, memory, disk).
    /// Cheap: plain session property reads, no extra enumeration.
    pub fn get_system_info(&self) -> SystemInfo {
        unsafe {
            SystemInfo {
                hostname: NiSysCfg::get_system_string_prop(
                    self.api,
                    self.handle,
                    system_properties::HOSTNAME,
                ),
                product: NiSysCfg::get_system_string_prop(
                    self.api,
                    self.handle,
                    system_properties::PRODUCT_NAME,
                ),
                operating_system: NiSysCfg::get_system_string_prop(
                    self.api,
                    self.handle,
                    system_properties::OPERATING_SYSTEM,
                ),
                os_version: NiSysCfg::get_system_string_prop(
                    self.api,
                    self.handle,
                    system_properties::OS_VERSION,
                ),
                serial_number: NiSysCfg::get_system_string_prop(
                    self.api,
                    self.handle,
                    system_properties::SERIAL_NUMBER,
                ),
                memory_total_mb: NiSysCfg::get_system_f64_prop(
                    self.api,
                    self.handle,
                    system_properties::MEMORY_PHYS_TOTAL,
                )
                .map(|kb| kb / 1024.0),
                memory_free_mb: NiSysCfg::get_system_f64_prop(
                    self.api,
                    self.handle,
                    system_properties::MEMORY_PHYS_FREE,
                )
                .map(|kb| kb / 1024.0),
                disk_total_mb: NiSysCfg::get_system_f64_prop(
                    self.api,
                    self.handle,
                    system_properties::PRIMARY_DISK_TOTAL,
                )
                .map(|kb| kb / 1024.0),
                disk_free_mb: NiSysCfg::get_system_f64_prop(
                    self.api,
                    self.handle,
                    system_properties::PRIMARY_DISK_FREE,
                )
                .map(|kb| kb / 1024.0),
            }
        }
    }
}

impl Drop for SysCfgSession {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { (self.api.close_handle)(self.handle as *mut _) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available() {
        // Returns true on machines with NI software; must not panic either way
        let _ = NiSysCfg::is_available();
    }

    #[test]
    fn test_discovered_device_creation() {
        let device = DiscoveredDevice::new("PXIe-8880".to_string(), "12345678".to_string());
        assert_eq!(device.product_name, "PXIe-8880");
        assert_eq!(device.serial_number, "12345678");
    }

    #[test]
    fn test_device_health_unreachable() {
        let health = DeviceHealth::unreachable();
        assert!(!health.is_reachable);
        assert!(health.error_message.is_some());
    }

    #[test]
    fn test_device_health_to_status_healthy() {
        let health = DeviceHealth {
            is_reachable: true,
            temperature: Some(42.0),
            sensors: Vec::new(),
            self_test_passed: None,
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, metrics) = health.to_status_and_metrics();
        assert!(matches!(status, nimon_core::HealthStatus::Healthy));
        assert!(metrics.contains_key("temperature"));
    }
}
