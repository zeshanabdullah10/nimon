//! Safe Rust wrapper for NI-SysCfg API
//!
//! This module provides safe, idiomatic Rust wrappers around the NI-SysCfg C API.

use libloading::os::windows::{Library, Symbol};
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::raw::c_int;
use std::path::Path;

use super::ffi::*;
use super::types::*;
use crate::common::check_status;
use crate::{NimonError, NimonResult};

/// NI System Configuration API wrapper
///
/// This struct holds the loaded DLL and function pointers for the NI-SysCfg API.
/// It provides a safe interface for discovering and querying NI hardware.
pub struct NiSysCfg {
    #[allow(dead_code)]
    library: Library,
    initialize_session: Symbol<NISysCfgInitializeSession>,
    close_handle: Symbol<NISysCfgCloseHandle>,
    find_hardware: Symbol<NISysCfgFindHardware>,
    next_resource: Symbol<NISysCfgNextResource>,
    get_property: Symbol<NISysCfgGetResourceProperty>,
}

impl NiSysCfg {
    /// Load the NI-SysCfg DLL and initialize function pointers
    ///
    /// # Errors
    /// Returns an error if:
    /// - The DLL cannot be found (NI software not installed)
    /// - The DLL cannot be loaded
    /// - Required symbols are missing from the DLL
    pub fn load() -> NimonResult<Self> {
        let dll_path = Self::find_dll()?;

        unsafe {
            let library = Library::new(&dll_path).map_err(|e| {
                NimonError::Connection(format!(
                    "Failed to load {:?}: {}",
                    dll_path, e
                ))
            })?;

            let initialize_session =
                Self::get_symbol(&library, b"NISysCfgInitializeSession")?;
            let close_handle = Self::get_symbol(&library, b"NISysCfgCloseHandle")?;
            let find_hardware = Self::get_symbol(&library, b"NISysCfgFindHardware")?;
            let next_resource = Self::get_symbol(&library, b"NISysCfgNextResource")?;
            let get_property =
                Self::get_symbol(&library, b"NISysCfgGetResourceProperty")?;

            Ok(Self {
                library,
                initialize_session,
                close_handle,
                find_hardware,
                next_resource,
                get_property,
            })
        }
    }

    unsafe fn get_symbol<T>(library: &Library, name: &[u8]) -> NimonResult<Symbol<T>> {
        library.get(name).map_err(|e| {
            NimonError::Connection(format!(
                "Symbol not found: {} ({}). The installed niSysCfg.dll may be an incompatible version.",
                String::from_utf8_lossy(name),
                e
            ))
        })
    }

