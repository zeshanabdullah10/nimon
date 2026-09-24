//! Safe Rust wrapper for NI-SysCfg API
//!
//! The library is loaded once per process and the resolved function
//! pointers are cached ([`crate::common::LibCache`]); a load *failure* is
//! retried after [`LOAD_RETRY_INTERVAL`] so installing or repairing NI
//! drivers does not require restarting the edge.
//!
//! # Threading
//! Every call in this module blocks on the NI driver (session creation can
//! take up to the 10 s connect timeout, a hardware enumeration can take
//! seconds). Async callers must run them on a blocking thread
//! (`tokio::task::spawn_blocking` / `actix_rt::task::spawn_blocking`).
//!
//! [`SysCfgSession`] is `Send + Sync`: NI-SysCfg handles are plain
//! process-wide handles, not bound to the creating thread, so a session
//! may be created once and used from whichever blocking-pool thread runs
//! the next sweep (e.g. kept in an `Arc<SysCfgSession>`). Calls on one
//! session are serialised by an internal mutex.

use libloading::Library;
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::raw::{c_char, c_int};
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::ffi::*;
use super::types::*;
use crate::common::{
    c_buf_to_string, c_str_to_string, check_status, library_candidates, load_first_library,
    optional_symbol, required_symbol, LibCache, LOAD_RETRY_INTERVAL,
};
use crate::{NimonError, NimonResult};

/// Connect timeout passed to NISysCfgInitializeSession
const CONNECT_TIMEOUT_MS: u32 = 10_000;
/// Hard bound on resources per enumeration (guards against a driver that
/// never signals end-of-enumeration)
const MAX_RESOURCES: usize = 4096;
/// Bound on per-resource indexed property counts (experts, sensors)
const MAX_INDEXED: c_int = 64;

/// The same resources fail extraction on every sweep (e.g. non-device
/// entries), so warn once per process and log repeats at debug.
static SKIP_WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn log_skipped_resource(e: &NimonError) {
    if SKIP_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::debug!("Skipping resource: failed to extract device info: {e}");
    } else {
        tracing::warn!(
            "Skipping resource: failed to extract device info: {e} (further occurrences logged at debug)"
        );
    }
}

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
    // optional: only used for friendlier error logs
    get_status_description: Option<NISysCfgGetStatusDescription>,
    free_detailed_string: Option<NISysCfgFreeDetailedString>,
}

struct SysCfgLoaded {
    /// keeps the library mapped for the process lifetime
    _library: Library,
    api: SysCfgApi,
}

static SYSCFG: LibCache<SysCfgLoaded> = LibCache::new(LOAD_RETRY_INTERVAL);

fn syscfg() -> Result<&'static SysCfgLoaded, String> {
    SYSCFG.get_or_load(|| unsafe { load_once() })
}

unsafe fn load_once() -> Result<SysCfgLoaded, String> {
    let candidates = library_candidates(
        "nisyscfg.dll",
        &[
            "C:\\Program Files\\National Instruments\\Shared\\nisyscfg.dll",
            "C:\\Program Files (x86)\\National Instruments\\Shared\\nisyscfg.dll",
        ],
        &[
            "libnisyscfg.so",
            "libnisyscfg.so.1",
            "/usr/local/natinst/lib/libnisyscfg.so",
        ],
    );
    let library = load_first_library("NI System Configuration (nisyscfg)", &candidates)?;

    let api = SysCfgApi {
        initialize_session: required_symbol(&library, "NISysCfgInitializeSession")?,
        close_handle: required_symbol(&library, "NISysCfgCloseHandle")?,
        find_hardware: required_symbol(&library, "NISysCfgFindHardware")?,
        next_resource: required_symbol(&library, "NISysCfgNextResource")?,
        get_property: required_symbol(&library, "NISysCfgGetResourceProperty")?,
        get_indexed_property: required_symbol(&library, "NISysCfgGetResourceIndexedProperty")?,
        get_system_property: required_symbol(&library, "NISysCfgGetSystemProperty")?,
        get_status_description: optional_symbol(&library, "NISysCfgGetStatusDescription"),
        free_detailed_string: optional_symbol(&library, "NISysCfgFreeDetailedString"),
    };

    Ok(SysCfgLoaded {
        _library: library,
        api,
    })
}

