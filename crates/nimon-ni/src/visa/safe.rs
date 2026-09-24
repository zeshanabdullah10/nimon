//! Safe Rust wrapper for NI-VISA API
//!
//! The VISA library is loaded once per process and cached (a load failure
//! is retried after 60 s). All calls block (an `*IDN?` probe waits up to
//! the I/O timeout); async callers should use `spawn_blocking`.
//! VISA is specified as multithread-safe, so [`VisaSession`] is
//! `Send + Sync`.

use libloading::Library;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::os::raw::c_char;
use std::time::Instant;

use super::ffi::*;
use super::types::*;
use crate::common::{
    c_buf_to_string, library_candidates, load_first_library, optional_symbol, required_symbol,
    string_to_c_string, LibCache, LOAD_RETRY_INTERVAL,
};
use crate::{NimonError, NimonResult};

/// viOpen lock-wait timeout. Only meaningful when a lock is requested (we
/// use VI_NO_LOCK); some implementations also use it while connecting.
const OPEN_TIMEOUT_MS: u32 = 2000;
/// I/O timeout set through VI_ATTR_TMO_VALUE after opening
pub const DEFAULT_IO_TIMEOUT_MS: u32 = 2000;
/// `*IDN?` response buffer
const IDN_BUFFER_LEN: usize = 1024;

/// Check VISA status: >= 0 is success (positive = completion code)
fn check_visa_status(api_name: &'static str, status: ViStatus) -> NimonResult<()> {
    if status >= 0 {
        Ok(())
    } else {
        Err(NimonError::NiApi {
            api: api_name,
            code: status,
        })
    }
}

#[derive(Clone, Copy)]
struct VisaApi {
    open_default_rm: ViOpenDefaultRM,
    close: ViClose,
    find_rsrc: ViFindRsrc,
    find_next: ViFindNext,
    open: ViOpen,
    write: ViWrite,
    read: ViRead,
    get_attribute: Option<ViGetAttribute>,
    set_attribute: Option<ViSetAttribute>,
}

struct VisaLoaded {
    _library: Library,
    api: VisaApi,
}

static VISA: LibCache<VisaLoaded> = LibCache::new(LOAD_RETRY_INTERVAL);

fn visa() -> Result<&'static VisaLoaded, String> {
    VISA.get_or_load(|| unsafe { load_once() })
}

#[cfg(target_pointer_width = "64")]
const VISA_DLL: &str = "visa64.dll";
/// On 64-bit Windows System32\visa32.dll is also a 64-bit router
#[cfg(target_pointer_width = "64")]
const VISA_EXTRA_WINDOWS: &[&str] = &[
    "C:\\Program Files\\IVI Foundation\\VISA\\Win64\\Bin\\visa64.dll",
    "visa32.dll",
];
#[cfg(not(target_pointer_width = "64"))]
const VISA_DLL: &str = "visa32.dll";
#[cfg(not(target_pointer_width = "64"))]
const VISA_EXTRA_WINDOWS: &[&str] = &[];

unsafe fn load_once() -> Result<VisaLoaded, String> {
    let candidates = library_candidates(
        VISA_DLL,
        VISA_EXTRA_WINDOWS,
        &[
            "libvisa.so",
            "/usr/local/vxipnp/linux/lib64/libvisa.so",
            "/usr/local/vxipnp/linux/lib/libvisa.so",
        ],
    );
    let library = load_first_library("NI-VISA", &candidates)?;
    let api = VisaApi {
        open_default_rm: required_symbol(&library, "viOpenDefaultRM")?,
        close: required_symbol(&library, "viClose")?,
        find_rsrc: required_symbol(&library, "viFindRsrc")?,
        find_next: required_symbol(&library, "viFindNext")?,
        open: required_symbol(&library, "viOpen")?,
        write: required_symbol(&library, "viWrite")?,
        read: required_symbol(&library, "viRead")?,
        get_attribute: optional_symbol(&library, "viGetAttribute"),
        set_attribute: optional_symbol(&library, "viSetAttribute"),
    };
    Ok(VisaLoaded {
        _library: library,
        api,
    })
}

