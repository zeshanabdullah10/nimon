//! Safe Rust wrapper for NI-DAQmx API
//!
//! This module provides safe, idiomatic Rust wrappers around the NI-DAQmx C API.
//! Unlike VISA, DAQmx does not require session handles for attribute-query
//! functions -- just pass device name strings directly.

use libloading::os::windows::{Library, Symbol};
use std::path::Path;

use super::ffi::*;
use super::types::*;
use crate::common::{c_str_to_string, check_status, string_to_c_string};
use crate::{NimonError, NimonResult};

/// NI-DAQmx API wrapper
///
/// This struct holds the loaded DLL and function pointers for the NI-DAQmx API.
/// It provides a safe interface for discovering DAQ devices and querying
/// their health information.
pub struct NiDaqMx {
    #[allow(dead_code)]
    library: Library,
    get_sys_dev_names: Symbol<DAQmxGetSysDevNames>,
    get_dev_product_type_name: Symbol<DAQmxGetDevProductTypeName>,
    get_dev_serial_num: Symbol<DAQmxGetDevSerialNum>,
    get_dev_temperature: Symbol<DAQmxGetDevTemperature>,
    get_dev_self_test_result: Symbol<DAQmxGetDevSelfTestResult>,
    get_dev_ai_power_supply_voltages: Symbol<DAQmxGetDevAIPowerSupplyVoltages>,
    reset_device: Symbol<DAQmxResetDevice>,
    get_dev_product_number: Symbol<DAQmxGetDevProductNumber>,
}

impl NiDaqMx {
    /// Load the NI-DAQmx DLL and initialize function pointers
    ///
    /// # Errors
    /// Returns an error if:
    /// - The DLL cannot be found (NI-DAQmx not installed)
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

            let get_sys_dev_names = Self::get_symbol(&library, b"DAQmxGetSysDevNames")?;
            let get_dev_product_type_name =
                Self::get_symbol(&library, b"DAQmxGetDevProductTypeName")?;
            let get_dev_serial_num = Self::get_symbol(&library, b"DAQmxGetDevSerialNum")?;
            let get_dev_temperature = Self::get_symbol(&library, b"DAQmxGetDevTemperature")?;
            let get_dev_self_test_result =
                Self::get_symbol(&library, b"DAQmxGetDevSelfTestResult")?;
            let get_dev_ai_power_supply_voltages =
                Self::get_symbol(&library, b"DAQmxGetDevAIPowerSupplyVoltages")?;
            let reset_device = Self::get_symbol(&library, b"DAQmxResetDevice")?;
            let get_dev_product_number =
                Self::get_symbol(&library, b"DAQmxGetDevProductNumber")?;