    /// Find the NI-SysCfg DLL location
    ///
    /// Searches common installation paths for the NI-SysCfg DLL.
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
            "niSysCfg.dll not found. Please install NI System Configuration.".into()
        ))
    }

    /// Check if NI-SysCfg is available on this system
    ///
    /// Returns true if the DLL can be found, false otherwise.
    /// This is useful for conditional feature enabling.
    pub fn is_available() -> bool {
        Self::find_dll().is_ok()
    }

    /// Create a new NI-SysCfg session for the local system
    ///
    /// A session is required for most operations.
    /// The session will be automatically closed when dropped.
    pub fn create_session(&self) -> NimonResult<SysCfgSession<'_>> {
        let mut handle: *mut NiSysCfgSession = std::ptr::null_mut();

        unsafe {
            let status = (self.initialize_session)(
                std::ptr::null(),                      // target: NULL => localhost
                std::ptr::null(),                      // username: NULL => no credentials
                std::ptr::null(),                      // password: NULL => no credentials
                NISYSCFG_LOCALE_DEFAULT,
                NISYSCFG_BOOL_FALSE, // TRUE here crashes NextResource on NI 26.3
                10_000,              // connect timeout ms
                std::ptr::null_mut(), // expert enum handle (optional)
                &mut handle,
            );
            check_status("NISysCfgInitializeSession", status)?;
        }

        Ok(SysCfgSession {
            handle,
            api: self,
        })
    }

    fn close_handle_ptr(&self, handle: *mut c_void) {
        unsafe {
            (self.close_handle)(handle);
        }
    }

    /// Read a string property into a caller-provided buffer.
    ///
    /// String properties copy into a buffer of at least
    /// NISYSCFG_SIMPLE_STRING_LENGTH bytes; they do not return allocations.
    unsafe fn get_string_prop(
        &self,
        resource: *mut NiSysCfgResource,
        property_id: c_int,
    ) -> Option<String> {
        let mut buffer = [0 as std::os::raw::c_char; NISYSCFG_SIMPLE_STRING_LENGTH];
        let status = (self.get_property)(
            resource,
            property_id,
            buffer.as_mut_ptr() as *mut c_void,
        );
        if status != 0 {
            return None;
        }
        let s = crate::common::c_str_to_string(buffer.as_ptr());
        s.filter(|s| !s.is_empty())
    }

    /// Read an integer property (e.g., slot number, presence)
    unsafe fn get_int_prop(
        &self,
        resource: *mut NiSysCfgResource,
        property_id: c_int,
    ) -> Option<c_int> {
        let mut value: c_int = 0;
        let status = (self.get_property)(
            resource,
            property_id,
            &mut value as *mut _ as *mut c_void,
        );
        if status != 0 {
            None
        } else {
            Some(value)
        }
    }

    /// Read a floating-point property (e.g., temperature)
    unsafe fn get_f64_prop(
        &self,
        resource: *mut NiSysCfgResource,
        property_id: c_int,
    ) -> Option<f64> {
        let mut value: f64 = 0.0;
        let status = (self.get_property)(
            resource,
            property_id,
            &mut value as *mut _ as *mut c_void,
        );
        if status != 0 {
            None
        } else {
            Some(value)
        }
    }

    /// Read a boolean (NISysCfgBool) property
    unsafe fn get_bool_prop(
        &self,
        resource: *mut NiSysCfgResource,
        property_id: c_int,
    ) -> Option<bool> {
        let mut value: i32 = 0;
        let status = (self.get_property)(
            resource,
            property_id,
            &mut value as *mut _ as *mut c_void,
        );
        if status != 0 {
            None
        } else {
            Some(value != 0)
        }
    }
}

/// RAII wrapper for SysCfg session
///
/// This represents an active session with the NI-SysCfg API.
/// The session is automatically closed when this struct is dropped.
pub struct SysCfgSession<'a> {
    handle: *mut NiSysCfgSession,
    api: &'a NiSysCfg,
}

impl<'a> SysCfgSession<'a> {
    /// Discover all NI devices on the system
    ///
    /// Returns a list of all discovered devices with their properties.
    /// This uses filter-syntax search with an empty filter, which returns all
    /// locally available resources.
    pub fn discover_devices(&self) -> NimonResult<Vec<DiscoveredDevice>> {
        let mut devices = Vec::new();

        for resource in self.enumerate_all()? {
            let device = match unsafe { self.extract_device_info(resource) } {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(
                        "Skipping device: failed to extract device info: {}",
                        e
                    );
                    self.api.close_handle_ptr(resource as *mut c_void);
                    continue;
                }
            };
            self.api.close_handle_ptr(resource as *mut c_void);
            devices.push(device);
        }

