//! Common utilities for NI API FFI bindings
//!
//! This module provides helper functions for working with NI C APIs via FFI:
//! status classification, bounded/lossy C string decoding, and a
//! process-wide library cache with a retry policy for load failures.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use libloading::Library;
use tracing::{debug, warn};

/// Upper bound when scanning an unbounded C string pointer for its NUL.
/// NI strings are device names, aliases, and error descriptions: 64 KiB is
/// far beyond anything legitimate and stops a runaway read on a missing
/// terminator.
pub const MAX_C_STRING_LEN: usize = 64 * 1024;

/// How long a failed library load is remembered before it is retried.
/// Lets NI driver installs/repairs take effect without restarting the edge.
pub const LOAD_RETRY_INTERVAL: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// Decode NUL-terminated bytes lossily: invalid UTF-8 (e.g. Windows ANSI
/// codepage device names) becomes U+FFFD instead of dropping the string.
/// If no NUL is present the whole slice is used.
pub fn decode_c_bytes(bytes: &[u8]) -> String {
    match CStr::from_bytes_until_nul(bytes) {
        Ok(cstr) => cstr.to_string_lossy().into_owned(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Decode a fixed-size C character buffer (as filled by an NI API) lossily,
/// never reading past the end of the buffer.
pub fn c_buf_to_string(buf: &[c_char]) -> String {
    // SAFETY: c_char and u8 have identical size and alignment.
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, buf.len()) };
    decode_c_bytes(bytes)
}

/// Convert a C string pointer to a Rust String (lossy UTF-8)
///
/// Reads at most [`MAX_C_STRING_LEN`] bytes looking for the terminator.
/// Returns `None` only for a NULL pointer.
///
/// # Safety
/// The pointer must be NULL or point to readable memory that is either
/// NUL-terminated or at least `MAX_C_STRING_LEN` bytes long.
pub unsafe fn c_str_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let base = ptr as *const u8;
    let mut len = 0usize;
    while len < MAX_C_STRING_LEN && *base.add(len) != 0 {
        len += 1;
    }
    let bytes = std::slice::from_raw_parts(base, len);
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// Convert a C string pointer to a String, returning empty string for NULL
///
/// # Safety
/// Same contract as [`c_str_to_string`].
pub unsafe fn c_str_to_string_or_empty(ptr: *const c_char) -> String {
    c_str_to_string(ptr).unwrap_or_default()
}

/// Convert a Rust string to a C string
///
/// Returns None if the string contains null bytes
pub fn string_to_c_string(s: &str) -> Option<CString> {
    CString::new(s).ok()
}

// ---------------------------------------------------------------------------
// Status codes
// ---------------------------------------------------------------------------

/// Classification of an NI status code.
///
/// NI-SysCfg, NI-DAQmx and NI-VISA all use the same convention:
/// negative = error, zero = success, positive = warning (the call
/// succeeded and its outputs are valid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Success,
    Warning,
    Error,
}

/// Classify an NI status code (see [`StatusKind`])
pub fn classify_status(status: i32) -> StatusKind {
    match status {
        0 => StatusKind::Success,
        s if s > 0 => StatusKind::Warning,
        _ => StatusKind::Error,
    }
}

/// Check NI API status code and convert to Result
///
/// Only negative codes are errors. Positive codes are warnings: they are
/// logged at debug level and treated as success.
pub fn check_status(api_name: &'static str, status: i32) -> crate::NimonResult<()> {
    match classify_status(status) {
        StatusKind::Success => Ok(()),
        StatusKind::Warning => {
            debug!("NI API warning in {api_name}: status {status}");
            Ok(())
        }
        StatusKind::Error => Err(crate::NimonError::NiApi {
            api: api_name,
            code: status,
        }),
    }
}

