//! Raw FFI bindings to NI-VISA API
//!
//! This module contains the low-level C FFI definitions for the NI-VISA API.
//! NI-VISA uses visa64.dll (or visa32.dll as fallback) on Windows.

#![allow(dead_code)]

use std::os::raw::{c_char, c_int, c_long, c_ulong, c_void};

// ---------------------------------------------------------------------------
// Status constants
// ---------------------------------------------------------------------------

/// VISA success status code (VI_SUCCESS)
pub const VI_SUCCESS: c_long = 0;

/// Null end-of-termination indicator for viFindRsrc / viFindNext
pub const VI_NULL: *const c_char = std::ptr::null();

/// Maximum error message length
pub const VI_ERROR_DESCR_BUF_SIZE: usize = 256;

// ---------------------------------------------------------------------------
// Attribute IDs
// ---------------------------------------------------------------------------

pub mod attributes {
    use std::os::raw::c_int;

    /// Resource name attribute (ViAttr)
    pub const VI_ATTR_RSRC_NAME: c_int = 0xBFFFFFF1u32 as c_int;
    /// Timeout value in milliseconds
    pub const VI_ATTR_TMO_VALUE: c_int = 0x3FFF001A;
    /// Interface type (TCPIP, GPIB, ASRL, etc.)
    pub const VI_ATTR_INTF_TYPE: c_int = 0xBFFF0017u32 as c_int;
}

// ---------------------------------------------------------------------------
// Interface type constants (returned by VI_ATTR_INTF_TYPE)
// ---------------------------------------------------------------------------

pub mod intf {
    use std::os::raw::c_int;

    /// Unknown / no interface
    pub const VI_INTF_UNKNOWN: c_int = 0;
    /// GPIB interface
    pub const VI_INTF_GPIB: c_int = 1;
    /// VXI interface
    pub const VI_INTF_VXI: c_int = 2;
    /// GPIB-VXI interface
    pub const VI_INTF_GPIB_VXI: c_int = 3;
    /// Serial (ASRL) interface
    pub const VI_INTF_ASRL: c_int = 4;
    /// PXI interface
    pub const VI_INTF_PXI: c_int = 5;
    /// TCPIP (LAN) interface
    pub const VI_INTF_TCPIP: c_int = 6;
    /// USB interface
    pub const VI_INTF_USB: c_int = 7;
}

// ---------------------------------------------------------------------------
// Opaque handles
// ---------------------------------------------------------------------------

/// Opaque handle to a VISA session (resource manager or instrument)
#[repr(C)]
pub struct ViSession {
    _private: [u8; 0],
}

/// Opaque handle to a VISA object (used for find lists)
#[repr(C)]
pub struct ViObject {
    _private: [u8; 0],
}

// ---------------------------------------------------------------------------
// Function pointer types
// ---------------------------------------------------------------------------

/// Function signature for viOpenDefaultRM
///
/// Opens a session to the default resource manager.
pub type ViOpenDefaultRM = unsafe extern "C" fn(sesn: *mut *mut ViSession) -> c_long;

/// Function signature for viClose
///
/// Closes the specified session, object, or find list.
pub type ViClose = unsafe extern "C" fn(vi: *mut ViSession) -> c_long;

/// Function signature for viFindRsrc
///
/// Queries the system for VISA resources matching the expression.
pub type ViFindRsrc = unsafe extern "C" fn(
    sesn: *mut ViSession,
    expr: *const c_char,
    find_list: *mut *mut ViObject,
    retcnt: *mut c_ulong,
    desc: *mut c_char,
) -> c_long;

/// Function signature for viFindNext
///
/// Returns the next resource in the find list.
pub type ViFindNext = unsafe extern "C" fn(find_list: *mut ViObject, desc: *mut c_char) -> c_long;

/// Function signature for viOpen
///
/// Opens a session to the specified resource.
pub type ViOpen = unsafe extern "C" fn(
    sesn: *mut ViSession,
    rsrc_name: *const c_char,
    access_mode: c_ulong,
    timeout: c_ulong,
    vi: *mut *mut ViSession,
) -> c_long;

/// Function signature for viWrite
///
/// Writes data to the specified resource synchronously.
pub type ViWrite = unsafe extern "C" fn(
    vi: *mut ViSession,
    buf: *const u8,
    count: c_ulong,
    ret_count: *mut c_ulong,
) -> c_long;

/// Function signature for viRead
///
/// Reads data from the specified resource synchronously.
pub type ViRead = unsafe extern "C" fn(
    vi: *mut ViSession,
    buf: *mut u8,
    count: c_ulong,
    ret_count: *mut c_ulong,
) -> c_long;

/// Function signature for viGetAttribute
///
/// Retrieves the value of an attribute for the specified session or object.
pub type ViGetAttribute =
    unsafe extern "C" fn(vi: *mut ViSession, attr: c_int, attr_state: *mut c_void) -> c_long;