/// `true` when an NI-SysCfg property read produced a usable value
/// (success or warning; warnings are logged at debug level).
fn property_ok(status: NISysCfgStatus, property_id: c_int) -> bool {
    if status > 0 {
        tracing::debug!("NI-SysCfg property {property_id}: warning status {status}");
    }
    status >= 0
}

/// NI System Configuration API wrapper
///
/// Cheap to obtain and `Copy`: the library and entry points are cached
/// process-wide.
#[derive(Clone, Copy)]
pub struct NiSysCfg {
    api: SysCfgApi,
}

impl NiSysCfg {
    /// Load the NI-SysCfg API (cached after the first successful call;
    /// failures are retried after 60 s)
    ///
    /// # Errors
    /// Returns an error if the library cannot be found or loaded, or a
    /// required symbol is missing.
    pub fn load() -> NimonResult<Self> {
        syscfg()
            .map(|loaded| NiSysCfg { api: loaded.api })
            .map_err(NimonError::Connection)
    }

    /// Check if NI-SysCfg is available on this system
    ///
    /// Actually attempts the (cached) load, so a present-but-broken
    /// installation reports `false`.
    pub fn is_available() -> bool {
        syscfg().is_ok()
    }

    /// Open a new NI-SysCfg session to the local system
    ///
    /// Blocks for up to the 10 s connect timeout. The returned session is
    /// reusable across sweeps and closes its handle when dropped.
    pub fn create_session(&self) -> NimonResult<SysCfgSession> {
        let handle = unsafe { open_raw_session(&self.api)? };
        Ok(SysCfgSession {
            api: self.api,
            handle: Mutex::new(RawSession(handle)),
        })
    }

    /// Human-readable description of an NI-SysCfg status code, if the
    /// installed driver exports NISysCfgGetStatusDescription.
    pub fn status_description(&self, status: i32) -> Option<String> {
        unsafe { status_description(&self.api, std::ptr::null_mut(), status) }
    }
}

unsafe fn status_description(
    api: &SysCfgApi,
    session: NISysCfgSessionHandle,
    status: NISysCfgStatus,
) -> Option<String> {
    let get = api.get_status_description?;
    let free = api.free_detailed_string?;
    let mut text: *mut c_char = std::ptr::null_mut();
    let st = get(session, status, &mut text);
    let result = if st >= 0 { c_str_to_string(text) } else { None };
    if !text.is_null() {
        free(text);
    }
    result.filter(|s| !s.is_empty())
}

unsafe fn open_raw_session(api: &SysCfgApi) -> NimonResult<NISysCfgSessionHandle> {
    let mut handle: NISysCfgSessionHandle = std::ptr::null_mut();
    let status = (api.initialize_session)(
        std::ptr::null(), // target: NULL => localhost
        std::ptr::null(), // username: NULL => no credentials
        std::ptr::null(), // password: NULL => no credentials
        NISYSCFG_LOCALE_DEFAULT,
        NISYSCFG_BOOL_FALSE, // TRUE here crashes NextResource on NI 26.3
        CONNECT_TIMEOUT_MS,
        std::ptr::null_mut(), // expert enum handle (optional)
        &mut handle,
    );
    if status < 0 || handle.is_null() {
        if !handle.is_null() {
            (api.close_handle)(handle);
        }
        if let Some(desc) = status_description(api, std::ptr::null_mut(), status) {
            tracing::warn!("NISysCfgInitializeSession failed: {desc} ({status})");
        }
        check_status("NISysCfgInitializeSession", status)?;
        return Err(NimonError::Connection(
            "NISysCfgInitializeSession returned a NULL session".into(),
        ));
    }
    check_status("NISysCfgInitializeSession", status)?; // logs warnings
    Ok(handle)
}

/// RAII guard for an NI-SysCfg enum/resource handle: closed exactly once
struct HandleGuard<'a> {
    api: &'a SysCfgApi,
    handle: *mut c_void,
}

