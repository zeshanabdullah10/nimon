//! Safe Rust wrapper for NI-DAQmx API
//!
//! Only the entry points needed for core operation are required
//! (`DAQmxGetSysDevNames`, `DAQmxResetDevice`); everything else is
//! optional so an older/trimmed driver degrades gracefully instead of
//! failing `load()` as a whole. The library is cached process-wide with
//! the same retry-after-60 s policy as NI-SysCfg.
//!
//! All calls block on the driver (a reset can take seconds); async callers
//! should use `spawn_blocking`. [`NiDaqMx`] is `Copy + Send + Sync`.

use libloading::Library;
use std::os::raw::c_char;

use super::ffi::*;
use super::types::*;
use crate::common::{
    c_buf_to_string, check_status, library_candidates, load_first_library, optional_symbol,
    required_symbol, string_to_c_string, LibCache, LOAD_RETRY_INTERVAL,
};
use crate::{NimonError, NimonResult};

#[derive(Clone, Copy)]
struct DaqmxApi {
    // required
    get_sys_dev_names: DAQmxGetSysDevNames,
    reset_device: DAQmxResetDevice,
    // optional
    self_test_device: Option<DAQmxSelfTestDevice>,
    get_dev_product_type: Option<DAQmxGetDevProductType>,
    get_dev_product_num: Option<DAQmxGetDevProductNum>,
    get_dev_serial_num: Option<DAQmxGetDevSerialNum>,
    get_dev_is_simulated: Option<DAQmxGetDevIsSimulated>,
    get_cal_dev_temp: Option<DAQmxGetCalDevTemp>,
    get_extended_error_info: Option<DAQmxGetExtendedErrorInfo>,
}

struct DaqmxLoaded {
    _library: Library,
    api: DaqmxApi,
}

static DAQMX: LibCache<DaqmxLoaded> = LibCache::new(LOAD_RETRY_INTERVAL);

fn daqmx() -> Result<&'static DaqmxLoaded, String> {
    DAQMX.get_or_load(|| unsafe { load_once() })
}

unsafe fn load_once() -> Result<DaqmxLoaded, String> {
    let candidates = library_candidates(
        "nicaiu.dll",
        &[],
        &[
            "libnidaqmx.so",
            "libnidaqmx.so.1",
            "/usr/local/natinst/lib/libnidaqmx.so",
        ],
    );
    let library = load_first_library("NI-DAQmx (nicaiu)", &candidates)?;
    let api = DaqmxApi {
        get_sys_dev_names: required_symbol(&library, "DAQmxGetSysDevNames")?,
        reset_device: required_symbol(&library, "DAQmxResetDevice")?,
        self_test_device: optional_symbol(&library, "DAQmxSelfTestDevice"),
        get_dev_product_type: optional_symbol(&library, "DAQmxGetDevProductType"),
        get_dev_product_num: optional_symbol(&library, "DAQmxGetDevProductNum"),
        get_dev_serial_num: optional_symbol(&library, "DAQmxGetDevSerialNum"),
        get_dev_is_simulated: optional_symbol(&library, "DAQmxGetDevIsSimulated"),
        get_cal_dev_temp: optional_symbol(&library, "DAQmxGetCalDevTemp"),
        get_extended_error_info: optional_symbol(&library, "DAQmxGetExtendedErrorInfo"),
    };
    Ok(DaqmxLoaded {
        _library: library,
        api,
    })
}

/// Whether a DAQmx error only means "property not supported here"
fn is_unsupported(status: i32) -> bool {
    matches!(
        status,
        errors::ATTR_NOT_SUPPORTED
            | errors::ATTRIBUTE_NOT_SUPPORTED_IN_TASK_CONTEXT
            | errors::ATTR_NOT_SUPPORTED_ON_ACCESSORY
            | errors::ATTR_NOT_SUPPORTED_USE_PHYSICAL_CHANNEL_PROPERTY
    )
}

