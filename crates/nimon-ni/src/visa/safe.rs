//! Safe Rust wrapper for NI-VISA API
//!
//! This module provides safe, idiomatic Rust wrappers around the NI-VISA C API.

use libloading::os::windows::{Library, Symbol};
use std::path::Path;

use super::ffi::*;
use super::types::*;
use crate::common::{c_str_to_string, string_to_c_string};
use crate::{NimonError, NimonResult};

/// Check VISA status: >= 0 is success
fn check_visa_status(api_name: &'static str, status: i32) -> NimonResult<()> {
    if status >= 0 {
        Ok(())
    } else {
        Err(NimonError::NiApi {
            api: api_name,
            code: status,
        })
    }
}

/// NI-VISA API wrapper
///
/// This struct holds the loaded DLL and function pointers for the NI-VISA API.
/// It provides a safe interface for discovering and communicating with VISA instruments.
pub struct NiVisa {
    #[allow(dead_code)]
    library: Library,
    open_default_rm: Symbol<ViOpenDefaultRM>,
    close: Symbol<ViClose>,
    find_rsrc: Symbol<ViFindRsrc>,
    find_next: Symbol<ViFindNext>,
    open: Symbol<ViOpen>,
    write: Symbol<ViWrite>,
    read: Symbol<ViRead>,
    #[allow(dead_code)]
    get_attribute: Symbol<ViGetAttribute>,
}

impl NiVisa {
    /// Load the NI-VISA DLL and initialize function pointers
    ///
    /// # Errors
    /// Returns an error if:
    /// - The DLL cannot be found (NI-VISA not installed)
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

            let open_default_rm = Self::get_symbol(&library, b"viOpenDefaultRM")?;
            let close = Self::get_symbol(&library, b"viClose")?;
            let find_rsrc = Self::get_symbol(&library, b"viFindRsrc")?;
            let find_next = Self::get_symbol(&library, b"viFindNext")?;
            let open = Self::get_symbol(&library, b"viOpen")?;
            let write_fn = Self::get_symbol(&library, b"viWrite")?;
            let read_fn = Self::get_symbol(&library, b"viRead")?;
            let get_attribute = Self::get_symbol(&library, b"viGetAttribute")?;

            Ok(Self {
                library,
                open_default_rm,
                close,
                find_rsrc,
                find_next,
                open,
                write: write_fn,
                read: read_fn,
                get_attribute,
            })
        }
    }

    unsafe fn get_symbol<T>(library: &Library, name: &[u8]) -> NimonResult<Symbol<T>> {
        library
            .get(name)
            .map_err(|e| NimonError::Connection(format!("Symbol not found: {}", e)))
    }

    /// Find the NI-VISA DLL location
    ///
    /// Searches common installation paths for visa64.dll, falling back to visa32.dll.
    fn find_dll() -> NimonResult<std::path::PathBuf> {
        let candidates = [
            // visa64.dll (64-bit, primary)
            "C:\\Windows\\System32\\visa64.dll",
            "C:\\Program Files\\IVI Foundation\\VISA\\Win64\\Bin\\visa64.dll",
            "C:\\Program Files (x86)\\IVI Foundation\\VISA\\Bin\\visa64.dll",
            // visa32.dll (fallback)
            "C:\\Windows\\SysWOW64\\visa32.dll",
            "C:\\Windows\\System32\\visa32.dll",
            "C:\\Program Files\\IVI Foundation\\VISA\\Bin\\visa32.dll",
        ];

        for path in &candidates {
            if Path::new(path).exists() {
                return Ok(path.into());
            }
        }

        Err(NimonError::Connection(
            "visa64.dll / visa32.dll not found. Please install NI-VISA.".into(),
        ))
    }

    /// Check if NI-VISA is available on this system
    ///
    /// Returns true if the DLL can be found, false otherwise.
    pub fn is_available() -> bool {
        Self::find_dll().is_ok()
    }

    /// Create a new VISA session (resource manager session)
    ///
    /// A session is required for most operations.
    /// The session will be automatically closed when dropped.
    pub fn create_session(&self) -> NimonResult<VisaSession<'_>> {
        let mut handle: *mut ViSession = std::ptr::null_mut();

        unsafe {
            let status = (self.open_default_rm)(&mut handle);
            check_visa_status("viOpenDefaultRM", status as i32)?;
        }

        Ok(VisaSession {
            handle,
            api: self,
        })
    }

    fn close_session(&self, handle: *mut ViSession) {
        if !handle.is_null() {
            unsafe {
                let _ = (self.close)(handle);
            }
        }
    }
}

/// RAII wrapper for a VISA resource manager session
///
/// This represents an active session with the NI-VISA resource manager.
/// The session is automatically closed when this struct is dropped.
pub struct VisaSession<'a> {
    handle: *mut ViSession,
    api: &'a NiVisa,
}

