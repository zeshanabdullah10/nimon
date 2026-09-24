//! Windows Service support for nimon-hub
//!
//! `nimon-hub install` registers the executable with the SCM using the
//! `run-service` argument; the SCM launches it as
//! `nimon-hub.exe run-service`, which dispatches into `run_as_service`
//! (no interactive-fallback ambiguity). The other subcommands manage the
//! registration and lifecycle.

/// Name used for SCM registration and dispatch
pub const SERVICE_NAME: &str = "NIMonHub";

/// A sensible config template written next to the exe on install when
/// no config exists yet.
pub fn default_config_yaml() -> &'static str {
    r#"# nimon-hub service configuration
host: "0.0.0.0"
port: 9090
# Relative paths are resolved against this file's directory
database_path: "../data/nimon.db"

alert:
  default_cooldown_minutes: 5
  max_firing_count: 100
  cleanup_interval_hours: 24
"#
}

/// SCM ImagePath for `exe`: the executable path is always quoted so a
/// path with spaces (`C:\Program Files\...`) cannot be hijacked by a
/// binary planted at a prefix such as `C:\Program.exe` (CWE-428).
pub fn service_command_line_for(exe: &std::path::Path) -> String {
    let path = exe.display().to_string();
    let path = path.trim_matches('"');
    format!("\"{}\" run-service", path)
}

#[cfg(windows)]
pub use windows_impl::{
    install_service, run_as_service, service_exe_command_line, start_service, stop_service,
    uninstall_service, write_default_config_if_missing,
};

#[cfg(windows)]
mod windows_impl {
    use std::path::PathBuf;