/// Read a DAQmx string using the `(NULL, 0)` size-query convention.
/// `call(buf, size)` must invoke the DAQmx getter.
fn read_daqmx_string(
    api_name: &'static str,
    mut call: impl FnMut(*mut c_char, u32) -> i32,
) -> NimonResult<String> {
    let needed = call(std::ptr::null_mut(), 0);
    if needed < 0 {
        return Err(NimonError::NiApi {
            api: api_name,
            code: needed,
        });
    }
    if needed == 0 {
        return Ok(String::new());
    }
    // +1 guards against drivers that exclude the terminator
    let size = (needed as usize).saturating_add(1).min(DAQMX_MAX_STRING);
    let mut buffer = vec![0 as c_char; size];
    let status = call(buffer.as_mut_ptr(), size as u32);
    check_status(api_name, status)?;
    Ok(c_buf_to_string(&buffer))
}

/// NI-DAQmx API wrapper
///
/// Cheap to obtain and `Copy`: the library and entry points are cached
/// process-wide.
#[derive(Clone, Copy)]
pub struct NiDaqMx {
    api: DaqmxApi,
}

impl NiDaqMx {
    /// Load NI-DAQmx (cached after the first successful call; failures are
    /// retried after 60 s)
    ///
    /// # Errors
    /// Returns an error if the library cannot be found/loaded or lacks
    /// `DAQmxGetSysDevNames` / `DAQmxResetDevice`.
    pub fn load() -> NimonResult<Self> {
        daqmx()
            .map(|l| NiDaqMx { api: l.api })
            .map_err(NimonError::Connection)
    }

    /// Check if NI-DAQmx is available on this system
    ///
    /// Actually attempts the (cached) load.
    pub fn is_available() -> bool {
        daqmx().is_ok()
    }

    /// Extended description of the most recent DAQmx error **on the
    /// calling thread** (DAQmxGetExtendedErrorInfo), if available.
    pub fn last_error_message(&self) -> Option<String> {
        let f = self.api.get_extended_error_info?;
        // fixed buffer (as in NI's examples): a separate size-query call
        // could itself invalidate the per-thread error information
        let mut buffer = vec![0 as c_char; 2048];
        let status = unsafe { f(buffer.as_mut_ptr(), buffer.len() as u32) };
        if status < 0 {
            return None;
        }
        Some(c_buf_to_string(&buffer).trim().to_string()).filter(|s| !s.is_empty())
    }

    /// Log and convert a status, attaching DAQmx's extended error info
    fn check(&self, api_name: &'static str, status: i32, device: &str) -> NimonResult<()> {
        if status < 0 {
            if let Some(msg) = self.last_error_message() {
                tracing::warn!("{api_name}('{device}') failed ({status}): {msg}");
            }
        }
        check_status(api_name, status)
    }

    /// Names of all DAQmx devices on the system (DAQmxGetSysDevNames)
    pub fn device_names(&self) -> NimonResult<Vec<String>> {
        let f = self.api.get_sys_dev_names;
        let names = read_daqmx_string("DAQmxGetSysDevNames", |buf, size| unsafe { f(buf, size) })?;
        Ok(parse_device_names(&names))
    }

    /// Discover all DAQ devices on the system
    ///
    /// Queries each device for product type, product number, serial number
    /// and simulation state where the driver supports it.
    pub fn discover_devices(&self) -> NimonResult<Vec<DaqDevice>> {
        let mut devices = Vec::new();
        for name in self.device_names()? {
            match self.get_device_info(&name) {
                Ok(device) => devices.push(device),
                Err(e) => tracing::warn!("Failed to query info for DAQ device '{name}': {e}"),
            }
        }
        Ok(devices)
    }