impl<'a> HandleGuard<'a> {
    fn new(api: &'a SysCfgApi, handle: *mut c_void) -> Self {
        Self { api, handle }
    }
}

impl Drop for HandleGuard<'_> {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { (self.api.close_handle)(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

/// Typed property reads on one resource handle
struct Resource<'a>(HandleGuard<'a>);

impl Resource<'_> {
    fn string(&self, property_id: c_int) -> Option<String> {
        let mut buffer = [0 as c_char; NISYSCFG_SIMPLE_STRING_LENGTH];
        let status = unsafe {
            (self.0.api.get_property)(self.0.handle, property_id, buffer.as_mut_ptr().cast())
        };
        if !property_ok(status, property_id) {
            return None;
        }
        Some(c_buf_to_string(&buffer)).filter(|s| !s.is_empty())
    }

    fn int(&self, property_id: c_int) -> Option<c_int> {
        let mut value: c_int = 0;
        let status = unsafe {
            (self.0.api.get_property)(
                self.0.handle,
                property_id,
                (&mut value as *mut c_int).cast(),
            )
        };
        property_ok(status, property_id).then_some(value)
    }

    fn bool(&self, property_id: c_int) -> Option<bool> {
        self.int(property_id).map(|v| v != 0)
    }

    /// Finite doubles only: NaN/Inf readings never reach metrics
    fn f64(&self, property_id: c_int) -> Option<f64> {
        let mut value: f64 = 0.0;
        let status = unsafe {
            (self.0.api.get_property)(self.0.handle, property_id, (&mut value as *mut f64).cast())
        };
        property_ok(status, property_id)
            .then_some(value)
            .filter(|v| v.is_finite())
    }

    fn indexed_string(&self, property_id: c_int, index: u32) -> Option<String> {
        let mut buffer = [0 as c_char; NISYSCFG_SIMPLE_STRING_LENGTH];
        let status = unsafe {
            (self.0.api.get_indexed_property)(
                self.0.handle,
                property_id,
                index,
                buffer.as_mut_ptr().cast(),
            )
        };
        if !property_ok(status, property_id) {
            return None;
        }
        Some(c_buf_to_string(&buffer)).filter(|s| !s.is_empty())
    }

    fn indexed_f64(&self, property_id: c_int, index: u32) -> Option<f64> {
        let mut value: f64 = 0.0;
        let status = unsafe {
            (self.0.api.get_indexed_property)(
                self.0.handle,
                property_id,
                index,
                (&mut value as *mut f64).cast(),
            )
        };
        property_ok(status, property_id)
            .then_some(value)
            .filter(|v| v.is_finite())
    }

    /// Number of entries for an indexed property family, bounded
    fn count(&self, property_id: c_int) -> u32 {
        self.int(property_id).unwrap_or(0).clamp(0, MAX_INDEXED) as u32
    }

    fn identity(&self) -> ResourceIdentity {
        let mut id = ResourceIdentity {
            product: self.string(properties::PRODUCT_NAME).unwrap_or_default(),
            serial: self.string(properties::SERIAL_NUMBER).unwrap_or_default(),
            aliases: Vec::new(),
            names: Vec::new(),
        };
        // at least index 0: some resources don't report NumberOfExperts
        let experts = self.count(properties::NUMBER_OF_EXPERTS).max(1);
        for i in 0..experts {
            if let Some(a) = self.indexed_string(indexed_properties::EXPERT_USER_ALIAS, i) {
                id.aliases.push(a);
            }
            if let Some(n) = self.indexed_string(indexed_properties::EXPERT_RESOURCE_NAME, i) {
                id.names.push(n);
            }
            if let Some(n) = self.indexed_string(indexed_properties::EXPERT_NAME, i) {
                id.names.push(n);
            }
        }
        id
    }

    fn device_info(&self) -> NimonResult<DiscoveredDevice> {
        let product_name = self.string(properties::PRODUCT_NAME).ok_or_else(|| {
            NimonError::Connection("Failed to read PRODUCT_NAME from resource".to_string())
        })?;

        Ok(DiscoveredDevice {
            product_name,
            serial_number: self.string(properties::SERIAL_NUMBER).unwrap_or_default(),
            // NI MAX device name: the first expert's user alias. Index 0 is
            // kept deliberately: the edge derives stable device IDs from it.
            alias: self.indexed_string(indexed_properties::EXPERT_USER_ALIAS, 0),
            slot: self.int(properties::SLOT_NUMBER).filter(|s| *s >= 0),
            parent_link: self.string(properties::CONNECTS_TO_LINK_NAME),
            num_slots: self.int(properties::NUMBER_OF_SLOTS).filter(|s| *s >= 0),
            is_simulated: self.bool(properties::IS_SIMULATED).unwrap_or(false),
            ip_address: self.string(properties::TCP_IP_ADDRESS),
            is_reachable: self.is_present(),
            temperature: self.f64(properties::CURRENT_TEMP),
            firmware_version: self.string(properties::FIRMWARE_REVISION),
            // nisyscfg.h has no per-resource driver version (only HasDriver);
            // versions live in the software-component enumeration, which is
            // far too expensive for a polling sweep.
            driver_version: None,
            is_chassis: self.bool(properties::IS_CHASSIS).unwrap_or(false),
            resource_name: self.indexed_string(indexed_properties::EXPERT_RESOURCE_NAME, 0),
        })
    }

    fn is_present(&self) -> bool {
        self.int(properties::IS_PRESENT)
            .map(|present| present == NISYSCFG_IS_PRESENT_TYPE_PRESENT)
            .unwrap_or(false)
    }

    fn health(&self) -> DeviceHealth {
        if !self.is_present() {
            return DeviceHealth {
                is_reachable: false,
                temperature: None,
                sensors: Vec::new(),
                self_test_passed: None,
                error_message: Some("Device not reachable".to_string()),
                metrics: HashMap::new(),
            };
        }

        // Named temperature sensors (count, then name/reading/threshold
        // per index); non-finite readings are dropped by indexed_f64
        let mut sensors = Vec::new();
        for index in 0..self.count(properties::NUMBER_OF_TEMP_SENSORS) {
            let name = self.indexed_string(indexed_properties::TEMPERATURE_NAME, index);
            let reading = self.indexed_f64(indexed_properties::TEMPERATURE_READING, index);
            let upper = self.indexed_f64(indexed_properties::TEMPERATURE_UPPER_CRITICAL, index);
            if let (Some(name), Some(reading)) = (name, reading) {
                sensors.push(SensorReading {
                    name,
                    reading,
                    upper_critical: upper,
                });
            }
        }

        // Devices without a plain CURRENT_TEMP still get an aggregate
        // temperature: the hottest sensor
        let temperature = self
            .f64(properties::CURRENT_TEMP)
            .or_else(|| sensors.iter().map(|s| s.reading).reduce(f64::max));

        DeviceHealth {
            is_reachable: true,
            temperature,
            sensors,
            // NISysCfgSelfTestHardware *runs* a self-test (it can disturb
            // running tasks) and there is no cached-result property, so it
            // is not called on the polling path.
            self_test_passed: None,
            error_message: None,
            metrics: HashMap::new(),
        }
    }
}

