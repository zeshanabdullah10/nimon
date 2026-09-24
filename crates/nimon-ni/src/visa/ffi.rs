//! Raw FFI bindings to NI-VISA API
//!
//! Verified against visa.h / visatype.h (IVI Foundation VISA, Win64 and
//! WinNT include directories). `_VI_FUNC` is `__stdcall` on Win32/Win64
//! and empty on Unix, i.e. Rust's `extern "system"`.
//! Library: visa64.dll (64-bit) / visa32.dll (32-bit) on Windows,
//! libvisa.so on Linux.

use std::os::raw::{c_char, c_void};

/// ViStatus: ViInt32; < 0 error, 0 success, > 0 completion/warning code
pub type ViStatus = i32;
/// ViUInt32 (unsigned long on Win32/LLP64, unsigned int on LP64: 32 bits)
pub type ViUInt32 = u32;
/// ViObject / ViSession / ViFindList are all ViUInt32 *values*, not pointers
pub type ViObject = ViUInt32;
pub type ViSession = ViObject;
pub type ViFindList = ViObject;
pub type ViAttr = ViUInt32;
pub type ViAccessMode = ViUInt32;
/// ViAttrState: ViUInt64 when `_VISA_ENV_IS_64_BIT` (Win64 / LP64),
/// otherwise ViUInt32
#[cfg(target_pointer_width = "64")]
pub type ViAttrState = u64;
#[cfg(not(target_pointer_width = "64"))]
pub type ViAttrState = u32;

/// VI_NULL session / object
pub const VI_NULL: ViObject = 0;
/// VI_NO_LOCK access mode
pub const VI_NO_LOCK: ViAccessMode = 0;
/// VI_FIND_BUFLEN: buffer size for viFindRsrc / viFindNext descriptors
pub const VI_FIND_BUFLEN: usize = 256;

/// VI_ERROR_RSRC_NFOUND (0xBFFF0011): no resource matched the expression
pub const VI_ERROR_RSRC_NFOUND: ViStatus = 0xBFFF0011u32 as ViStatus;
/// VI_ERROR_TMO (0xBFFF0015): timeout expired before operation completed
pub const VI_ERROR_TMO: ViStatus = 0xBFFF0015u32 as ViStatus;

/// Attribute IDs (ViAttr)
pub mod attributes {
    use super::ViAttr;

    /// I/O timeout in milliseconds (ViUInt32, read/write)
    pub const VI_ATTR_TMO_VALUE: ViAttr = 0x3FFF001A;
    /// Interface type (ViUInt16, read-only)
    pub const VI_ATTR_INTF_TYPE: ViAttr = 0x3FFF0171;
}

/// Interface type values returned by VI_ATTR_INTF_TYPE
pub mod intf {
    pub const VI_INTF_GPIB: u16 = 1;
    pub const VI_INTF_VXI: u16 = 2;
    pub const VI_INTF_GPIB_VXI: u16 = 3;
    pub const VI_INTF_ASRL: u16 = 4;
    pub const VI_INTF_PXI: u16 = 5;
    pub const VI_INTF_TCPIP: u16 = 6;
    pub const VI_INTF_USB: u16 = 7;
    pub const VI_INTF_RIO: u16 = 8;
    pub const VI_INTF_FIREWIRE: u16 = 9;
}

/// ViStatus viOpenDefaultRM(ViPSession vi);
pub type ViOpenDefaultRM = unsafe extern "system" fn(vi: *mut ViSession) -> ViStatus;

/// ViStatus viClose(ViObject vi);
pub type ViClose = unsafe extern "system" fn(vi: ViObject) -> ViStatus;

/// ViStatus viFindRsrc(ViSession sesn, ViConstString expr, ViPFindList vi,
///                     ViPUInt32 retCnt, ViChar desc[]);
pub type ViFindRsrc = unsafe extern "system" fn(
    sesn: ViSession,
    expr: *const c_char,
    find_list: *mut ViFindList,
    ret_cnt: *mut ViUInt32,
    desc: *mut c_char,
) -> ViStatus;

/// ViStatus viFindNext(ViFindList vi, ViChar desc[]);
pub type ViFindNext =
    unsafe extern "system" fn(find_list: ViFindList, desc: *mut c_char) -> ViStatus;

/// ViStatus viOpen(ViSession sesn, ViConstRsrc name, ViAccessMode mode,
///                 ViUInt32 timeout, ViPSession vi);
///
/// `timeout` is the *lock* wait time (only meaningful when a lock is
/// requested), not the I/O timeout.
pub type ViOpen = unsafe extern "system" fn(
    sesn: ViSession,
    name: *const c_char,
    mode: ViAccessMode,
    timeout: ViUInt32,
    vi: *mut ViSession,
) -> ViStatus;

/// ViStatus viWrite(ViSession vi, ViConstBuf buf, ViUInt32 cnt, ViPUInt32 retCnt);
pub type ViWrite = unsafe extern "system" fn(
    vi: ViSession,
    buf: *const u8,
    cnt: ViUInt32,
    ret_cnt: *mut ViUInt32,
) -> ViStatus;

/// ViStatus viRead(ViSession vi, ViPBuf buf, ViUInt32 cnt, ViPUInt32 retCnt);
pub type ViRead = unsafe extern "system" fn(
    vi: ViSession,
    buf: *mut u8,
    cnt: ViUInt32,
    ret_cnt: *mut ViUInt32,
) -> ViStatus;

/// ViStatus viGetAttribute(ViObject vi, ViAttr attrName, void *attrValue);
pub type ViGetAttribute =
    unsafe extern "system" fn(vi: ViObject, attr: ViAttr, value: *mut c_void) -> ViStatus;

/// ViStatus viSetAttribute(ViObject vi, ViAttr attrName, ViAttrState attrValue);
pub type ViSetAttribute =
    unsafe extern "system" fn(vi: ViObject, attr: ViAttr, value: ViAttrState) -> ViStatus;