    /// Get health information for a specific DAQ device
    ///
    /// Non-invasive: a basic identity query decides reachability and the
    /// calibration temperature is read where supported. No self-test is
    /// run (see [`NiDaqMx::self_test_device`]).
    pub fn get_device_health(&self, device_name: &str) -> NimonResult<DaqHealth> {
        let dev = string_to_c_string(device_name)
            .ok_or_else(|| NimonError::Config("Device name contains null bytes".into()))?;
        let mut health = DaqHealth::new();

        // Reachability probe: serial number, else product type
        let probe = if let Some(f) = self.api.get_dev_serial_num {
            let mut serial: u32 = 0;
            Some(unsafe { f(dev.as_ptr(), &mut serial) })
        } else {
            self.api.get_dev_product_type.map(|f| {
                let mut buf = [0 as c_char; 256];
                unsafe { f(dev.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) }
            })
        };
        match probe {
            Some(status) if status < 0 => {
                let detail = self.last_error_message();
                health.is_reachable = Some(false);
                health.error_message = Some(match detail {
                    Some(d) => format!("Device query failed ({status}): {d}"),
                    None => format!("Device query failed ({status})"),
                });
                return Ok(health);
            }
            Some(_) => health.is_reachable = Some(true),
            None => {}
        }

        if let Some(f) = self.api.get_cal_dev_temp {
            let mut temp: f64 = 0.0;
            let status = unsafe { f(dev.as_ptr(), &mut temp) };
            if status >= 0 {
                health.temperature = Some(temp).filter(|t| t.is_finite());
            } else if !is_unsupported(status) {
                health.error_message = Some(format!("Temperature query failed: status {status}"));
            }
        }

        Ok(health)
    }

    /// Reset a device to its default state (aborts its tasks)
    ///
    /// # Errors
    /// Returns an error if the reset operation fails.
    pub fn reset_device(&self, device_name: &str) -> NimonResult<()> {
        let dev = string_to_c_string(device_name)
            .ok_or_else(|| NimonError::Config("Device name contains null bytes".into()))?;
        let status = unsafe { (self.api.reset_device)(dev.as_ptr()) };
        self.check("DAQmxResetDevice", status, device_name)
    }

    /// Run the device self-test (DAQmxSelfTestDevice)
    ///
    /// Returns `Ok(true)` when the test passes and `Ok(false)` when the
    /// driver reports a failure; `Err` if the call itself is unavailable.
    /// Invasive: the device must not be running tasks.
    pub fn self_test_device(&self, device_name: &str) -> NimonResult<bool> {
        let f = self.api.self_test_device.ok_or_else(|| {
            NimonError::Connection("DAQmxSelfTestDevice not exported by this driver".into())
        })?;
        let dev = string_to_c_string(device_name)
            .ok_or_else(|| NimonError::Config("Device name contains null bytes".into()))?;
        let status = unsafe { f(dev.as_ptr()) };
        if status < 0 {
            if let Some(msg) = self.last_error_message() {
                tracing::warn!("DAQmxSelfTestDevice('{device_name}') failed ({status}): {msg}");
            }
            return Ok(false);
        }
        Ok(true)
    }

    /// Query detailed information for a single DAQ device
    fn get_device_info(&self, device_name: &str) -> NimonResult<DaqDevice> {
        let mut device = DaqDevice::new(device_name.to_string());
        let dev = string_to_c_string(device_name)
            .ok_or_else(|| NimonError::Config("Device name contains null bytes".into()))?;

        if let Some(f) = self.api.get_dev_product_type {
            match read_daqmx_string("DAQmxGetDevProductType", |buf, size| unsafe {
                f(dev.as_ptr(), buf, size)
            }) {
                Ok(name) => device.product_name = name,
                Err(e) => tracing::debug!("{e} for '{device_name}'"),
            }
        }
        if let Some(f) = self.api.get_dev_product_num {
            let mut num: u32 = 0;
            let status = unsafe { f(dev.as_ptr(), &mut num) };
            if status >= 0 {
                device.product_number = format_product_number(num);
            } else {
                tracing::debug!("DAQmxGetDevProductNum for '{device_name}' returned {status}");
            }
        }
        if let Some(f) = self.api.get_dev_serial_num {
            let mut serial: u32 = 0;
            let status = unsafe { f(dev.as_ptr(), &mut serial) };
            if status >= 0 {
                device.serial_number = format_serial(serial);
            } else {
                tracing::debug!("DAQmxGetDevSerialNum for '{device_name}' returned {status}");
            }
        }
        if let Some(f) = self.api.get_dev_is_simulated {
            let mut sim: Bool32 = 0;
            if unsafe { f(dev.as_ptr(), &mut sim) } >= 0 {
                device.is_simulated = sim != 0;
            }
        }
        Ok(device)
    }
}