/// Identifying strings of one resource, used to resolve a device name
#[derive(Debug, Clone, Default)]
struct ResourceIdentity {
    product: String,
    serial: String,
    /// NI MAX / expert user aliases (all experts)
    aliases: Vec<String>,
    /// Expert resource names and expert names (all experts)
    names: Vec<String>,
}

/// Resolve `wanted` to one resource, in priority order:
/// alias, then serial number, then expert resource/expert name, then
/// product name *only if exactly one resource has that product*.
/// Comparisons are trimmed and ASCII case-insensitive (NI MAX names are
/// case-insensitive).
fn find_device_index(ids: &[ResourceIdentity], wanted: &str) -> Option<usize> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return None;
    }
    let eq = |s: &str| !s.trim().is_empty() && s.trim().eq_ignore_ascii_case(wanted);

    ids.iter()
        .position(|id| id.aliases.iter().any(|a| eq(a)))
        .or_else(|| ids.iter().position(|id| eq(&id.serial)))
        .or_else(|| ids.iter().position(|id| id.names.iter().any(|n| eq(n))))
        .or_else(|| {
            let mut products = ids
                .iter()
                .enumerate()
                .filter(|(_, id)| eq(&id.product))
                .map(|(i, _)| i);
            match (products.next(), products.next()) {
                (Some(i), None) => Some(i),
                (Some(_), Some(_)) => {
                    tracing::debug!(
                        "Device name '{wanted}' matches several resources by product name; \
                         use the NI MAX alias or serial number to disambiguate"
                    );
                    None
                }
                _ => None,
            }
        })
}