            Ok(Self {
                library,
                get_sys_dev_names,
                get_dev_product_type_name,
                get_dev_serial_num,
                get_dev_temperature,
                get_dev_self_test_result,
                get_dev_ai_power_supply_voltages,
                reset_device,
                get_dev_product_number,
            })
        }
    }

    unsafe fn get_symbol<T>(library: &Library, name: &[u8]) -> NimonResult<Symbol<T>> {
        library
            .get(name)
            .map_err(|e| NimonError::Connection(format!("Symbol not found: {}", e)))
    }

    /// Find the NI-DAQmx DLL location
    ///
    /// Searches common installation paths for nicaiu.dll.
    fn find_dll() -> NimonResult<std::path::PathBuf> {
        let candidates = [
            "C:\\Windows\\System32\\nicaiu.dll",
            "C:\\Program Files\\NI\\Shared\\nicaiu.dll",
            "C:\\Program Files (x86)\\NI\\Shared\\nicaiu.dll",
        ];

        for path in &candidates {
            if Path::new(path).exists() {
                return Ok(path.into());
            }
        }

        Err(NimonError::Connection(
            "nicaiu.dll not found. Please install NI-DAQmx.".into(),
        ))
    }

    /// Check if NI-DAQmx is available on this system
    ///
    /// Returns true if the DLL can be found, false otherwise.
    pub fn is_available() -> bool {
        Self::find_dll().is_ok()
    }

    /// Discover all DAQ devices on the system
    ///
    /// Uses DAQmxGetSysDevNames to get a comma-separated list of device names,
    /// then queries each device for its product name, product number, and serial number.
    pub fn discover_devices(&self) -> NimonResult<Vec<DaqDevice>> {
        let mut dev_names_buffer = [0i8; DAQMX_BUFFER_SIZE];

        let status = unsafe {
            (self.get_sys_dev_names)(dev_names_buffer.as_mut_ptr(), DAQMX_BUFFER_SIZE as i32)
        };
        check_status("DAQmxGetSysDevNames", status)?;

        let dev_names_str = unsafe { c_str_to_string(dev_names_buffer.as_ptr()) }
            .ok_or_else(|| NimonError::Config("Failed to read device names".into()))?;

        let device_names = parse_device_names(&dev_names_str);
        let mut devices = Vec::with_capacity(device_names.len());

        for name in device_names {
            match self.get_device_info(&name) {
                Ok(device) => devices.push(device),
                Err(e) => {
                    tracing::warn!(
                        "Failed to query info for DAQ device '{}': {}",
                        name,
                        e
                    );
                }
            }
        }

        Ok(devices)
    }

    /// Get health information for a specific DAQ device
    ///
    /// Queries the device for temperature, self-test result, and power supply voltages.
    /// Individual query failures are captured as error_message rather than returning
    /// an error, so partial health data can still be reported.
    pub fn get_device_health(&self, device_name: &str) -> NimonResult<DaqHealth> {
        let mut health = DaqHealth::new();
        let dev_cstr = string_to_c_string(device_name)
            .ok_or_else(|| NimonError::Config("Device name contains null bytes".into()))?;

        // Query temperature
        match unsafe { self.query_temperature(dev_cstr.as_ptr()) } {
            Ok(temp) => health.temperature = Some(temp),
            Err(e) => {
                health.error_message = Some(format!("Temperature query failed: {}", e));
            }
        }

        // Query self-test result
        match unsafe { self.query_self_test(dev_cstr.as_ptr()) } {
            Ok(passed) => health.self_test_passed = Some(passed),
            Err(e) => {
                let msg = format!("Self-test query failed: {}", e);
                health.error_message = Some(match health.error_message {
                    Some(existing) => format!("{}; {}", existing, msg),
                    None => msg,
                });
            }
        }

        // Query power supply voltages
        match unsafe { self.query_power_supply_voltages(dev_cstr.as_ptr()) } {
            Ok((v5, v3v3, v_user, v_neg_user)) => {
                health.voltage_5v = Some(v5);
                health.voltage_3v3 = Some(v3v3);
                health.voltage_user = Some(v_user);
                health.voltage_negative_user = Some(v_neg_user);
            }
            Err(e) => {
                let msg = format!("Power supply query failed: {}", e);
                health.error_message = Some(match health.error_message {
                    Some(existing) => format!("{}; {}", existing, msg),
                    None => msg,
                });
            }
        }

        Ok(health)
    }

    /// Reset a device to its default state
    ///
    /// # Errors
    /// Returns an error if the reset operation fails.
    pub fn reset_device(&self, device_name: &str) -> NimonResult<()> {
        let dev_cstr = string_to_c_string(device_name)
            .ok_or_else(|| NimonError::Config("Device name contains null bytes".into()))?;

        let status = unsafe { (self.reset_device)(dev_cstr.as_ptr()) };
        check_status("DAQmxResetDevice", status)
    }

    /// Query detailed information for a single DAQ device
    fn get_device_info(&self, device_name: &str) -> NimonResult<DaqDevice> {
        let mut device = DaqDevice::new(device_name.to_string());
        let dev_cstr = string_to_c_string(device_name)
            .ok_or_else(|| NimonError::Config("Device name contains null bytes".into()))?;

        // Query product type name
        let mut product_name_buffer = [0i8; 256];
        let status = unsafe {
            (self.get_dev_product_type_name)(
                dev_cstr.as_ptr(),
                product_name_buffer.as_mut_ptr(),
                256,
            )
        };
        if status == DAQMX_SUCCESS {
            if let Some(name) = unsafe { c_str_to_string(product_name_buffer.as_ptr()) } {
                device.product_name = name;
            }
        } else {
            tracing::debug!(
                "DAQmxGetDevProductTypeName for '{}' returned status {}",
                device_name,
                status
            );
        }

        // Query product number
        let mut product_number: i32 = 0;
        let status = unsafe {
            (self.get_dev_product_number)(dev_cstr.as_ptr(), &mut product_number)
        };
        if status == DAQMX_SUCCESS {
            device.product_number = product_number.to_string();
        } else {
            tracing::debug!(
                "DAQmxGetDevProductNumber for '{}' returned status {}",
                device_name,
                status
            );
        }

        // Query serial number
        let mut serial_number: u32 = 0;
        let status = unsafe {
            (self.get_dev_serial_num)(dev_cstr.as_ptr(), &mut serial_number)
        };
        if status == DAQMX_SUCCESS {
            device.serial_number = serial_number.to_string();
        } else {
            tracing::debug!(
                "DAQmxGetDevSerialNum for '{}' returned status {}",
                device_name,
                status
            );
        }

        Ok(device)
    }

    /// Query device temperature
    ///
    /// # Safety
    /// `dev_name` must be a valid pointer to a null-terminated C string.
    unsafe fn query_temperature(&self, dev_name: *const i8) -> NimonResult<f64> {
        let mut temperature: f64 = 0.0;
        let status = (self.get_dev_temperature)(dev_name, &mut temperature);
        check_status("DAQmxGetDevTemperature", status)?;
        Ok(temperature)
    }

    /// Query device self-test result
    ///
    /// # Safety
    /// `dev_name` must be a valid pointer to a null-terminated C string.
    unsafe fn query_self_test(&self, dev_name: *const i8) -> NimonResult<bool> {
        let mut self_test_result: i32 = 0;
        let status = (self.get_dev_self_test_result)(dev_name, &mut self_test_result);
        check_status("DAQmxGetDevSelfTestResult", status)?;
        Ok(self_test_result == 0)
    }

    /// Query device power supply voltages
    ///
    /// Returns (5V, 3.3V, user+, user-) voltages.
    ///
    /// # Safety
    /// `dev_name` must be a valid pointer to a null-terminated C string.
    unsafe fn query_power_supply_voltages(
        &self,
        dev_name: *const i8,
    ) -> NimonResult<(f64, f64, f64, f64)> {
        let mut v5: f64 = 0.0;
        let mut v3v3: f64 = 0.0;
        let mut v_user: f64 = 0.0;
        let mut v_neg_user: f64 = 0.0;

        let status = (self.get_dev_ai_power_supply_voltages)(
            dev_name,
            &mut v5,
            &mut v3v3,
            &mut v_user,
            &mut v_neg_user,
        );
        check_status("DAQmxGetDevAIPowerSupplyVoltages", status)?;
        Ok((v5, v3v3, v_user, v_neg_user))
    }
}