/// DAQmx product numbers are hardware IDs, conventionally shown in hex
fn format_product_number(num: u32) -> String {
    format!("0x{num:04X}")
}

/// Serial numbers are shown in hex by NI MAX / NI-SysCfg; 0 = none
/// (simulated devices)
fn format_serial(serial: u32) -> String {
    if serial == 0 {
        String::new()
    } else {
        format!("{serial:X}")
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
        // Must not panic whether or not NI-DAQmx is installed
        let _ = NiDaqMx::is_available();
    }

    #[test]
    fn test_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NiDaqMx>();
    }

    #[test]
    fn test_load_consistent_with_is_available() {
        assert_eq!(NiDaqMx::load().is_ok(), NiDaqMx::is_available());
    }

    #[test]
    fn test_parse_device_names_single() {
        assert_eq!(parse_device_names("Dev1"), vec!["Dev1"]);
    }

    #[test]
    fn test_parse_device_names_multiple() {
        assert_eq!(
            parse_device_names("Dev1,Dev2,Dev3"),
            vec!["Dev1", "Dev2", "Dev3"]
        );
    }

    #[test]
    fn test_parse_device_names_with_spaces() {
        assert_eq!(
            parse_device_names("Dev1, Dev2, Dev3"),
            vec!["Dev1", "Dev2", "Dev3"]
        );
    }

    #[test]
    fn test_parse_device_names_trailing_comma() {
        assert_eq!(parse_device_names("Dev1,Dev2,"), vec!["Dev1", "Dev2"]);
    }

    #[test]
    fn test_parse_device_names_empty() {
        assert!(parse_device_names("").is_empty());
    }

    #[test]
    fn test_parse_device_names_only_commas() {
        assert!(parse_device_names(",,").is_empty());
    }

    #[test]
    fn test_parse_device_names_pxi_names() {
        assert_eq!(
            parse_device_names("PXI1Slot2,PXI1Slot3,PXI1Slot4"),
            vec!["PXI1Slot2", "PXI1Slot3", "PXI1Slot4"]
        );
    }

    #[test]
    fn test_parse_device_names_mixed_whitespace() {
        assert_eq!(
            parse_device_names("  Dev1  ,  Dev2  ,  Dev3  "),
            vec!["Dev1", "Dev2", "Dev3"]
        );
    }

    #[test]
    fn test_read_daqmx_string_size_query() {
        let src = b"Dev1, Dev2\0";
        let s = read_daqmx_string("Test", |buf, size| {
            if buf.is_null() {
                return src.len() as i32; // required size incl. NUL
            }
            let n = (size as usize).min(src.len());
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr() as *const c_char, buf, n) };
            0
        })
        .unwrap();
        assert_eq!(s, "Dev1, Dev2");
    }

    #[test]
    fn test_read_daqmx_string_empty_and_errors() {
        assert_eq!(read_daqmx_string("Test", |_, _| 0).unwrap(), "");
        assert!(matches!(
            read_daqmx_string("Test", |_, _| -200220),
            Err(NimonError::NiApi { code: -200220, .. })
        ));
        // size query ok, fill fails
        let r = read_daqmx_string("Test", |buf, _| if buf.is_null() { 8 } else { -200228 });
        assert!(r.is_err());
        // fill returns a warning: value kept
        let r = read_daqmx_string("Test", |buf, _| {
            if buf.is_null() {
                2
            } else {
                unsafe { *buf = b'A' as c_char };
                200_000
            }
        });
        assert_eq!(r.unwrap(), "A");
    }

    #[test]
    fn test_is_unsupported() {
        assert!(is_unsupported(-200197));
        assert!(is_unsupported(-200452));
        assert!(!is_unsupported(-200220)); // invalid device ID is a real error
        assert!(!is_unsupported(0));
    }

    #[test]
    fn test_formatting() {
        assert_eq!(format_product_number(0x7262), "0x7262");
        assert_eq!(format_product_number(0x1B), "0x001B");
        assert_eq!(format_serial(0x01A2B3C4), "1A2B3C4");
        assert_eq!(format_serial(0), "");
    }
}