/// Session handle, owned by the mutex in [`SysCfgSession`]
struct RawSession(NISysCfgSessionHandle);

// SAFETY: NI-SysCfg session handles are process-wide opaque handles that
// are not tied to the creating thread; access is serialised by the Mutex.
unsafe impl Send for RawSession {}

/// Reusable NI-SysCfg session (RAII: the handle is closed on drop)
///
/// Open it once (e.g. with [`SysCfgSession::open`]) and reuse it for every
/// sweep instead of paying NISysCfgInitializeSession on each poll. If an
/// enumeration fails the session is transparently closed, re-initialised,
/// and the enumeration retried once.
///
/// All methods block; see the module docs for the threading contract.
/// The type is `Send + Sync`; concurrent calls are serialised.
pub struct SysCfgSession {
    api: SysCfgApi,
    handle: Mutex<RawSession>,
}

impl SysCfgSession {
    /// Load NI-SysCfg (cached) and open a session to the local system
    pub fn open() -> NimonResult<Self> {
        NiSysCfg::load()?.create_session()
    }

    /// Whether the session currently holds an open handle (false after a
    /// failed re-initialisation; the next call retries)
    pub fn is_open(&self) -> bool {
        !self.lock().0.is_null()
    }

    /// Force the session to be closed and re-initialised
    pub fn reconnect(&self) -> NimonResult<()> {
        let mut guard = self.lock();
        self.reopen(&mut guard)
    }

    fn lock(&self) -> MutexGuard<'_, RawSession> {
        self.handle.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn close(&self, raw: &mut RawSession) {
        if !raw.0.is_null() {
            unsafe { (self.api.close_handle)(raw.0) };
            raw.0 = std::ptr::null_mut();
        }
    }

    fn reopen(&self, raw: &mut RawSession) -> NimonResult<()> {
        self.close(raw);
        raw.0 = unsafe { open_raw_session(&self.api)? };
        Ok(())
    }

    /// Enumerate every resource once. Handles collected so far are closed
    /// (exactly once, by their guards) on every error path.
    fn enumerate(&self, session: NISysCfgSessionHandle) -> NimonResult<Vec<Resource<'_>>> {
        let api = &self.api;
        let mut enum_handle: NISysCfgEnumResourceHandle = std::ptr::null_mut();
        let status = unsafe {
            (api.find_hardware)(
                session,
                NISYSCFG_FILTER_MODE_MATCH_VALUES_ALL, // ignored: filter is NULL
                std::ptr::null_mut(),                  // filter: NULL => all resources
                std::ptr::null(),                      // expert names: NULL => all experts
                &mut enum_handle,
            )
        };
        let enum_guard = HandleGuard::new(api, enum_handle);
        if status < 0 {
            if let Some(desc) = unsafe { status_description(api, session, status) } {
                tracing::debug!("NISysCfgFindHardware failed: {desc} ({status})");
            }
        }
        check_status("NISysCfgFindHardware", status)?;
        if enum_guard.handle.is_null() {
            return Ok(Vec::new());
        }