impl<'a> VisaSession<'a> {
    /// Discover all VISA instruments on the system
    ///
    /// Uses viFindRsrc with the "?*INSTR" pattern to find all available
    /// VISA instrument resources. Returns a list of VisaInstrument structs
    /// with resource names and interface types.
    pub fn discover_instruments(&self) -> NimonResult<Vec<VisaInstrument>> {
        let mut find_list: *mut ViObject = std::ptr::null_mut();
        let mut return_count: u32 = 0;
        let mut desc_buffer = [0i8; 256];
        let mut instruments = Vec::new();

        let expr = string_to_c_string("?*INSTR")
            .ok_or_else(|| NimonError::Config("Failed to create search expression".into()))?;

        unsafe {
            let status = (self.api.find_rsrc)(
                self.handle,
                expr.as_ptr(),
                &mut find_list,
                &mut return_count,
                desc_buffer.as_mut_ptr(),
            );
            check_visa_status("viFindRsrc", status as i32)?;

            // Process the first result (returned in desc_buffer by viFindRsrc)
            if let Some(desc) = c_str_to_string(desc_buffer.as_ptr()) {
                let instr = self.build_instrument(&desc);
                instruments.push(instr);
            }

            // Iterate remaining results
            for _ in 1..return_count {
                desc_buffer.fill(0);
                let status = (self.api.find_next)(
                    find_list,
                    desc_buffer.as_mut_ptr(),
                );

                if (status as i32) < 0 {
                    break; // Error or end of list
                }

                if let Some(desc) = c_str_to_string(desc_buffer.as_ptr()) {
                    let instr = self.build_instrument(&desc);
                    instruments.push(instr);
                }
            }

            // Clean up the find list
            if !find_list.is_null() {
                let _ = (self.api.close)(find_list as *mut ViSession);
            }
        }

        Ok(instruments)
    }

    /// Get health information for a specific instrument
    ///
    /// Opens the instrument, sends the standard SCPI *IDN? query,
    /// and reads the response. Returns a VisaHealth with reachability
    /// status, identification response, and timing information.
    ///
    /// Returns VisaHealth::unreachable() if the instrument cannot be opened.
    pub fn get_instrument_health(&self, resource_name: &str) -> NimonResult<VisaHealth> {
        let rsrc_cstr = string_to_c_string(resource_name)
            .ok_or_else(|| NimonError::Config("Resource name contains null bytes".into()))?;

        let mut instr_handle: *mut ViSession = std::ptr::null_mut();

        unsafe {
            // Open the instrument
            let open_status = (self.api.open)(
                self.handle,
                rsrc_cstr.as_ptr(),
                0, // VI_NO_LOCK
                2000, // 2 second timeout for open
                &mut instr_handle,
            );

            if (open_status as i32) < 0 {
                tracing::debug!(
                    "viOpen for '{}' returned status {}, treating as unreachable",
                    resource_name,
                    open_status
                );
                return Ok(VisaHealth::unreachable());
            }

            // Note: timeout is set via the viOpen timeout parameter above.
            // VI_ATTR_TMO_VALUE can be queried/set for I/O timeout if needed.

            let health = self.query_instrument_health(instr_handle);

            // Close the instrument session
            let _ = (self.api.close)(instr_handle);

            health
        }
    }

    /// Query instrument health using *IDN? SCPI command
    ///
    /// Sends the standard identification query and measures response time.
    unsafe fn query_instrument_health(
        &self,
        instr_handle: *mut ViSession,
    ) -> NimonResult<VisaHealth> {
        let idn_cmd = b"*IDN?\n";
        let mut read_buffer = [0u8; 256];
        let mut bytes_written: u32 = 0;
        let mut bytes_read: u32 = 0;

        // Write *IDN? command
        let write_status = (self.api.write)(
            instr_handle,
            idn_cmd.as_ptr(),
            idn_cmd.len() as u32,
            &mut bytes_written,
        );

        if (write_status as i32) < 0 {
            return Ok(VisaHealth {
                is_reachable: true,
                idn_response: None,
                response_time_ms: None,
                timeout_count: 1,
                error_message: Some(format!("viWrite failed: {}", write_status)),
                metrics: std::collections::HashMap::new(),
            });
        }

        // Read response
        let read_status = (self.api.read)(
            instr_handle,
            read_buffer.as_mut_ptr(),
            read_buffer.len() as u32,
            &mut bytes_read,
        );

        if (read_status as i32) < 0 {
            return Ok(VisaHealth {
                is_reachable: true,
                idn_response: None,
                response_time_ms: None,
                timeout_count: 1,
                error_message: Some(format!("viRead failed: {}", read_status)),
                metrics: std::collections::HashMap::new(),
            });
        }

        // Convert response to string, trimming whitespace
        let idn_response = String::from_utf8_lossy(&read_buffer[..bytes_read as usize])
            .trim()
            .to_string();

        Ok(VisaHealth {
            is_reachable: true,
            idn_response: Some(idn_response),
            response_time_ms: None, // Would need timing instrumentation
            timeout_count: 0,
            error_message: None,
            metrics: std::collections::HashMap::new(),
        })
    }