/// RAII guard for any VISA object (RM session, instrument, find list):
/// `viClose` exactly once, skipped for VI_NULL
struct ViGuard {
    close: ViClose,
    vi: ViObject,
}

impl Drop for ViGuard {
    fn drop(&mut self) {
        if self.vi != VI_NULL {
            unsafe { (self.close)(self.vi) };
            self.vi = VI_NULL;
        }
    }
}

/// NI-VISA API wrapper
///
/// Cheap to obtain and `Copy`: the library and entry points are cached
/// process-wide.
#[derive(Clone, Copy)]
pub struct NiVisa {
    api: VisaApi,
}

impl NiVisa {
    /// Load NI-VISA (cached after the first successful call; failures are
    /// retried after 60 s)
    ///
    /// # Errors
    /// Returns an error if the library cannot be found/loaded or a
    /// required symbol is missing.
    pub fn load() -> NimonResult<Self> {
        visa()
            .map(|l| NiVisa { api: l.api })
            .map_err(NimonError::Connection)
    }

    /// Check if NI-VISA is available on this system
    ///
    /// Actually attempts the (cached) load.
    pub fn is_available() -> bool {
        visa().is_ok()
    }

    /// Open the default resource manager session
    ///
    /// The session is closed automatically when dropped.
    pub fn create_session(&self) -> NimonResult<VisaSession<'_>> {
        let mut rm: ViSession = VI_NULL;
        let status = unsafe { (self.api.open_default_rm)(&mut rm) };
        let guard = ViGuard {
            close: self.api.close,
            vi: rm,
        };
        check_visa_status("viOpenDefaultRM", status)?;
        if guard.vi == VI_NULL {
            return Err(NimonError::Connection(
                "viOpenDefaultRM returned VI_NULL".into(),
            ));
        }
        Ok(VisaSession {
            api: self.api,
            rm: guard,
            _api: PhantomData,
        })
    }
}

/// Outcome of an `*IDN?` round trip
enum IdnOutcome {
    Ok { response: String, elapsed_ms: f64 },
    WriteFailed(ViStatus),
    ReadFailed(ViStatus),
}

/// RAII wrapper for a VISA resource manager session
pub struct VisaSession<'a> {
    api: VisaApi,
    rm: ViGuard,
    _api: PhantomData<&'a NiVisa>,
}