/// Parse comma-separated device names from DAQmxGetSysDevNames output
///
/// DAQmx returns names like "Dev1,Dev2,Dev3" or "Dev1, Dev2, Dev3".
/// Handles trailing commas and whitespace around names.
fn parse_device_names(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available() {
        // This will return false on systems without NI-DAQmx installed.
        // The test just verifies the function doesn't panic.
        let _ = NiDaqMx::is_available();
    }

    #[test]
    fn test_parse_device_names_single() {
        let names = parse_device_names("Dev1");
        assert_eq!(names, vec!["Dev1"]);
    }

    #[test]
    fn test_parse_device_names_multiple() {
        let names = parse_device_names("Dev1,Dev2,Dev3");
        assert_eq!(names, vec!["Dev1", "Dev2", "Dev3"]);
    }

    #[test]
    fn test_parse_device_names_with_spaces() {
        let names = parse_device_names("Dev1, Dev2, Dev3");
        assert_eq!(names, vec!["Dev1", "Dev2", "Dev3"]);
    }

    #[test]
    fn test_parse_device_names_trailing_comma() {
        let names = parse_device_names("Dev1,Dev2,");
        assert_eq!(names, vec!["Dev1", "Dev2"]);
    }

    #[test]
    fn test_parse_device_names_empty() {
        let names = parse_device_names("");
        assert!(names.is_empty());
    }

    #[test]
    fn test_parse_device_names_only_commas() {
        let names = parse_device_names(",,");
        assert!(names.is_empty());
    }

    #[test]
    fn test_parse_device_names_pxi_names() {
        let names = parse_device_names("PXI1Slot2,PXI1Slot3,PXI1Slot4");
        assert_eq!(
            names,
            vec!["PXI1Slot2", "PXI1Slot3", "PXI1Slot4"]
        );
    }

    #[test]
    fn test_parse_device_names_mixed_whitespace() {
        let names = parse_device_names("  Dev1  ,  Dev2  ,  Dev3  ");
        assert_eq!(names, vec!["Dev1", "Dev2", "Dev3"]);
    }

    #[test]
    fn test_find_dll_not_found() {
        // This test verifies the error path when DLL is not found.
        // On systems without NI-DAQmx, find_dll() returns an error.
        // We can't easily test this in isolation without mocking,
        // but we can verify the function signature is correct.
        let result = NiDaqMx::find_dll();
        // The result depends on whether NI-DAQmx is installed.
        // Just verify it doesn't panic.
        match result {
            Ok(path) => assert!(path.to_string_lossy().contains("nicaiu.dll")),
            Err(_) => (), // Expected on systems without NI-DAQmx
        }
    }
}
