//! Raw FFI bindings to NI System Configuration API
//!
//! This module contains the low-level C FFI definitions for the NI-SysCfg API,
//! matching the signatures in the official nisyscfg.h shipped with the
//! installed NI System Configuration version.

use std::os::raw::{c_char, c_int, c_void};

/// NISysCfgFilterMode (ignored when the filter handle is NULL)
pub const NISYSCFG_FILTER_MODE_MATCH_VALUES_ALL: c_int = 1;

/// NISysCfgLocale: LCID or 0 for default
pub const NISYSCFG_LOCALE_DEFAULT: c_int = 0;

/// NISysCfgBool values
pub const NISYSCFG_BOOL_FALSE: c_int = 0;
pub const NISYSCFG_BOOL_TRUE: c_int = 1;

/// NISysCfgIsPresentType values
pub const NISYSCFG_IS_PRESENT_TYPE_PRESENT: c_int = 1;

/// Required size for buffers that receive string property values
pub const NISYSCFG_SIMPLE_STRING_LENGTH: usize = 1024;

/// Property IDs for GetResourceProperty (NISysCfgResourceProperty)
pub mod properties {
    use std::os::raw::c_int;

    pub const IS_DEVICE: c_int = 16781312; // NISysCfgBool
    pub const IS_CHASSIS: c_int = 16941056; // NISysCfgBool
    pub const VENDOR_NAME: c_int = 16793600; // char *
    pub const PRODUCT_NAME: c_int = 16801792; // char *
    pub const SERIAL_NUMBER: c_int = 16805888; // char *
    pub const FIRMWARE_REVISION: c_int = 16969728; // char *
    pub const IS_SIMULATED: c_int = 16814080; // NISysCfgBool
    pub const SLOT_NUMBER: c_int = 16822272; // int
    pub const IS_PRESENT: c_int = 16924672; // NISysCfgIsPresentType
    pub const CURRENT_TEMP: c_int = 16965632; // double
    pub const TCP_IP_ADDRESS: c_int = 16957440; // char *
    pub const MODEL_NAME_NUMBER: c_int = 17436672; // unsigned int
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

/// NISysCfgInitializeSession
pub type NISysCfgInitializeSession = unsafe extern "C" fn(
    target: *const c_char,
    username: *const c_char,
    password: *const c_char,
    language: c_int,
    force_property_refresh: c_int,
    connect_timeout_msec: u32,
    expert_enum_handle: *mut *mut NiSysCfgEnum,
    session_handle: *mut *mut NiSysCfgSession,
) -> c_int;

/// NISysCfgCloseHandle (works on session, enum, and resource handles)
pub type NISysCfgCloseHandle = unsafe extern "C" fn(handle: *mut c_void) -> c_int;

/// NISysCfgFindHardware
pub type NISysCfgFindHardware = unsafe extern "C" fn(
    session_handle: *mut NiSysCfgSession,
    filter_mode: c_int,
    filter_handle: *mut NiSysCfgEnum,
    expert_names: *const c_char,
    resource_enum_handle: *mut *mut NiSysCfgEnum,
) -> c_int;

/// NISysCfgNextResource
pub type NISysCfgNextResource = unsafe extern "C" fn(
    session_handle: *mut NiSysCfgSession,
    resource_enum_handle: *mut NiSysCfgEnum,
    resource_handle: *mut *mut NiSysCfgResource,
) -> c_int;

/// NISysCfgGetResourceProperty
pub type NISysCfgGetResourceProperty = unsafe extern "C" fn(
    resource_handle: *mut NiSysCfgResource,
    property_id: c_int,
    value: *mut c_void,
) -> c_int;
