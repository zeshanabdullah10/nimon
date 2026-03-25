//! Common utilities for NI API FFI bindings
//!
//! This module provides helper functions for working with NI C APIs via FFI.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use tracing::warn;

/// Convert a C string pointer to a Rust String
///
/// # Safety
/// The pointer must be valid and point to a null-terminated string
pub unsafe fn c_str_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    CStr::from_ptr(ptr)
        .to_str()
        .ok()
        .map(|s| s.to_owned())
}

/// Convert a C string pointer to a String, returning empty string on error
///
/// # Safety
/// The pointer must be valid and point to a null-terminated string
pub unsafe fn c_str_to_string_or_empty(ptr: *const c_char) -> String {
    c_str_to_string(ptr).unwrap_or_default()
}

/// Convert a Rust string to a C string
///
/// Returns None if the string contains null bytes
pub fn string_to_c_string(s: &str) -> Option<CString> {
    CString::new(s).ok()
}

/// Check NI API status code and convert to Result
///
/// NI APIs return 0 for success, non-zero for errors
pub fn check_status(api_name: &'static str, status: i32) -> crate::NimonResult<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(crate::NimonError::NiApi {
            api: api_name,
            code: status,
        })
    }
}

/// Check NI API status code with error message lookup
///
/// Uses the provided error lookup function to get a human-readable message
pub fn check_status_with_message<F>(
    api_name: &'static str,
    status: i32,
    get_error_msg: F,
) -> crate::NimonResult<()>
where
    F: FnOnce(i32) -> Option<String>,
{
    if status == 0 {
        Ok(())
    } else {
        let error_msg = get_error_msg(status);
        if let Some(msg) = error_msg {
            warn!("NI API error in {}: {} ({})", api_name, msg, status);
        }
        Err(crate::NimonError::NiApi {
            api: api_name,
            code: status,
        })
    }
}

/// Helper to write a string to a C buffer
///
/// Returns the number of bytes written (excluding null terminator)
///
/// # Safety
/// - buffer must be valid for writes up to buffer_size bytes
/// - buffer_size must be large enough for the string plus null terminator
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_string_to_c_string() {
        let cstr = string_to_c_string("hello");
        assert!(cstr.is_some());
        assert_eq!(cstr.unwrap().as_bytes(), b"hello");
    }

    #[test]
    fn test_string_to_c_string_with_null() {
        let cstr = string_to_c_string("hello\0world");
        assert!(cstr.is_none(), "Should return None for strings with embedded nulls");
    }

    #[test]
    fn test_check_status_success() {
        let result = check_status("Test", 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_status_error() {
        let result = check_status("Test", -1);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, crate::NimonError::NiApi { api: "Test", code: -1 }));
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
    fn test_c_str_to_string_or_empty_null() {
        let result = unsafe { c_str_to_string_or_empty(std::ptr::null()) };
        assert_eq!(result, "");
    }
}