/// Check NI API status code with error message lookup
///
/// Same semantics as [`check_status`]; on error the provided lookup is used
/// to log a human-readable message.
pub fn check_status_with_message<F>(
    api_name: &'static str,
    status: i32,
    get_error_msg: F,
) -> crate::NimonResult<()>
where
    F: FnOnce(i32) -> Option<String>,
{
    let result = check_status(api_name, status);
    if result.is_err() {
        if let Some(msg) = get_error_msg(status) {
            warn!("NI API error in {}: {} ({})", api_name, msg, status);
        }
    }
    result
}

/// Helper to write a string to a C buffer
///
/// Returns the number of bytes written (excluding null terminator)
///
/// # Safety
/// - buffer must be valid for writes up to buffer_size bytes
pub unsafe fn write_string_to_buffer(s: &str, buffer: *mut c_char, buffer_size: usize) -> usize {
    if buffer.is_null() || buffer_size == 0 {
        return 0;
    }

    let bytes = s.as_bytes();
    let copy_len = bytes.len().min(buffer_size - 1); // Leave room for null terminator

    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer as *mut u8, copy_len);
    *buffer.add(copy_len) = 0; // Null terminator

    copy_len
}

// ---------------------------------------------------------------------------
// Library loading
// ---------------------------------------------------------------------------

/// Process-wide cache for a dynamically loaded NI library.
///
/// Success is cached forever (the DLL stays loaded for the process
/// lifetime, so copied function pointers stay valid). A failure is cached
/// only for `retry` and then the load is attempted again.
pub(crate) struct LibCache<T: 'static> {
    ok: OnceLock<T>,
    failure: Mutex<Option<(Instant, String)>>,
    retry: Duration,
}

impl<T: Send + Sync + 'static> LibCache<T> {
    pub(crate) const fn new(retry: Duration) -> Self {
        Self {
            ok: OnceLock::new(),
            failure: Mutex::new(None),
            retry,
        }
    }

    /// Return the cached value, or run `load` (at most one thread at a
    /// time) if nothing is cached or the cached failure has expired.
    pub(crate) fn get_or_load<F>(&'static self, load: F) -> Result<&'static T, String>
    where
        F: FnOnce() -> Result<T, String>,
    {
        if let Some(v) = self.ok.get() {
            return Ok(v);
        }
        let mut failure = self.failure.lock().unwrap_or_else(PoisonError::into_inner);
        // another thread may have loaded it while we waited for the lock
        if let Some(v) = self.ok.get() {
            return Ok(v);
        }
        if let Some((at, msg)) = failure.as_ref() {
            if at.elapsed() < self.retry {
                return Err(msg.clone());
            }
        }
        match load() {
            Ok(v) => {
                *failure = None;
                // cannot already be set: only set under this lock
                let _ = self.ok.set(v);
                self.ok
                    .get()
                    .ok_or_else(|| "library cache initialisation failed".to_string())
            }
            Err(e) => {
                *failure = Some((Instant::now(), e.clone()));
                Err(e)
            }
        }
    }
}

/// Platform-specific search list for an NI shared library.
///
/// * Windows: `%SystemRoot%\System32\<dll>` first (avoids picking up a
///   planted DLL from the working directory), then `extra_windows` absolute
///   paths, then the bare name for the standard search order.
/// * Unix (NI Linux RT / desktop Linux): the `unix` names, resolved by the
///   dynamic linker (absolute paths may be included).
/// * Anything else: empty (the loader reports "unsupported platform").
#[cfg(windows)]
pub(crate) fn library_candidates(
    windows_dll: &str,
    extra_windows: &[&str],
    _unix: &[&str],
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    out.push(PathBuf::from(root).join("System32").join(windows_dll));
    out.extend(extra_windows.iter().map(PathBuf::from));
    out.push(PathBuf::from(windows_dll));
    out
}

/// See the Windows variant for the search policy.
#[cfg(unix)]
pub(crate) fn library_candidates(
    _windows_dll: &str,
    _extra_windows: &[&str],
    unix: &[&str],
) -> Vec<PathBuf> {
    unix.iter().map(PathBuf::from).collect()
}

