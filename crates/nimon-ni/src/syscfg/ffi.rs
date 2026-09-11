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
#[allow(dead_code)] // complete FFI surface; not every value is read yet
pub const NISYSCFG_BOOL_TRUE: c_int = 1;

/// NISysCfgIsPresentType values
pub const NISYSCFG_IS_PRESENT_TYPE_PRESENT: c_int = 1;

/// Required size for buffers that receive string property values
pub const NISYSCFG_SIMPLE_STRING_LENGTH: usize = 1024;

/// Property IDs for GetResourceProperty (NISysCfgResourceProperty)
#[allow(dead_code)] // complete FFI property surface; not every property is read yet
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
    pub const NUMBER_OF_SLOTS: c_int = 16826368; // int
    pub const IS_PRESENT: c_int = 16924672; // NISysCfgIsPresentType
    pub const CURRENT_TEMP: c_int = 16965632; // double
    pub const TCP_IP_ADDRESS: c_int = 16957440; // char *
    pub const MODEL_NAME_NUMBER: c_int = 17436672; // unsigned int
    pub const NUMBER_OF_TEMP_SENSORS: c_int = 17186816; // int
    /// GUID of the chassis/bus a module is plugged into (matches the
    /// chassis resource's CONNECTS_TO / resource GUID)
    pub const CONNECTS_TO_LINK_NAME: c_int = 16818176; // char *
}

/// Property IDs for GetResourceIndexedProperty (NISysCfgIndexedProperty)
#[allow(dead_code)] // complete FFI property surface
pub mod indexed_properties {
    use std::os::raw::c_int;

    pub const EXPERT_NAME: c_int = 16900096; // char *
    pub const EXPERT_RESOURCE_NAME: c_int = 16896000; // char *
    /// The NI MAX device name (DAQmx alias), survives user renames
    pub const EXPERT_USER_ALIAS: c_int = 16904192; // char *
    pub const TEMPERATURE_NAME: c_int = 17190912; // char *
    pub const TEMPERATURE_READING: c_int = 16965632; // double
    pub const TEMPERATURE_UPPER_CRITICAL: c_int = 17199104; // double
}

/// Property IDs for GetSystemProperty (NISysCfgSystemProperty)
#[allow(dead_code)] // complete FFI property surface; not every property is read yet
pub mod system_properties {
    use std::os::raw::c_int;

    pub const HOSTNAME: c_int = 16941063; // char *
    pub const IP_ADDRESS: c_int = 16941064; // char *
    pub const MAC_ADDRESS: c_int = 16941077; // char *
    pub const PRODUCT_NAME: c_int = 16941078; // char *
    pub const OPERATING_SYSTEM: c_int = 16941079; // char *
    pub const OS_VERSION: c_int = 17100800; // char *
    pub const SERIAL_NUMBER: c_int = 16941080; // char *
    pub const INSTALLED_API_VERSION: c_int = 16941087; // char *
    pub const MEMORY_PHYS_TOTAL: c_int = 219480064; // double (KB)
    pub const MEMORY_PHYS_FREE: c_int = 219484160; // double (KB)
    pub const PRIMARY_DISK_TOTAL: c_int = 219291648; // double (KB)
    pub const PRIMARY_DISK_FREE: c_int = 219295744; // double (KB)
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

/// NISysCfgGetResourceIndexedProperty
pub type NISysCfgGetResourceIndexedProperty = unsafe extern "C" fn(
    resource_handle: *mut NiSysCfgResource,
    property_id: c_int,
    index: u32,
    value: *mut c_void,
) -> c_int;

/// NISysCfgGetSystemProperty
pub type NISysCfgGetSystemProperty = unsafe extern "C" fn(
    session_handle: *mut NiSysCfgSession,
    property_id: c_int,
    value: *mut c_void,
) -> c_int;