        let mut resources = Vec::new();
        while resources.len() < MAX_RESOURCES {
            let mut resource: NISysCfgResourceHandle = std::ptr::null_mut();
            let status = unsafe { (api.next_resource)(session, enum_guard.handle, &mut resource) };
            let guard = Resource(HandleGuard::new(api, resource));
            // Only negative is an error; positive statuses are warnings
            // (EndOfEnum = 1 among them) and must not truncate the list.
            check_status("NISysCfgNextResource", status)?;
            if resource.is_null() {
                break;
            }
            resources.push(guard);
            if status == NISYSCFG_END_OF_ENUM {
                break;
            }
        }
        Ok(resources)
        // enum_guard dropped here: enumeration handle closed
    }

    /// Run `f` over one full enumeration, reopening the session and
    /// retrying once if the session is closed or the enumeration fails.
    fn with_resources<R>(&self, f: impl FnOnce(&[Resource<'_>]) -> R) -> NimonResult<R> {
        let mut raw = self.lock();
        if raw.0.is_null() {
            self.reopen(&mut raw)?;
        }
        let resources = match self.enumerate(raw.0) {
            Ok(r) => r,
            Err(first) => {
                tracing::debug!("NI-SysCfg enumeration failed ({first}); reopening session");
                self.reopen(&mut raw)?;
                self.enumerate(raw.0)?
            }
        };
        let out = f(&resources);
        drop(resources); // close resource handles while the session is alive
        Ok(out)
    }

    /// Discover all NI devices on the system
    pub fn discover_devices(&self) -> NimonResult<Vec<DiscoveredDevice>> {
        self.with_resources(|resources| {
            resources
                .iter()
                .filter_map(|r| match r.device_info() {
                    Ok(d) => Some(d),
                    Err(e) => {
                        log_skipped_resource(&e);
                        None
                    }
                })
                .collect()
        })
    }

    /// Single-pass sweep: discover every device and its health with ONE
    /// enumeration of the hardware.
    pub fn discover_with_health(&self) -> NimonResult<Vec<(DiscoveredDevice, DeviceHealth)>> {
        self.with_resources(|resources| {
            resources
                .iter()
                .filter_map(|r| match r.device_info() {
                    Ok(d) => Some((d, r.health())),
                    Err(e) => {
                        log_skipped_resource(&e);
                        None
                    }
                })
                .collect()
        })
    }

    /// Get health information for a specific device
    ///
    /// `device_name` is resolved by NI MAX alias first, then serial
    /// number, then expert resource/expert name, then product name (only
    /// when exactly one resource has that product). Returns
    /// `DeviceHealth::unreachable()` if no resource matches.
    pub fn get_device_health(&self, device_name: &str) -> NimonResult<DeviceHealth> {
        self.with_resources(|resources| {
            let ids: Vec<ResourceIdentity> = resources.iter().map(|r| r.identity()).collect();
            match find_device_index(&ids, device_name) {
                Some(i) => resources[i].health(),
                None => {
                    tracing::debug!(
                        "Device '{device_name}' not found via NI-SysCfg, treating as unreachable"
                    );
                    DeviceHealth::unreachable()
                }
            }
        })
    }

    /// Station-level system information (hostname, OS, memory, disk).
    /// Plain session property reads, no enumeration. Returns empty info if
    /// the session cannot be (re)opened.
    pub fn system_info(&self) -> SystemInfo {
        let mut raw = self.lock();
        if raw.0.is_null() && self.reopen(&mut raw).is_err() {
            return SystemInfo::default();
        }
        let session = raw.0;
        let api = &self.api;
        let string = |id: c_int| -> Option<String> {
            let mut buffer = [0 as c_char; NISYSCFG_SIMPLE_STRING_LENGTH];
            let status =
                unsafe { (api.get_system_property)(session, id, buffer.as_mut_ptr().cast()) };
            if !property_ok(status, id) {
                return None;
            }
            Some(c_buf_to_string(&buffer)).filter(|s| !s.is_empty())
        };
        // doubles are in KB per nisyscfg.h; converted to MB, finite only
        let mb = |id: c_int| -> Option<f64> {
            let mut value: f64 = 0.0;
            let status =
                unsafe { (api.get_system_property)(session, id, (&mut value as *mut f64).cast()) };
            property_ok(status, id)
                .then_some(value / 1024.0)
                .filter(|v| v.is_finite())
        };
        SystemInfo {
            hostname: string(system_properties::HOSTNAME),
            product: string(system_properties::PRODUCT_NAME),
            operating_system: string(system_properties::OPERATING_SYSTEM),
            os_version: string(system_properties::OS_VERSION),
            serial_number: string(system_properties::SERIAL_NUMBER),
            memory_total_mb: mb(system_properties::MEMORY_PHYS_TOTAL),
            memory_free_mb: mb(system_properties::MEMORY_PHYS_FREE),
            disk_total_mb: mb(system_properties::PRIMARY_DISK_TOTAL),
            disk_free_mb: mb(system_properties::PRIMARY_DISK_FREE),
        }
    }

    /// Alias of [`SysCfgSession::system_info`] (original name)
    pub fn get_system_info(&self) -> SystemInfo {
        self.system_info()
    }
}

impl Drop for SysCfgSession {
    fn drop(&mut self) {
        let raw = self
            .handle
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        if !raw.0.is_null() {
            unsafe { (self.api.close_handle)(raw.0) };
            raw.0 = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(product: &str, serial: &str, aliases: &[&str], names: &[&str]) -> ResourceIdentity {
        ResourceIdentity {
            product: product.into(),
            serial: serial.into(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            names: names.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn sample() -> Vec<ResourceIdentity> {
        vec![
            id(
                "NI 9205",
                "01A2B3C4",
                &["cDAQ1Mod1"],
                &["cDAQ1Mod1", "daqmx"],
            ),
            id(
                "NI 9205",
                "01A2B3C5",
                &["AI_Rack2"],
                &["cDAQ1Mod2", "daqmx"],
            ),
            id("cDAQ-9178", "1234ABCD", &["cDAQ1"], &["cDAQ1", "daqmx"]),
            id("PXIe-4081", "", &[], &["PXI1Slot3", "nidmm"]),
        ]
    }

    #[test]
    fn test_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SysCfgSession>();
        assert_send_sync::<NiSysCfg>();
    }

    #[test]
    fn test_match_alias_first() {
        assert_eq!(find_device_index(&sample(), "AI_Rack2"), Some(1));
        assert_eq!(find_device_index(&sample(), "ai_rack2"), Some(1));
    }

    #[test]
    fn test_match_alias_beats_serial_and_names() {
        // "cDAQ1Mod1" is both resource 0's alias and name; alias wins
        let mut ids = sample();
        ids[3].serial = "cDAQ1".into(); // serial colliding with an alias
        assert_eq!(find_device_index(&ids, "cDAQ1"), Some(2));
    }

    #[test]
    fn test_match_serial() {
        assert_eq!(find_device_index(&sample(), "01A2B3C5"), Some(1));
    }

    #[test]
    fn test_match_resource_name() {
        assert_eq!(find_device_index(&sample(), "PXI1Slot3"), Some(3));
        assert_eq!(find_device_index(&sample(), "cDAQ1Mod2"), Some(1));
    }

    #[test]
    fn test_match_unique_product_only() {
        assert_eq!(find_device_index(&sample(), "cDAQ-9178"), Some(2));
        assert_eq!(find_device_index(&sample(), "PXIe-4081"), Some(3));
        // two NI 9205 modules: ambiguous, never silently the first one
        assert_eq!(find_device_index(&sample(), "NI 9205"), None);
    }

    #[test]
    fn test_match_empty_and_unknown() {
        assert_eq!(find_device_index(&sample(), ""), None);
        assert_eq!(find_device_index(&sample(), "  "), None);
        assert_eq!(find_device_index(&sample(), "nope"), None);
        // an empty serial must not match an empty-ish name
        assert_eq!(find_device_index(&[id("X", "", &[], &[])], " "), None);
    }

    #[test]
    fn test_property_ok_status_semantics() {
        assert!(property_ok(0, 1));
        assert!(property_ok(1, 1), "warnings keep the value");
        assert!(!property_ok(-2147220623, 1), "PropDoesNotExist is an error");
    }

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
