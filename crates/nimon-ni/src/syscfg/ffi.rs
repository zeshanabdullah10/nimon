//! Raw FFI bindings to NI System Configuration API
//!
//! This module contains the low-level C FFI definitions for the NI-SysCfg API.

use std::os::raw::{c_char, c_int, c_void};

/// Search mode for FindHardware
pub const NISYSCFG_SIMPLE_SEARCH: c_int = 0;

/// Property IDs for GetResourceProperty
pub mod properties {
    use std::os::raw::c_int;

    pub const PRODUCT_NAME: c_int = 0;
    pub const SERIAL_NUMBER: c_int = 1;
    pub const IPADDRESS: c_int = 3;
    pub const IS_REACHABLE: c_int = 7;
    pub const TEMPERATURE: c_int = 100;
    pub const FIRMWARE_REVISION: c_int = 200;
    pub const DRIVER_VERSION: c_int = 201;
}

/// Opaque handle to NI-SysCfg session
#[repr(C)]
pub struct NiSysCfgSession {
    _private: [u8; 0],
}

/// Opaque handle to hardware enumeration
#[repr(C)]
pub struct NiSysCfgEnum {
    _private: [u8; 0],
}

/// Opaque handle to a hardware resource
#[repr(C)]
pub struct NiSysCfgResource {
    _private: [u8; 0],
}

/// Function signature for NiSysCfg_Initialize
pub type NiSysCfgInitialize = unsafe extern "C" fn(
    hostname: *const c_char,
    username: *const c_char,
    password: *const c_char,
    session: *mut *mut NiSysCfgSession,
) -> c_int;

/// Function signature for NiSysCfg_CloseHandle
pub type NiSysCfgCloseHandle = unsafe extern "C" fn(
    handle: *mut c_void,
) -> c_int;

/// Function signature for NiSysCfg_FindHardware
pub type NiSysCfgFindHardware = unsafe extern "C" fn(
    session: *mut NiSysCfgSession,
    mode: c_int,
    filter: *const c_char,
    enum_handle: *mut *mut NiSysCfgEnum,
) -> c_int;

/// Function signature for NiSysCfg_NextResource
pub type NiSysCfgNextResource = unsafe extern "C" fn(
    session: *mut NiSysCfgSession,
    enum_handle: *mut NiSysCfgEnum,
    resource: *mut *mut NiSysCfgResource,
) -> c_int;

/// Function signature for NiSysCfg_GetResourceProperty
pub type NiSysCfgGetResourceProperty = unsafe extern "C" fn(
    resource: *mut NiSysCfgResource,
    property_id: c_int,
    property_value: *mut c_void,
) -> c_int;