        Ok(devices)
    }

    /// Get health information for a specific device
    ///
    /// Locates the resource whose product name (or formatted serial number)
    /// matches `device_name`, then queries its health properties.
    /// Returns DeviceHealth::unreachable() if the device cannot be found.
    pub fn get_device_health(&self, device_name: &str) -> NimonResult<DeviceHealth> {
        let mut result = DeviceHealth::unreachable();
        let mut found = false;

        for resource in self.enumerate_all()? {
            if !found {
                let product = unsafe {
                    self.api
                        .get_string_prop(resource, properties::PRODUCT_NAME)
                }
                .unwrap_or_default();
                let serial = unsafe {
                    self.api
                        .get_string_prop(resource, properties::SERIAL_NUMBER)
                }
                .unwrap_or_default();

                if product == device_name || serial == device_name {
                    result = unsafe { self.query_resource_health(resource) };
                    found = true;
                }
            }

            self.api.close_handle_ptr(resource as *mut c_void);
        }

        if !found {
            tracing::debug!(
                "Device '{}' not found via NI-SysCfg, treating as unreachable",
                device_name
            );
        }
        Ok(result)
    }

    /// Enumerate all resources, returning raw resource handles.
    ///
    /// Each handle is opened with NextResource; callers must close every
    /// returned handle via `close_handle_ptr` (handles are NOT closed here
    /// beyond the enumeration itself).
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
                let status = (self.api.next_resource)(
                    self.handle,
                    enum_handle,
                    &mut resource,
                );

                if status != 0 || resource.is_null() {
                    break; // No more resources
                }

                resources.push(resource);
            }

            // Clean up enumeration handle
            self.api.close_handle_ptr(enum_handle as *mut c_void);
        }

        Ok(resources)
    }

    /// Query health properties from a resource handle
    ///
    /// Reads IS_PRESENT and CURRENT_TEMP properties and builds a DeviceHealth.
    unsafe fn query_resource_health(
        &self,
        resource: *mut NiSysCfgResource,
    ) -> DeviceHealth {
        let is_reachable = self
            .api
            .get_int_prop(resource, properties::IS_PRESENT)
            .map(|present| present == NISYSCFG_IS_PRESENT_TYPE_PRESENT)
            .unwrap_or(false);
        let temperature = self.api.get_f64_prop(resource, properties::CURRENT_TEMP);
        let metrics = HashMap::new();

        if !is_reachable {
            return DeviceHealth {
                is_reachable: false,
                temperature: None,
                self_test_passed: None,
                error_message: Some("Device not reachable".to_string()),
                metrics,
            };
        }

        DeviceHealth {
            is_reachable: true,
            temperature,
            self_test_passed: None,
            error_message: None,
            metrics,
        }
    }

    unsafe fn extract_device_info(
        &self,
        resource: *mut NiSysCfgResource,
    ) -> NimonResult<DiscoveredDevice> {
        let api = self.api;

        // Product name
        let product_name = api
            .get_string_prop(resource, properties::PRODUCT_NAME)
            .ok_or_else(|| {
                NimonError::Connection(
                    "Failed to read PRODUCT_NAME from resource".to_string(),
                )
            })?;

        // Serial number (string property in current NI versions)
        let serial_number = api
            .get_string_prop(resource, properties::SERIAL_NUMBER)
            .unwrap_or_default();

        let ip_address = api.get_string_prop(resource, properties::TCP_IP_ADDRESS);
        let firmware_version =
            api.get_string_prop(resource, properties::FIRMWARE_REVISION);
        let is_reachable = api
            .get_int_prop(resource, properties::IS_PRESENT)
            .map(|present| present == NISYSCFG_IS_PRESENT_TYPE_PRESENT)
            .unwrap_or(false);
        let temperature = api.get_f64_prop(resource, properties::CURRENT_TEMP);

        Ok(DiscoveredDevice {
            product_name,
            serial_number,
            ip_address,
            is_reachable,
            temperature,
            firmware_version,
            // Not exposed by this NI-SysCfg version's resource properties
            driver_version: None,
        })
    }
}

impl<'a> Drop for SysCfgSession<'a> {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            self.api.close_handle_ptr(self.handle as *mut _);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available() {
        // This will return false on systems without NI software installed
        // The test just verifies the function doesn't panic
        let _ = NiSysCfg::is_available();
    }

    #[test]
    fn test_discovered_device_creation() {
        let device = DiscoveredDevice::new(
            "PXIe-8880".to_string(),
            "12345678".to_string(),
        );
        assert_eq!(device.product_name, "PXIe-8880");
        assert_eq!(device.serial_number, "12345678");
        assert!(device.ip_address.is_none());
    }

    #[test]
    fn test_device_health_unreachable_creation() {
        let health = DeviceHealth::unreachable();
        assert!(!health.is_reachable);
        assert!(health.error_message.is_some());
    }

    #[test]
    fn test_device_health_to_status_healthy() {
        let health = DeviceHealth {
            is_reachable: true,
            temperature: Some(42.0),
            self_test_passed: None,
            error_message: None,
            metrics: HashMap::new(),
        };
        let (status, metrics) = health.to_status_and_metrics();
        assert!(matches!(status, nimon_core::HealthStatus::Healthy));
        assert!(metrics.contains_key("temperature"));
        assert!(metrics.contains_key("is_reachable"));
    }

    #[test]
    fn test_device_health_to_status_offline() {
        let health = DeviceHealth::unreachable();
        let (status, _) = health.to_status_and_metrics();
        assert!(matches!(status, nimon_core::HealthStatus::Offline));
    }
}
