//! Safe Rust wrapper for NI-SysCfg API
//!
//! This module provides safe, idiomatic Rust wrappers around the NI-SysCfg C API.

use libloading::os::windows::{Library, Symbol};
use std::collections::HashMap;
use std::ffi::c_void;
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
    initialize: Symbol<NiSysCfgInitialize>,
    close_handle: Symbol<NiSysCfgCloseHandle>,
    find_hardware: Symbol<NiSysCfgFindHardware>,
    next_resource: Symbol<NiSysCfgNextResource>,
    get_property: Symbol<NiSysCfgGetResourceProperty>,
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

            let initialize = Self::get_symbol(&library, b"NiSysCfg_Initialize")?;
            let close_handle = Self::get_symbol(&library, b"NiSysCfg_CloseHandle")?;
            let find_hardware = Self::get_symbol(&library, b"NiSysCfg_FindHardware")?;
            let next_resource = Self::get_symbol(&library, b"NiSysCfg_NextResource")?;
            let get_property = Self::get_symbol(&library, b"NiSysCfg_GetResourceProperty")?;

            Ok(Self {
                library,
                initialize,
                close_handle,
                find_hardware,
                next_resource,
                get_property,
            })
        }
    }

    unsafe fn get_symbol<T>(library: &Library, name: &[u8]) -> NimonResult<Symbol<T>> {
        library
            .get(name)
            .map_err(|e| NimonError::Connection(format!("Symbol not found: {}", e)))
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

    /// Create a new NI-SysCfg session
    ///
    /// A session is required for most operations.
    /// The session will be automatically closed when dropped.
    pub fn create_session(&self) -> NimonResult<SysCfgSession<'_>> {
        let mut handle: *mut NiSysCfgSession = std::ptr::null_mut();

        unsafe {
            let status = (self.initialize)(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                &mut handle,
            );
            check_status("NiSysCfg_Initialize", status)?;
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
    /// This uses simple search mode which finds all locally connected devices.
    pub fn discover_devices(&self) -> NimonResult<Vec<DiscoveredDevice>> {
        let mut enum_handle: *mut NiSysCfgEnum = std::ptr::null_mut();
        let mut devices = Vec::new();

        unsafe {
            let status = (self.api.find_hardware)(
                self.handle,
                NISYSCFG_SIMPLE_SEARCH,
                std::ptr::null(),
                &mut enum_handle,
            );
            check_status("NiSysCfg_FindHardware", status)?;

            loop {
                let mut resource: *mut NiSysCfgResource = std::ptr::null_mut();
                let status = (self.api.next_resource)(
                    self.handle,
                    enum_handle,
                    &mut resource,
                );

                if status != 0 {
                    break; // No more resources
                }

                let device = match self.extract_device_info(resource) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(
                            "Skipping device: failed to extract device info: {}",
                            e
                        );
                        let _ = (self.api.close_handle)(
                            resource as *mut std::ffi::c_void,
                        );
                        continue;
                    }
                };
                // Close the resource handle to prevent leaks
                let _ = (self.api.close_handle)(
                    resource as *mut std::ffi::c_void,
                );
                devices.push(device);
            }

            // Clean up enumeration handle
            self.api.close_handle_ptr(enum_handle as *mut _);
        }

        Ok(devices)
    }

    /// Get health information for a specific device
    ///
    /// Uses FindHardware with a filter for the specific device name to locate
    /// the resource, then queries IS_REACHABLE and TEMPERATURE properties.
    /// Returns DeviceHealth::unreachable() if the device cannot be found.
    pub fn get_device_health(&self, device_id: &str) -> NimonResult<DeviceHealth> {
        // Build a filter that matches the device name (product name or serial)
        let filter = crate::common::string_to_c_string(device_id)
            .ok_or_else(|| NimonError::Config("Device ID contains null bytes".into()))?;

        let mut enum_handle: *mut NiSysCfgEnum = std::ptr::null_mut();

        unsafe {
            let status = (self.api.find_hardware)(
                self.handle,
                NISYSCFG_SIMPLE_SEARCH,
                filter.as_ptr(),
                &mut enum_handle,
            );

            if status != 0 {
                // Device not found or error - return unreachable
                tracing::debug!(
                    "FindHardware for '{}' returned status {}, treating as unreachable",
                    device_id,
                    status
                );
                return Ok(DeviceHealth::unreachable());
            }

            let mut resource: *mut NiSysCfgResource = std::ptr::null_mut();
            let next_status = (self.api.next_resource)(
                self.handle,
                enum_handle,
                &mut resource,
            );

            // Clean up enum handle regardless of outcome
            self.api.close_handle_ptr(enum_handle as *mut _);

            if next_status != 0 || resource.is_null() {
                // No matching resource found
                return Ok(DeviceHealth::unreachable());
            }

            let health = self.query_resource_health(resource)?;

            // Clean up resource handle
            self.api.close_handle_ptr(resource as *mut _);

            Ok(health)
        }
    }

    /// Query health properties from a resource handle
    ///
    /// Reads IS_REACHABLE and TEMPERATURE properties and builds a DeviceHealth.
    unsafe fn query_resource_health(
        &self,
        resource: *mut NiSysCfgResource,
    ) -> NimonResult<DeviceHealth> {
        let mut int_val: i32 = 0;
        let mut float_val: f64 = 0.0;

        // Query IS_REACHABLE
        (self.api.get_property)(
            resource,
            properties::IS_REACHABLE,
            &mut int_val as *mut _ as *mut _,
        );
        let is_reachable = int_val != 0;

        // Query TEMPERATURE
        let temp_status = (self.api.get_property)(
            resource,
            properties::TEMPERATURE,
            &mut float_val as *mut _ as *mut _,
        );
        let temperature = if temp_status == 0 { Some(float_val) } else { None };

        let metrics = HashMap::new();

        // If device is not reachable, return early
        if !is_reachable {
            return Ok(DeviceHealth {
                is_reachable: false,
                temperature: None,
                self_test_passed: None,
                error_message: Some("Device not reachable".to_string()),
                metrics,
            });
        }

        Ok(DeviceHealth {
            is_reachable: true,
            temperature,
            self_test_passed: None,
            error_message: None,
            metrics,
        })
    }

    unsafe fn extract_device_info(
        &self,
        resource: *mut NiSysCfgResource,
    ) -> NimonResult<DiscoveredDevice> {
        let mut buffer = [0i8; 512];
        let mut int_val: i32 = 0;
        let mut float_val: f64 = 0.0;

        // Product name
        let name_status = (self.api.get_property)(
            resource,
            properties::PRODUCT_NAME,
            buffer.as_mut_ptr() as *mut _,
        );
        if name_status != 0 {
            return Err(NimonError::Connection(format!(
                "Failed to read PRODUCT_NAME from resource (status {})",
                name_status
            )));
        }
        let product_name = crate::common::c_str_to_string(buffer.as_ptr())
            .unwrap_or_default();

        // Serial number
        buffer.fill(0);
        let serial_status = (self.api.get_property)(
            resource,
            properties::SERIAL_NUMBER,
            buffer.as_mut_ptr() as *mut _,
        );
        if serial_status != 0 {
            tracing::warn!(
                "Failed to read SERIAL_NUMBER from resource '{}' (status {})",
                product_name,
                serial_status
            );
        }
        let serial_number = crate::common::c_str_to_string(buffer.as_ptr())
            .unwrap_or_default();

        // IP address
        buffer.fill(0);
        (self.api.get_property)(
            resource,
            properties::IPADDRESS,
            buffer.as_mut_ptr() as *mut _,
        );
        let ip_address = crate::common::c_str_to_string(buffer.as_ptr());

        // Is reachable
        (self.api.get_property)(
            resource,
            properties::IS_REACHABLE,
            &mut int_val as *mut _ as *mut _,
        );
        let is_reachable = int_val != 0;

        // Temperature
        let temp_status = (self.api.get_property)(
            resource,
            properties::TEMPERATURE,
            &mut float_val as *mut _ as *mut _,
        );
        let temperature = if temp_status == 0 { Some(float_val) } else { None };

        // Firmware version
        buffer.fill(0);
        (self.api.get_property)(
            resource,
            properties::FIRMWARE_REVISION,
            buffer.as_mut_ptr() as *mut _,
        );
        let firmware_version = crate::common::c_str_to_string(buffer.as_ptr());

        // Driver version
        buffer.fill(0);
        (self.api.get_property)(
            resource,
            properties::DRIVER_VERSION,
            buffer.as_mut_ptr() as *mut _,
        );
        let driver_version = crate::common::c_str_to_string(buffer.as_ptr());

        Ok(DiscoveredDevice {
            product_name,
            serial_number,
            ip_address,
            is_reachable,
            temperature,
            firmware_version,
            driver_version,
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