impl VisaSession<'_> {
    /// Find resources matching a VISA expression (e.g. `"?*INSTR"`).
    ///
    /// Returns `Ok(vec![])` when nothing matches (VI_ERROR_RSRC_NFOUND).
    pub fn find_resources(&self, expr: &str) -> NimonResult<Vec<String>> {
        let expr = string_to_c_string(expr)
            .ok_or_else(|| NimonError::Config("Search expression contains null bytes".into()))?;
        let mut find_list: ViFindList = VI_NULL;
        let mut count: ViUInt32 = 0;
        let mut desc = [0 as c_char; VI_FIND_BUFLEN];

        let status = unsafe {
            (self.api.find_rsrc)(
                self.rm.vi,
                expr.as_ptr(),
                &mut find_list,
                &mut count,
                desc.as_mut_ptr(),
            )
        };
        let _list = ViGuard {
            close: self.api.close,
            vi: find_list,
        };
        if status == VI_ERROR_RSRC_NFOUND {
            return Ok(Vec::new());
        }
        check_visa_status("viFindRsrc", status)?;

        // `count` comes from the driver: bound it before it sizes an
        // allocation or drives the loop.
        const MAX_RESOURCES: u32 = 1024;
        let count = count.min(MAX_RESOURCES);
        let mut names = Vec::with_capacity(count as usize);
        let first = c_buf_to_string(&desc);
        if !first.is_empty() {
            names.push(first);
        }
        for _ in 1..count {
            desc.fill(0);
            let status = unsafe { (self.api.find_next)(find_list, desc.as_mut_ptr()) };
            if status < 0 {
                break; // error or end of list
            }
            let name = c_buf_to_string(&desc);
            if !name.is_empty() {
                names.push(name);
            }
        }
        Ok(names)
        // _list dropped: find list closed exactly once
    }

    /// List all VISA instruments (`"?*INSTR"`) without talking to them
    ///
    /// No I/O is performed, so `is_reachable` is `false` ("not probed")
    /// and `description`/`response_time_ms` are `None`. Use
    /// [`VisaSession::discover_and_probe`] or
    /// [`VisaSession::probe_instrument`] to fill them.
    pub fn discover_instruments(&self) -> NimonResult<Vec<VisaInstrument>> {
        Ok(self
            .find_resources("?*INSTR")?
            .into_iter()
            .map(|name| {
                let intf = Self::parse_interface_type(&name);
                VisaInstrument::new(name, intf)
            })
            .collect())
    }

    /// List all instruments and probe each with `*IDN?`
    ///
    /// Blocks up to the I/O timeout per unresponsive instrument. Note that
    /// this writes `*IDN?` to every resource, including serial ports.
    pub fn discover_and_probe(&self) -> NimonResult<Vec<VisaInstrument>> {
        Ok(self
            .find_resources("?*INSTR")?
            .iter()
            .map(|name| self.probe_instrument(name))
            .collect())
    }

    /// Open one instrument and identify it with `*IDN?`
    ///
    /// `is_reachable` is true when the query succeeded; `description` is
    /// "manufacturer model" from the response; `response_time_ms` is the
    /// measured round trip.
    pub fn probe_instrument(&self, resource_name: &str) -> VisaInstrument {
        let mut instr = VisaInstrument::new(
            resource_name.to_string(),
            Self::parse_interface_type(resource_name),
        );
        let Ok(Some(vi)) = self.open_instrument(resource_name, DEFAULT_IO_TIMEOUT_MS) else {
            return instr;
        };
        if let Some(name) = self.interface_type(vi.vi).and_then(intf_type_name) {
            instr.interface_type = name.to_string();
        }
        if let IdnOutcome::Ok {
            response,
            elapsed_ms,
        } = self.query_idn(vi.vi)
        {
            instr.is_reachable = true;
            instr.description = idn_description(&response);
            instr.response_time_ms = Some(elapsed_ms);
        }
        instr
    }

    /// Get health information for a specific instrument
    ///
    /// Opens the instrument, sets the I/O timeout, sends `*IDN?` and times
    /// the round trip. Returns `VisaHealth::unreachable()` if the
    /// instrument cannot be opened.
    pub fn get_instrument_health(&self, resource_name: &str) -> NimonResult<VisaHealth> {
        let Some(vi) = self.open_instrument(resource_name, DEFAULT_IO_TIMEOUT_MS)? else {
            return Ok(VisaHealth::unreachable());
        };
        let failed = |what: &str, status: ViStatus| VisaHealth {
            is_reachable: true,
            idn_response: None,
            response_time_ms: None,
            timeout_count: 1,
            error_message: Some(if status == VI_ERROR_TMO {
                format!("{what} timed out")
            } else {
                format!("{what} failed: status {status}")
            }),
            metrics: HashMap::new(),
        };
        Ok(match self.query_idn(vi.vi) {
            IdnOutcome::Ok {
                response,
                elapsed_ms,
            } => VisaHealth::healthy(response, elapsed_ms),
            IdnOutcome::WriteFailed(s) => failed("viWrite", s),
            IdnOutcome::ReadFailed(s) => failed("viRead", s),
        })
        // vi dropped: instrument session closed exactly once
    }

    /// Open an instrument session with an explicit I/O timeout.
    /// `Ok(None)` when viOpen fails (instrument unreachable).
    fn open_instrument(
        &self,
        resource_name: &str,
        io_timeout_ms: u32,
    ) -> NimonResult<Option<ViGuard>> {
        let rsrc = string_to_c_string(resource_name)
            .ok_or_else(|| NimonError::Config("Resource name contains null bytes".into()))?;
        let mut vi: ViSession = VI_NULL;
        let status = unsafe {
            (self.api.open)(
                self.rm.vi,
                rsrc.as_ptr(),
                VI_NO_LOCK,
                OPEN_TIMEOUT_MS,
                &mut vi,
            )
        };
        let guard = ViGuard {
            close: self.api.close,
            vi,
        };
        if status < 0 || guard.vi == VI_NULL {
            tracing::debug!("viOpen('{resource_name}') returned {status}, treating as unreachable");
            return Ok(None);
        }
        // viOpen's timeout is the lock timeout; the I/O timeout is an attribute
        if let Some(set) = self.api.set_attribute {
            let st = unsafe {
                set(
                    guard.vi,
                    attributes::VI_ATTR_TMO_VALUE,
                    io_timeout_ms as ViAttrState,
                )
            };
            if st < 0 {
                tracing::debug!("VI_ATTR_TMO_VALUE on '{resource_name}' failed: {st}");
            }
        }
        Ok(Some(guard))
    }

    /// VI_ATTR_INTF_TYPE (ViUInt16) of an open session
    fn interface_type(&self, vi: ViObject) -> Option<u16> {
        let get = self.api.get_attribute?;
        let mut value: u16 = 0;
        let status = unsafe {
            get(
                vi,
                attributes::VI_ATTR_INTF_TYPE,
                (&mut value as *mut u16).cast(),
            )
        };
        (status >= 0).then_some(value)
    }

    /// Send `*IDN?` and read the reply, timing the round trip
    fn query_idn(&self, vi: ViSession) -> IdnOutcome {
        let cmd = b"*IDN?\n";
        let mut buffer = [0u8; IDN_BUFFER_LEN];
        let mut written: ViUInt32 = 0;
        let mut read: ViUInt32 = 0;

        let start = Instant::now();
        let status = unsafe { (self.api.write)(vi, cmd.as_ptr(), cmd.len() as u32, &mut written) };
        if status < 0 {
            return IdnOutcome::WriteFailed(status);
        }
        let status =
            unsafe { (self.api.read)(vi, buffer.as_mut_ptr(), buffer.len() as u32, &mut read) };
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        if status < 0 {
            return IdnOutcome::ReadFailed(status);
        }
        IdnOutcome::Ok {
            response: decode_response(&buffer, read),
            elapsed_ms,
        }
    }

    /// Parse the interface type from a VISA resource name
    ///
    /// Resource name format: "INTF<num>::...::INSTR"
    /// Common prefixes: TCPIP, GPIB, ASRL, PXI, VXI, USB
    fn parse_interface_type(resource_name: &str) -> String {
        let prefix = resource_name.split("::").next().unwrap_or("");
        // Strip trailing digits (e.g., "TCPIP0" -> "TCPIP")
        prefix
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .to_string()
    }
}