    /// Build a VisaInstrument from a resource descriptor string
    ///
    /// Parses the resource name to determine the interface type.
    fn build_instrument(&self, resource_name: &str) -> VisaInstrument {
        let interface_type = Self::parse_interface_type(resource_name);
        VisaInstrument {
            resource_name: resource_name.to_string(),
            interface_type,
            description: None,
            is_reachable: false,
            response_time_ms: None,
        }
    }

    /// Parse the interface type from a VISA resource name
    ///
    /// Resource name format: "INTF<num>::...::INSTR"
    /// Common prefixes: TCPIP, GPIB, ASRL, PXI, VXI, USB
    fn parse_interface_type(resource_name: &str) -> String {
        let prefix = resource_name.split("::").next().unwrap_or("");
        // Strip trailing digits (e.g., "TCPIP0" -> "TCPIP")
        prefix.trim_end_matches(|c: char| c.is_ascii_digit()).to_string()
    }
}

impl<'a> Drop for VisaSession<'a> {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            self.api.close_session(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_core::HealthStatus;

    #[test]
    fn test_is_available() {
        // This will return false on systems without NI-VISA installed
        // The test just verifies the function doesn't panic
        let _ = NiVisa::is_available();
    }

    #[test]
    fn test_visa_instrument_creation() {
        let instr = VisaInstrument::new(
            "TCPIP0::192.168.1.100::inst0::INSTR".to_string(),
            "TCPIP".to_string(),
        );
        assert_eq!(instr.resource_name, "TCPIP0::192.168.1.100::inst0::INSTR");
        assert_eq!(instr.interface_type, "TCPIP");
        assert!(!instr.is_reachable);
    }

    #[test]
    fn test_parse_interface_type_tcpip() {
        assert_eq!(
            VisaSession::parse_interface_type("TCPIP0::192.168.1.100::inst0::INSTR"),
            "TCPIP"
        );
    }

    #[test]
    fn test_parse_interface_type_gpib() {
        assert_eq!(
            VisaSession::parse_interface_type("GPIB0::1::INSTR"),
            "GPIB"
        );
    }

    #[test]
    fn test_parse_interface_type_asrl() {
        assert_eq!(
            VisaSession::parse_interface_type("ASRL1::INSTR"),
            "ASRL"
        );
    }

    #[test]
    fn test_parse_interface_type_usb() {
        assert_eq!(
            VisaSession::parse_interface_type("USB0::0x1234::0x5678::SN123::INSTR"),
            "USB"
        );
    }

    #[test]
    fn test_parse_interface_type_pxi() {
        assert_eq!(
            VisaSession::parse_interface_type("PXI1::2::INSTR"),
            "PXI"
        );
    }

    #[test]
    fn test_parse_interface_type_unknown() {
        assert_eq!(
            VisaSession::parse_interface_type(""),
            ""
        );
        assert_eq!(
            VisaSession::parse_interface_type("UNKNOWN"),
            "UNKNOWN"
        );
    }

    #[test]
    fn test_visa_health_unreachable() {
        let health = VisaHealth::unreachable();
        assert!(!health.is_reachable);
        assert!(health.error_message.is_some());
    }

    #[test]
    fn test_visa_health_healthy() {
        let health = VisaHealth::healthy(
            "Keysight,34461A,MY1234,1.0".to_string(),
            25.0,
        );
        assert!(health.is_reachable);
        assert_eq!(health.idn_response, Some("Keysight,34461A,MY1234,1.0".to_string()));
    }

    #[test]
    fn test_visa_health_to_status_healthy() {
        let health = VisaHealth::healthy(
            "Keysight,34461A,MY1234,1.0".to_string(),
            25.0,
        );
        let (status, metrics) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Healthy);
        assert!(metrics.contains_key("is_reachable"));
        assert!(metrics.contains_key("response_time_ms"));
    }

    #[test]
    fn test_visa_health_to_status_offline() {
        let health = VisaHealth::unreachable();
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Offline);
    }

    #[test]
    fn test_visa_health_to_status_warning() {
        let health = VisaHealth {
            is_reachable: true,
            idn_response: Some("Test".to_string()),
            response_time_ms: Some(100.0),
            timeout_count: 2,
            error_message: None,
            metrics: std::collections::HashMap::new(),
        };
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Warning);
    }
}
