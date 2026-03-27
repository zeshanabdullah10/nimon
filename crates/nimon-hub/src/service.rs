//! Windows Service support for nimon-hub

#[cfg(windows)]
use windows::Win32::System::Services::*;

#[cfg(windows)]
pub fn run_as_service() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

#[cfg(windows)]
pub fn install_service(service_name: &str, display_name: &str, exe_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let sc_manager = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)?;
        let exe_path_wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
        let service_name_wide: Vec<u16> = service_name.encode_utf16().chain(std::iter::once(0)).collect();
        let display_name_wide: Vec<u16> = display_name.encode_utf16().chain(std::iter::once(0)).collect();

        let handle = CreateServiceW(
            sc_manager,
            windows::core::PCWSTR::from_raw(service_name_wide.as_ptr()),
            windows::core::PCWSTR::from_raw(display_name_wide.as_ptr()),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            windows::core::PCWSTR::from_raw(exe_path_wide.as_ptr()),
            None,
            None,
            None,
            None,
            None,
        )?;

        CloseServiceHandle(handle)?;
        CloseServiceHandle(sc_manager)?;
    }
    Ok(())
}

#[cfg(windows)]
pub fn uninstall_service(service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let sc_manager = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)?;
        let service_name_wide: Vec<u16> = service_name.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = OpenServiceW(sc_manager, windows::core::PCWSTR::from_raw(service_name_wide.as_ptr()), 0x00010000)?;
        DeleteService(handle)?;
        CloseServiceHandle(handle)?;
        CloseServiceHandle(sc_manager)?;
    }
    Ok(())
}

#[cfg(windows)]
pub fn start_service(service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let sc_manager = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)?;
        let service_name_wide: Vec<u16> = service_name.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = OpenServiceW(sc_manager, windows::core::PCWSTR::from_raw(service_name_wide.as_ptr()), SERVICE_ALL_ACCESS)?;
        StartServiceW(handle, None)?;
        CloseServiceHandle(handle)?;
        CloseServiceHandle(sc_manager)?;
    }
    Ok(())
}

#[cfg(windows)]
pub fn stop_service(service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let sc_manager = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)?;
        let service_name_wide: Vec<u16> = service_name.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = OpenServiceW(sc_manager, windows::core::PCWSTR::from_raw(service_name_wide.as_ptr()), SERVICE_ALL_ACCESS)?;
        let mut status = SERVICE_STATUS::default();
        ControlService(handle, SERVICE_CONTROL_STOP, &mut status)?;
        CloseServiceHandle(handle)?;
        CloseServiceHandle(sc_manager)?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn install_service(_service_name: &str, _display_name: &str, _exe_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    Err("Windows service support is only available on Windows".into())
}

#[cfg(not(windows))]
pub fn uninstall_service(_service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    Err("Windows service support is only available on Windows".into())
}

#[cfg(not(windows))]
pub fn start_service(_service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    Err("Windows service support is only available on Windows".into())
}

#[cfg(not(windows))]
pub fn stop_service(_service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    Err("Windows service support is only available on Windows".into())
}