/// Decode a viRead reply; `bytes_read` is clamped to the buffer so a
/// misbehaving driver cannot cause an out-of-bounds slice
fn decode_response(buffer: &[u8], bytes_read: u32) -> String {
    let n = (bytes_read as usize).min(buffer.len());
    String::from_utf8_lossy(&buffer[..n]).trim().to_string()
}

/// "manufacturer model" from an IEEE 488.2 `*IDN?` reply
/// ("manufacturer,model,serial,firmware")
fn idn_description(idn: &str) -> Option<String> {
    let mut fields = idn.split(',').map(str::trim).filter(|s| !s.is_empty());
    let desc = match (fields.next(), fields.next()) {
        (Some(manufacturer), Some(model)) => format!("{manufacturer} {model}"),
        (Some(only), None) => only.to_string(),
        _ => return None,
    };
    Some(desc)
}

/// Name for a VI_ATTR_INTF_TYPE value (matches resource-name prefixes)
fn intf_type_name(value: u16) -> Option<&'static str> {
    Some(match value {
        intf::VI_INTF_GPIB => "GPIB",
        intf::VI_INTF_VXI => "VXI",
        intf::VI_INTF_GPIB_VXI => "GPIB-VXI",
        intf::VI_INTF_ASRL => "ASRL",
        intf::VI_INTF_PXI => "PXI",
        intf::VI_INTF_TCPIP => "TCPIP",
        intf::VI_INTF_USB => "USB",
        intf::VI_INTF_RIO => "RIO",
        intf::VI_INTF_FIREWIRE => "FIREWIRE",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_core::HealthStatus;

    #[test]
    fn test_is_available() {
        // Must not panic whether or not NI-VISA is installed
        let _ = NiVisa::is_available();
    }

    #[test]
    fn test_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NiVisa>();
        assert_send_sync::<VisaSession<'static>>();
    }

    #[test]
    fn test_ffi_constants_match_visa_h() {
        assert_eq!(attributes::VI_ATTR_TMO_VALUE, 0x3FFF001A);
        assert_eq!(attributes::VI_ATTR_INTF_TYPE, 0x3FFF0171);
        assert_eq!(VI_ERROR_RSRC_NFOUND, -1073807343);
        assert_eq!(VI_ERROR_TMO, -1073807339);
        assert_eq!(
            std::mem::size_of::<ViAttrState>(),
            std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn test_find_resources_empty_is_ok_when_installed() {
        // Real VISA only: a pattern that cannot match must be Ok(empty)
        if let Ok(api) = NiVisa::load() {
            if let Ok(session) = api.create_session() {
                let r = session.find_resources("NOSUCHINTF99::?*::INSTR");
                assert!(matches!(r, Ok(ref v) if v.is_empty()), "{r:?}");
            }
        }
    }

    #[test]
    fn test_decode_response_clamps_bytes_read() {
        let buf = *b"KEYSIGHT,34461A\n";
        assert_eq!(decode_response(&buf, 1_000_000), "KEYSIGHT,34461A");
        assert_eq!(decode_response(&buf, 8), "KEYSIGHT");
        assert_eq!(decode_response(&buf, 0), "");
    }

    #[test]
    fn test_idn_description() {
        assert_eq!(
            idn_description("Keysight Technologies,34461A,MY1234,A.02.14"),
            Some("Keysight Technologies 34461A".to_string())
        );
        assert_eq!(idn_description("SIMPLE"), Some("SIMPLE".to_string()));
        assert_eq!(idn_description(" , "), None);
        assert_eq!(idn_description(""), None);
    }

    #[test]
    fn test_intf_type_name() {
        assert_eq!(intf_type_name(6), Some("TCPIP"));
        assert_eq!(intf_type_name(4), Some("ASRL"));
        assert_eq!(intf_type_name(0), None);
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
        assert_eq!(VisaSession::parse_interface_type("GPIB0::1::INSTR"), "GPIB");
    }

    #[test]
    fn test_parse_interface_type_asrl() {
        assert_eq!(VisaSession::parse_interface_type("ASRL1::INSTR"), "ASRL");
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
        assert_eq!(VisaSession::parse_interface_type("PXI1::2::INSTR"), "PXI");
    }

    #[test]
    fn test_parse_interface_type_unknown() {
        assert_eq!(VisaSession::parse_interface_type(""), "");
        assert_eq!(VisaSession::parse_interface_type("UNKNOWN"), "UNKNOWN");
    }

    #[test]
    fn test_visa_health_unreachable() {
        let health = VisaHealth::unreachable();
        assert!(!health.is_reachable);
        assert!(health.error_message.is_some());
    }

    #[test]
    fn test_visa_health_healthy() {
        let health = VisaHealth::healthy("Keysight,34461A,MY1234,1.0".to_string(), 25.0);
        assert!(health.is_reachable);
        assert_eq!(
            health.idn_response,
            Some("Keysight,34461A,MY1234,1.0".to_string())
        );
    }

    #[test]
    fn test_visa_health_to_status_healthy() {
        let health = VisaHealth::healthy("Keysight,34461A,MY1234,1.0".to_string(), 25.0);
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
            metrics: HashMap::new(),
        };
        let (status, _) = health.to_status_and_metrics();
        assert_eq!(status, HealthStatus::Warning);
    }
}
