//! Raw FFI bindings to NI-DAQmx API
//!
//! Verified against NIDAQmx.h (`Shared\ExternalCompilerSupport\C\include`).
//! Every function is `int32 __CFUNC`, where `__CFUNC` is `__stdcall` on
//! Windows and empty on Linux, i.e. Rust's `extern "system"`.
//! NI-DAQmx is nicaiu.dll on Windows and libnidaqmx.so on Linux.
//!
//! DAQmx string getters follow the usual NI convention: passing
//! `(NULL, 0)` returns the required buffer size (a positive number).

use std::os::raw::c_char;

/// int32 status: < 0 error, 0 success, > 0 warning
pub type Int32 = i32;
/// uInt32 (unsigned long on Win32, unsigned int on LP64 - always 32 bits)
pub type UInt32 = u32;
/// bool32 = uInt32
pub type Bool32 = u32;

/// Upper bound for any DAQmx string (device name lists, error info)
pub const DAQMX_MAX_STRING: usize = 1 << 20;

/// Errors that mean "this device does not support the property" rather
/// than "the device is broken"
pub mod errors {
    pub const ATTR_NOT_SUPPORTED: i32 = -200197;
    pub const ATTRIBUTE_NOT_SUPPORTED_IN_TASK_CONTEXT: i32 = -200452;
    pub const ATTR_NOT_SUPPORTED_ON_ACCESSORY: i32 = -201421;
    pub const ATTR_NOT_SUPPORTED_USE_PHYSICAL_CHANNEL_PROPERTY: i32 = -209896;
}

/// int32 DAQmxGetSysDevNames(char *data, uInt32 bufferSize);
pub type DAQmxGetSysDevNames =
    unsafe extern "system" fn(data: *mut c_char, buffer_size: UInt32) -> Int32;

/// int32 DAQmxResetDevice(const char deviceName[]);
pub type DAQmxResetDevice = unsafe extern "system" fn(device_name: *const c_char) -> Int32;

/// int32 DAQmxSelfTestDevice(const char deviceName[]);
pub type DAQmxSelfTestDevice = unsafe extern "system" fn(device_name: *const c_char) -> Int32;

/// int32 DAQmxGetDevProductType(const char device[], char *data, uInt32 bufferSize);
pub type DAQmxGetDevProductType = unsafe extern "system" fn(
    device: *const c_char,
    data: *mut c_char,
    buffer_size: UInt32,
) -> Int32;

/// int32 DAQmxGetDevProductNum(const char device[], uInt32 *data);
pub type DAQmxGetDevProductNum =
    unsafe extern "system" fn(device: *const c_char, data: *mut UInt32) -> Int32;

/// int32 DAQmxGetDevSerialNum(const char device[], uInt32 *data);
pub type DAQmxGetDevSerialNum =
    unsafe extern "system" fn(device: *const c_char, data: *mut UInt32) -> Int32;

/// int32 DAQmxGetDevIsSimulated(const char device[], bool32 *data);
pub type DAQmxGetDevIsSimulated =
    unsafe extern "system" fn(device: *const c_char, data: *mut Bool32) -> Int32;

/// int32 DAQmxGetCalDevTemp(const char deviceName[], float64 *data);
pub type DAQmxGetCalDevTemp =
    unsafe extern "system" fn(device_name: *const c_char, data: *mut f64) -> Int32;

/// int32 DAQmxGetExtendedErrorInfo(char errorString[], uInt32 bufferSize);
/// (per-thread: describes the last error on the calling thread)
pub type DAQmxGetExtendedErrorInfo =
    unsafe extern "system" fn(error_string: *mut c_char, buffer_size: UInt32) -> Int32;
