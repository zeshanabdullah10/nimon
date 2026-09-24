//! Raw FFI bindings to NI System Configuration API
//!
//! Verified against nisyscfg.h / nisyscfg_errors.h (NI System Configuration
//! 2026, `Shared\ExternalCompilerSupport\C\include`). Every public function
//! is `NISysCfgStatus NISYSCFGCONV`, where NISYSCFGCONV is `__stdcall` on
//! Windows and empty elsewhere, i.e. exactly Rust's `extern "system"`.

use std::os::raw::{c_char, c_int, c_uint, c_void};

/// NISysCfgStatus: signed 32-bit; < 0 error, 0 OK, > 0 warning
pub type NISysCfgStatus = i32;

/// All NI-SysCfg handles are `typedef void *`
pub type NISysCfgSessionHandle = *mut c_void;
pub type NISysCfgResourceHandle = *mut c_void;
pub type NISysCfgFilterHandle = *mut c_void;
pub type NISysCfgEnumResourceHandle = *mut c_void;
pub type NISysCfgEnumExpertHandle = *mut c_void;

/// NISysCfgFilterMode (ignored when the filter handle is NULL)
pub const NISYSCFG_FILTER_MODE_MATCH_VALUES_ALL: c_int = 1;

/// NISysCfgLocale: LCID or 0 for default
pub const NISYSCFG_LOCALE_DEFAULT: c_int = 0;

/// NISysCfgBool values
pub const NISYSCFG_BOOL_FALSE: c_int = 0;

/// NISysCfgIsPresentType values
pub const NISYSCFG_IS_PRESENT_TYPE_PRESENT: c_int = 1;

/// NISysCfg_EndOfEnum: returned (as a warning) by NISysCfgNext* at the end
pub const NISYSCFG_END_OF_ENUM: NISysCfgStatus = 1;

/// Required size for buffers that receive string property values
pub const NISYSCFG_SIMPLE_STRING_LENGTH: usize = 1024;

/// Property IDs for GetResourceProperty (NISysCfgResourceProperty)
pub mod properties {
    use std::os::raw::c_int;

    pub const IS_CHASSIS: c_int = 16941056; // NISysCfgBool
    pub const PRODUCT_NAME: c_int = 16801792; // char *
    pub const SERIAL_NUMBER: c_int = 16805888; // char *
    pub const FIRMWARE_REVISION: c_int = 16969728; // char *
    pub const IS_SIMULATED: c_int = 16814080; // NISysCfgBool
    /// GUID of the chassis/bus a module is plugged into
    pub const CONNECTS_TO_LINK_NAME: c_int = 16818176; // char *
    pub const IS_PRESENT: c_int = 16924672; // NISysCfgIsPresentType
    pub const SLOT_NUMBER: c_int = 16822272; // int
    pub const CURRENT_TEMP: c_int = 16965632; // double
    pub const TCP_IP_ADDRESS: c_int = 16957440; // char *
    pub const NUMBER_OF_SLOTS: c_int = 16826368; // int
    pub const NUMBER_OF_EXPERTS: c_int = 16891904; // int
    pub const NUMBER_OF_TEMP_SENSORS: c_int = 17186816; // int
}

/// Property IDs for GetResourceIndexedProperty (NISysCfgIndexedProperty)
pub mod indexed_properties {
    use std::os::raw::c_int;

    pub const EXPERT_NAME: c_int = 16900096; // char *
    pub const EXPERT_RESOURCE_NAME: c_int = 16896000; // char *
    /// The NI MAX device name (e.g. DAQmx alias)
    pub const EXPERT_USER_ALIAS: c_int = 16904192; // char *
    pub const TEMPERATURE_NAME: c_int = 17190912; // char *
    pub const TEMPERATURE_READING: c_int = 16965632; // double
    pub const TEMPERATURE_UPPER_CRITICAL: c_int = 17199104; // double
}

/// Property IDs for GetSystemProperty (NISysCfgSystemProperty)
pub mod system_properties {
    use std::os::raw::c_int;

    pub const HOSTNAME: c_int = 16941063; // char *
    pub const PRODUCT_NAME: c_int = 16941078; // char *
    pub const OPERATING_SYSTEM: c_int = 16941079; // char *
    pub const OS_VERSION: c_int = 17100800; // char *
    pub const SERIAL_NUMBER: c_int = 16941080; // char *
    pub const MEMORY_PHYS_TOTAL: c_int = 219480064; // double (KB)
    pub const MEMORY_PHYS_FREE: c_int = 219484160; // double (KB)
    pub const PRIMARY_DISK_TOTAL: c_int = 219291648; // double (KB)
    pub const PRIMARY_DISK_FREE: c_int = 219295744; // double (KB)
}

/// NISysCfgInitializeSession
pub type NISysCfgInitializeSession = unsafe extern "system" fn(
    target_name: *const c_char,
    username: *const c_char,
    password: *const c_char,
    language: c_int,
    force_property_refresh: c_int,
    connect_timeout_msec: c_uint,
    expert_enum_handle: *mut NISysCfgEnumExpertHandle,
    session_handle: *mut NISysCfgSessionHandle,
) -> NISysCfgStatus;

/// NISysCfgCloseHandle (works on session, enum, and resource handles)
pub type NISysCfgCloseHandle = unsafe extern "system" fn(handle: *mut c_void) -> NISysCfgStatus;

/// NISysCfgFindHardware
pub type NISysCfgFindHardware = unsafe extern "system" fn(
    session_handle: NISysCfgSessionHandle,
    filter_mode: c_int,
    filter_handle: NISysCfgFilterHandle,
    expert_names: *const c_char,
    resource_enum_handle: *mut NISysCfgEnumResourceHandle,
) -> NISysCfgStatus;

/// NISysCfgNextResource
pub type NISysCfgNextResource = unsafe extern "system" fn(
    session_handle: NISysCfgSessionHandle,
    resource_enum_handle: NISysCfgEnumResourceHandle,
    resource_handle: *mut NISysCfgResourceHandle,
) -> NISysCfgStatus;

/// NISysCfgGetResourceProperty
pub type NISysCfgGetResourceProperty = unsafe extern "system" fn(
    resource_handle: NISysCfgResourceHandle,
    property_id: c_int,
    value: *mut c_void,
) -> NISysCfgStatus;

/// NISysCfgGetResourceIndexedProperty
pub type NISysCfgGetResourceIndexedProperty = unsafe extern "system" fn(
    resource_handle: NISysCfgResourceHandle,
    property_id: c_int,
    index: c_uint,
    value: *mut c_void,
) -> NISysCfgStatus;

/// NISysCfgGetSystemProperty
pub type NISysCfgGetSystemProperty = unsafe extern "system" fn(
    session_handle: NISysCfgSessionHandle,
    property_id: c_int,
    value: *mut c_void,
) -> NISysCfgStatus;

/// NISysCfgGetStatusDescription (session may be NULL; the returned
/// string must be released with NISysCfgFreeDetailedString)
pub type NISysCfgGetStatusDescription = unsafe extern "system" fn(
    session_handle: NISysCfgSessionHandle,
    status: NISysCfgStatus,
    detailed_description: *mut *mut c_char,
) -> NISysCfgStatus;

/// NISysCfgFreeDetailedString
pub type NISysCfgFreeDetailedString =
    unsafe extern "system" fn(str_: *mut c_char) -> NISysCfgStatus;
