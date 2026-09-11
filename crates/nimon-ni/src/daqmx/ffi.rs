//! Raw FFI bindings to NI-DAQmx API
//!
//! This module contains the low-level C FFI definitions for the NI-DAQmx API.
//! NI-DAQmx uses nicaiu.dll on Windows.
//!
//! Note: DAQmx uses PascalCase function naming (DAQmxXxxYyy) unlike
//! VISA (viXxxYyy). There are no opaque session handles for the
//! attribute-query functions used here -- just pass device name strings.

#![allow(dead_code)]

use std::os::raw::{c_char, c_double, c_int};

// ---------------------------------------------------------------------------
// Status constants
// ---------------------------------------------------------------------------

/// DAQmx success status code
pub const DAQMX_SUCCESS: c_int = 0;

/// Buffer size for device name lists and product names
pub const DAQMX_BUFFER_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// Function pointer types
// ---------------------------------------------------------------------------

/// Function signature for DAQmxGetSysDevNames
///
/// Returns a comma-separated list of DAQmx device names installed on the system.
pub type DAQmxGetSysDevNames =
    unsafe extern "C" fn(dev_names_buffer: *mut c_char, buffer_size: c_int) -> c_int;

/// Function signature for DAQmxGetDevProductTypeName
///
/// Returns the product type name for a given device (e.g., "NI PXIe-6363").
pub type DAQmxGetDevProductTypeName = unsafe extern "C" fn(
    device_name: *const c_char,
    product_name_buffer: *mut c_char,
    buffer_size: c_int,
) -> c_int;

/// Function signature for DAQmxGetDevSerialNum
///
/// Returns the serial number for a given device.
pub type DAQmxGetDevSerialNum =
    unsafe extern "C" fn(device_name: *const c_char, serial_number: *mut u32) -> c_int;

/// Function signature for DAQmxGetDevTemperature
///
/// Returns the current device temperature in degrees Celsius.
pub type DAQmxGetDevTemperature =
    unsafe extern "C" fn(device_name: *const c_char, temperature: *mut c_double) -> c_int;

/// Function signature for DAQmxGetDevSelfTestResult
///
/// Returns the self-test result for a given device (0 = pass).
pub type DAQmxGetDevSelfTestResult =
    unsafe extern "C" fn(device_name: *const c_char, self_test_result: *mut c_int) -> c_int;

/// Function signature for DAQmxGetDevAIPowerSupplyVoltages
///
/// Returns the analog input power supply voltages for a given device.
pub type DAQmxGetDevAIPowerSupplyVoltages = unsafe extern "C" fn(
    device_name: *const c_char,
    v5: *mut c_double,
    v3v3: *mut c_double,
    v_user: *mut c_double,
    v_neg_user: *mut c_double,
) -> c_int;

/// Function signature for DAQmxResetDevice
///
/// Resets a device to its default state.
pub type DAQmxResetDevice = unsafe extern "C" fn(device_name: *const c_char) -> c_int;

/// Function signature for DAQmxGetDevProductNumber
///
/// Returns the product number for a given device (e.g., "6363" for PXIe-6363).
pub type DAQmxGetDevProductNumber =
    unsafe extern "C" fn(device_name: *const c_char, product_number: *mut c_int) -> c_int;