    use windows::Win32::System::Services::*;
    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
    };

    use super::{default_config_yaml, SERVICE_NAME};

    pub fn install_service(
        service_name: &str,
        display_name: &str,
        exe_command_line: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            let sc_manager = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)?;
            let exe_path_wide: Vec<u16> = exe_command_line
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let service_name_wide: Vec<u16> = service_name
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let display_name_wide: Vec<u16> = display_name
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

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

    pub fn uninstall_service(service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            let sc_manager = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)?;
            let service_name_wide: Vec<u16> = service_name
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let handle = OpenServiceW(
                sc_manager,
                windows::core::PCWSTR::from_raw(service_name_wide.as_ptr()),
                0x00010000, // DELETE
            )?;
            DeleteService(handle)?;
            CloseServiceHandle(handle)?;
            CloseServiceHandle(sc_manager)?;
        }
        Ok(())
    }

    pub fn start_service(service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            let sc_manager = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)?;
            let service_name_wide: Vec<u16> = service_name
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let handle = OpenServiceW(
                sc_manager,
                windows::core::PCWSTR::from_raw(service_name_wide.as_ptr()),
                SERVICE_ALL_ACCESS,
            )?;
            StartServiceW(handle, None)?;
            CloseServiceHandle(handle)?;
            CloseServiceHandle(sc_manager)?;
        }
        Ok(())
    }

    pub fn stop_service(service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            let sc_manager = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)?;
            let service_name_wide: Vec<u16> = service_name
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let handle = OpenServiceW(
                sc_manager,
                windows::core::PCWSTR::from_raw(service_name_wide.as_ptr()),
                SERVICE_ALL_ACCESS,
            )?;
            let mut status = SERVICE_STATUS::default();
            ControlService(handle, SERVICE_CONTROL_STOP, &mut status)?;
            CloseServiceHandle(handle)?;
            CloseServiceHandle(sc_manager)?;
        }
        Ok(())
    }

    /// Binary path registered with the SCM: the exe plus the internal
    /// `run-service` argument.
    pub fn service_exe_command_line() -> Result<String, Box<dyn std::error::Error>> {
        let exe = std::env::current_exe()?;
        Ok(super::service_command_line_for(&exe))
    }

    /// Persist a config template next to the exe when none exists
    /// (services run with CWD = System32, so the config must be located
    /// relative to the binary).
    pub fn write_default_config_if_missing() -> std::io::Result<()> {
        let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from))
        else {
            return Ok(());
        };
        let cfg_dir = dir.join("config");
        let cfg_path = cfg_dir.join("hub.yaml");
        if !cfg_path.exists() {
            std::fs::create_dir_all(&cfg_dir)?;
            std::fs::write(&cfg_path, default_config_yaml())?;
        }
        Ok(())
    }

    // ====================================================================
    // Service entry point
    // ====================================================================

    define_windows_service!(ffi_service_main, service_main);

    fn service_main(_args: Vec<std::ffi::OsString>) {
        if let Err(e) = run_service() {
            tracing::error!("nimon-hub service failed: {}", e);
        }
    }

    fn set_status(
        handle: &service_control_handler::ServiceStatusHandle,
        state: ServiceState,
        exit_code: ServiceExitCode,
        wait_hint: Duration,
        checkpoint: u32,
    ) -> windows_service::Result<()> {
        handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code,
            checkpoint,
            wait_hint,
            process_id: None,
        })
    }

    use std::time::Duration;

    /// Directory of the running executable.
    fn exe_dir() -> Option<PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from))
    }

    /// Load the service config: `<exe dir>/config/hub.yaml`, with relative
    /// paths resolved against the config file's directory.
    fn load_service_config() -> anyhow::Result<crate::config::HubConfig> {
        let dir = exe_dir().ok_or_else(|| anyhow::anyhow!("cannot locate the executable"))?;
        let config_dir = dir.join("config");
        let config_path = config_dir.join("hub.yaml");
        let mut config = crate::config::HubConfig::load_or_default(&config_path)
            .map_err(|e| anyhow::anyhow!("invalid config {}: {}", config_path.display(), e))?;
        config.resolve_relative_paths(&config_dir);
        tracing::info!(
            "Service config {} (database {})",
            config_path.display(),
            config.database_path
        );
        Ok(config)
    }

    fn run_service() -> windows_service::Result<()> {
        // The SCM starts services with CWD = System32: work next to the exe
        if let Some(dir) = exe_dir() {
            if let Err(e) = std::env::set_current_dir(&dir) {
                tracing::warn!("Cannot change directory to {}: {}", dir.display(), e);
            }
        }

        // Stop requests: a watch channel (non-blocking, never lost) — the
        // shutdown future must not block the single-threaded actix runtime.
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);

        let handler = move |control: ServiceControl| -> ServiceControlHandlerResult {
            match control {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    let _ = stop_tx.send(true);
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let status_handle = service_control_handler::register(SERVICE_NAME, handler)?;
        set_status(
            &status_handle,
            ServiceState::StartPending,
            ServiceExitCode::Win32(0),
            Duration::from_secs(30),
            1,
        )?;

        let config = match load_service_config() {
            Ok(config) => config,
            Err(e) => {
                tracing::error!("nimon-hub service cannot start: {}", e);
                set_status(
                    &status_handle,
                    ServiceState::Stopped,
                    ServiceExitCode::ServiceSpecific(1),
                    Duration::from_secs(1),
                    0,
                )?;
                return Ok(());
            }
        };

        set_status(
            &status_handle,
            ServiceState::Running,
            ServiceExitCode::Win32(0),
            Duration::from_secs(0),
            0,
        )?;

        // Run the hub (actix system + LocalSet) on a dedicated thread.
        let worker = std::thread::spawn(move || -> anyhow::Result<()> {
            let runtime = actix_rt::System::new();
            runtime.block_on(async move {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(async move {
                        let shutdown = async move {
                            let _ = stop_rx.wait_for(|stop| *stop).await;
                            tracing::info!("Service stop requested");
                        };
                        crate::run_with_shutdown(config, shutdown).await
                    })
                    .await
            })
        });

        let outcome = match worker.join() {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("hub worker thread panicked")),
        };

        let exit_code = match &outcome {
            Ok(()) => {
                tracing::info!("nimon-hub service stopped");
                ServiceExitCode::Win32(0)
            }
            Err(e) => {
                tracing::error!("nimon-hub service stopped with an error: {}", e);
                ServiceExitCode::ServiceSpecific(1)
            }
        };
        set_status(
            &status_handle,
            ServiceState::Stopped,
            exit_code,
            Duration::from_secs(1),
            0,
        )?;
        Ok(())
    }

    /// Dispatch the process as the NIMonHub service. Blocks until the
    /// service is stopped. Only call when the process was launched by the
    /// SCM (the `run-service` subcommand guarantees this).
    pub fn run_as_service() -> Result<(), Box<dyn std::error::Error>> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .map_err(|e| format!("Service dispatch failed: {}", e).into())
    }
}

#[cfg(not(windows))]
pub fn install_service(
    _service_name: &str,
    _display_name: &str,
    _exe_command_line: &str,
) -> Result<(), Box<dyn std::error::Error>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_image_path_is_quoted() {
        let exe = std::path::Path::new(r"C:\Program Files\NIMon Hub\nimon-hub.exe");
        assert_eq!(
            service_command_line_for(exe),
            r#""C:\Program Files\NIMon Hub\nimon-hub.exe" run-service"#
        );
        // No double quoting
        let quoted = std::path::Path::new(r#""C:\nimon\nimon-hub.exe""#);
        assert_eq!(
            service_command_line_for(quoted),
            r#""C:\nimon\nimon-hub.exe" run-service"#
        );
    }
}