/// See the Windows variant for the search policy.
#[cfg(not(any(windows, unix)))]
pub(crate) fn library_candidates(
    _windows_dll: &str,
    _extra_windows: &[&str],
    _unix: &[&str],
) -> Vec<PathBuf> {
    Vec::new()
}

/// Load the first library in `candidates` that loads successfully.
///
/// # Safety
/// Loading a library runs its initialisation routines.
pub(crate) unsafe fn load_first_library(
    what: &str,
    candidates: &[PathBuf],
) -> Result<Library, String> {
    if candidates.is_empty() {
        return Err(format!("{what} is not supported on this platform"));
    }
    let mut errors = Vec::new();
    for path in candidates {
        // skip absolute paths that don't exist (cheap, clearer errors)
        if path.is_absolute() && !path.exists() {
            continue;
        }
        match Library::new(path) {
            Ok(lib) => {
                debug!("Loaded {what} from {}", path.display());
                return Ok(lib);
            }
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    if errors.is_empty() {
        Err(format!("{what} not found. Please install the NI driver."))
    } else {
        Err(format!("Failed to load {what}: {}", errors.join("; ")))
    }
}

/// Resolve a required symbol and copy the function pointer out.
///
/// # Safety
/// `T` must be the exact function pointer type of the exported symbol, and
/// the library must outlive every use of the returned pointer.
pub(crate) unsafe fn required_symbol<T: Copy>(lib: &Library, name: &str) -> Result<T, String> {
    lib.get::<T>(name.as_bytes()).map(|s| *s).map_err(|e| {
        format!("Required symbol {name} not found ({e}); incompatible NI driver version")
    })
}

/// Resolve an optional symbol; `None` if the installed driver lacks it.
///
/// # Safety
/// Same as [`required_symbol`].
pub(crate) unsafe fn optional_symbol<T: Copy>(lib: &Library, name: &str) -> Option<T> {
    match lib.get::<T>(name.as_bytes()) {
        Ok(s) => Some(*s),
        Err(_) => {
            debug!("Optional NI symbol {name} not exported by the installed driver");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_string_to_c_string() {
        let cstr = string_to_c_string("hello");
        assert!(cstr.is_some());
        assert_eq!(cstr.unwrap().as_bytes(), b"hello");
    }

    #[test]
    fn test_string_to_c_string_with_null() {
        let cstr = string_to_c_string("hello\0world");
        assert!(
            cstr.is_none(),
            "Should return None for strings with embedded nulls"
        );
    }

    #[test]
    fn test_check_status_success() {
        assert!(check_status("Test", 0).is_ok());
    }

    #[test]
    fn test_check_status_warning_is_success() {
        // positive = warning (e.g. NISysCfg_EndOfEnum = 1, VI_SUCCESS_MAX_CNT)
        assert!(check_status("Test", 1).is_ok());
        assert!(check_status("Test", 0x3FFF0006).is_ok());
    }

    #[test]
    fn test_check_status_error() {
        let err = check_status("Test", -1).unwrap_err();
        assert!(matches!(
            err,
            crate::NimonError::NiApi {
                api: "Test",
                code: -1
            }
        ));
    }

    #[test]
    fn test_classify_status() {
        assert_eq!(classify_status(0), StatusKind::Success);
        assert_eq!(classify_status(263168), StatusKind::Warning);
        assert_eq!(classify_status(-200220), StatusKind::Error);
        assert_eq!(classify_status(i32::MIN), StatusKind::Error);
    }

    #[test]
    fn test_check_status_with_message_warning_ok() {
        let mut called = false;
        let r = check_status_with_message("Test", 5, |_| {
            called = true;
            None
        });
        assert!(r.is_ok());
        assert!(!called, "message lookup only runs on errors");
    }

    #[test]
    fn test_c_str_to_string_null() {
        let result = unsafe { c_str_to_string(std::ptr::null()) };
        assert!(result.is_none());
    }

    #[test]
    fn test_c_str_to_string_valid() {
        let cstr = CString::new("test string").unwrap();
        let result = unsafe { c_str_to_string(cstr.as_ptr()) };
        assert_eq!(result, Some("test string".to_string()));
    }

    #[test]
    fn test_c_str_to_string_non_utf8_is_lossy_not_dropped() {
        // "Gerät" in Windows-1252: 0xE4 is not valid UTF-8 on its own
        let bytes: &[u8] = b"Ger\xE4t\0";
        let result = unsafe { c_str_to_string(bytes.as_ptr() as *const c_char) };
        assert_eq!(result, Some("Ger\u{FFFD}t".to_string()));
    }

    #[test]
    fn test_c_str_to_string_or_empty_null() {
        let result = unsafe { c_str_to_string_or_empty(std::ptr::null()) };
        assert_eq!(result, "");
    }

    #[test]
    fn test_decode_c_bytes_stops_at_nul() {
        assert_eq!(decode_c_bytes(b"Dev1\0garbage"), "Dev1");
    }

    #[test]
    fn test_decode_c_bytes_unterminated_is_bounded() {
        assert_eq!(decode_c_bytes(b"abc"), "abc");
    }

    #[test]
    fn test_c_buf_to_string() {
        let mut buf = [0 as c_char; 8];
        for (i, b) in b"PXI1\xFF".iter().enumerate() {
            buf[i] = *b as c_char;
        }
        assert_eq!(c_buf_to_string(&buf), "PXI1\u{FFFD}");
        // completely full buffer without terminator does not over-read
        let full = [b'A' as c_char; 4];
        assert_eq!(c_buf_to_string(&full), "AAAA");
    }

    #[test]
    fn test_write_string_to_buffer_truncates() {
        let mut buf = [1 as c_char; 4];
        let n = unsafe { write_string_to_buffer("hello", buf.as_mut_ptr(), buf.len()) };
        assert_eq!(n, 3);
        assert_eq!(c_buf_to_string(&buf), "hel");
    }

    #[test]
    fn test_lib_cache_caches_failure_until_retry() {
        static CACHE: LibCache<u32> = LibCache::new(Duration::from_secs(3600));
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let load = || {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Err::<u32, _>("missing".to_string())
        };
        assert_eq!(CACHE.get_or_load(load).unwrap_err(), "missing");
        assert_eq!(CACHE.get_or_load(load).unwrap_err(), "missing");
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "failure is cached");
    }

    #[test]
    fn test_lib_cache_retries_after_interval_and_caches_success() {
        static CACHE: LibCache<u32> = LibCache::new(Duration::ZERO);
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        assert!(CACHE
            .get_or_load(|| {
                CALLS.fetch_add(1, Ordering::SeqCst);
                Err("not installed yet".to_string())
            })
            .is_err());
        // "driver installed": retry allowed immediately with a zero interval
        assert_eq!(
            *CACHE
                .get_or_load(|| {
                    CALLS.fetch_add(1, Ordering::SeqCst);
                    Ok(7)
                })
                .unwrap(),
            7
        );
        // success cached: loader not called again
        assert_eq!(
            *CACHE
                .get_or_load(|| {
                    CALLS.fetch_add(1, Ordering::SeqCst);
                    Ok(8)
                })
                .unwrap(),
            7
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_load_first_library_empty_is_unsupported() {
        let err = unsafe { load_first_library("libfoo", &[]) }.unwrap_err();
        assert!(err.contains("not supported"));
    }

    #[test]
    fn test_load_first_library_missing_absolute_paths() {
        let missing = std::env::temp_dir().join("definitely-not-an-ni-driver-5f3a.dll");
        let err = unsafe { load_first_library("NI test", &[missing]) }.unwrap_err();
        assert!(err.contains("not found"));
    }
}
