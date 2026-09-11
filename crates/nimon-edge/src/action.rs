//! Edge-side remediation action execution
//!
//! Executes actions requested by the hub: DAQmx device resets
//! (power cycle / driver reset), OS service restarts, and allowlisted
//! maintenance scripts.

use std::path::{Path, PathBuf};
use std::time::Instant;

use nimon_core::actor::messages::{ActionResult, ActionType, ExecuteAction};
use tracing::{info, warn};

/// Runtime configuration for edge action execution.
#[derive(Debug, Clone)]
pub struct ActionRuntime {
    /// Script names the edge is willing to run (allowlist)
    pub allowed_scripts: Vec<String>,
    /// Directory where allowlisted scripts live
    pub scripts_dir: PathBuf,
    /// Per-script execution timeout
    pub timeout_secs: u64,
}

impl Default for ActionRuntime {
    fn default() -> Self {
        Self {
            allowed_scripts: Vec::new(),
            scripts_dir: PathBuf::from("scripts"),
            timeout_secs: 120,
        }
    }
}

impl ActionRuntime {
    /// Execute a hub-requested action and produce a correlated result.
    pub async fn run(&self, action: ExecuteAction) -> ActionResult {
        let start = Instant::now();
        info!(
            "Executing action {} ({:?}) for device {}",
            action.action_id, action.action_type, action.device_id
        );

        let outcome = match action.action_type {
            ActionType::PowerCycle => {
                Self::delay(action.parameters.get("delay_secs")).await;
                self.reset_device(&action.device_id).await
            }
            ActionType::ResetDriver => self.reset_device(&action.device_id).await,
            ActionType::RestartServices => self.restart_services(&action.parameters).await,
            ActionType::CustomScript => self.run_script(&action.parameters).await,
        };

        let (success, output, error, exit_code) = outcome;
        ActionResult {
            action_id: action.action_id,
            success,
            output: if output.is_empty() {
                None
            } else {
                Some(output)
            },
            error: if error.is_empty() { None } else { Some(error) },
            exit_code,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    async fn delay(delay_secs: Option<&String>) {
        if let Some(secs) = delay_secs.and_then(|s| s.parse::<u64>().ok()) {
            tokio::time::sleep(std::time::Duration::from_secs(secs.min(300))).await;
        }
    }

    /// Extract the device's local name from the composite edge id
    /// (`edge-id:device` → `device`).
    fn local_device_name(device_id: &str) -> &str {
        match device_id.rsplit_once(':') {
            Some((_, local)) if !local.is_empty() => local,
            _ => device_id,
        }
    }

    /// Reset a device through NI-DAQmx. Blocking FFI runs on the
    /// blocking pool.
    async fn reset_device(&self, device_id: &str) -> (bool, String, String, Option<i32>) {
        let device_name = Self::local_device_name(device_id).to_string();
        let result = tokio::task::spawn_blocking(move || match nimon_ni::daqmx::NiDaqMx::load() {
            Ok(daqmx) => match daqmx.reset_device(&device_name) {
                Ok(()) => Ok(format!("DAQmxResetDevice({}) completed", device_name)),
                Err(e) => Err(format!("DAQmxResetDevice({}) failed: {}", device_name, e)),
            },
            Err(e) => Err(format!("NI-DAQmx driver unavailable: {}", e)),
        })
        .await;

        match result {
            Ok(Ok(output)) => (true, output, String::new(), None),
            Ok(Err(error)) => (false, String::new(), error, None),
            Err(e) => (
                false,
                String::new(),
                format!("reset task failed: {}", e),
                None,
            ),
        }
    }

    /// Restart OS services named in `parameters["services"]`
    /// (comma-separated).
    async fn restart_services(
        &self,
        parameters: &std::collections::HashMap<String, String>,
    ) -> (bool, String, String, Option<i32>) {
        let services: Vec<String> = parameters
            .get("services")
            .map(|s| {
                s.split(',')
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        if services.is_empty() {
            return (
                false,
                String::new(),
                "no 'services' parameter provided".to_string(),
                None,
            );
        }

        for name in &services {
            // Defensive: service names are simple identifiers
            if !name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
            {
                return (
                    false,
                    String::new(),
                    format!("invalid service name: {}", name),
                    None,
                );
            }
        }

        let mut output = String::new();
        for name in &services {
            match restart_one_service(name).await {
                Ok(out) => output.push_str(&out),
                Err(e) => return (false, output, e, None),
            }
        }
        (true, output, String::new(), Some(0))
    }

    /// Run an allowlisted maintenance script by name.
    async fn run_script(
        &self,
        parameters: &std::collections::HashMap<String, String>,
    ) -> (bool, String, String, Option<i32>) {
        let Some(name) = parameters.get("script").map(|s| s.trim()) else {
            return (
                false,
                String::new(),
                "no 'script' parameter provided".to_string(),
                None,
            );
        };

        // Allowlist + traversal guard
        if !self.allowed_scripts.iter().any(|a| a == name) {
            warn!("Rejected non-allowlisted script '{}'", name);
            return (
                false,
                String::new(),
                format!("script '{}' is not allowlisted on this edge", name),
                None,
            );
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") || name.is_empty() {
            return (
                false,
                String::new(),
                format!("invalid script name: {}", name),
                None,
            );
        }

        let script_path: PathBuf = self.scripts_dir.join(name);
        if !script_path.is_file() {
            return (
                false,
                String::new(),
                format!("script not found: {}", script_path.display()),
                None,
            );
        }

        let timeout = std::time::Duration::from_secs(self.timeout_secs.max(1));
        let run = tokio::time::timeout(timeout, script_command(&script_path).output()).await;
        match run {
            Err(_) => (
                false,
                String::new(),
                format!("script timed out after {}s", self.timeout_secs),
                None,
            ),
            Ok(Err(e)) => (false, String::new(), format!("script failed: {}", e), None),
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let success = out.status.success();
                (
                    success,
                    stdout,
                    if success { String::new() } else { stderr },
                    out.status.code(),
                )
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn script_command(script_path: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
    cmd.arg(script_path);
    cmd
}

#[cfg(not(target_os = "windows"))]
fn script_command(script_path: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg(script_path);
    cmd
}

#[cfg(target_os = "windows")]
async fn restart_one_service(name: &str) -> Result<String, String> {
    let mut output = String::new();
    for verb in ["stop", "start"] {
        let result = tokio::process::Command::new("sc")
            .args([verb, name])
            .output()
            .await
            .map_err(|e| format!("sc {} {} failed: {}", verb, name, e))?;
        let text = format!(
            "sc {} {}: {}{}",
            verb,
            name,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
        );
        output.push_str(&text);
        if !result.status.success() {
            // `sc stop` on an already-stopped service also reports failure;
            // only `start` failures abort.
            if verb == "start" {
                return Err(format!("failed to start service {}: {}", name, text));
            }
        }
    }
    Ok(output)
}

#[cfg(not(target_os = "windows"))]
async fn restart_one_service(name: &str) -> Result<String, String> {
    let result = tokio::process::Command::new("systemctl")
        .args(["restart", name])
        .output()
        .await
        .map_err(|e| format!("systemctl restart {} failed: {}", name, e))?;
    if result.status.success() {
        Ok(format!("systemctl restart {} succeeded", name))
    } else {
        Err(format!(
            "systemctl restart {} failed: {}{}",
            name,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn runtime(scripts: &[&str], dir: &str) -> ActionRuntime {
        ActionRuntime {
            allowed_scripts: scripts.iter().map(|s| s.to_string()).collect(),
            scripts_dir: PathBuf::from(dir),
            timeout_secs: 30,
        }
    }

    fn action(action_type: ActionType, parameters: HashMap<String, String>) -> ExecuteAction {
        ExecuteAction {
            action_id: "test-action".to_string(),
            device_id: "edge-1:daq-1".to_string(),
            action_type,
            parameters,
        }
    }

    #[tokio::test]
    async fn test_local_device_name_extraction() {
        assert_eq!(ActionRuntime::local_device_name("edge-1:daq-1"), "daq-1");
        assert_eq!(ActionRuntime::local_device_name("daq-1"), "daq-1");
    }

    #[tokio::test]
    async fn test_restart_services_requires_parameter() {
        let rt = runtime(&[], "scripts");
        let result = rt
            .run(action(ActionType::RestartServices, HashMap::new()))
            .await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("services"));
    }

    #[tokio::test]
    async fn test_restart_services_rejects_bad_names() {
        let rt = runtime(&[], "scripts");
        let mut parameters = HashMap::new();
        parameters.insert("services".to_string(), "svc; rm -rf /".to_string());
        let result = rt
            .run(action(ActionType::RestartServices, parameters))
            .await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("invalid service name"));
    }

    #[tokio::test]
    async fn test_custom_script_rejects_non_allowlisted() {
        let rt = runtime(&["safe.ps1"], "scripts");
        let mut parameters = HashMap::new();
        parameters.insert("script".to_string(), "evil.ps1".to_string());
        let result = rt.run(action(ActionType::CustomScript, parameters)).await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not allowlisted"));
    }

    #[tokio::test]
    async fn test_custom_script_rejects_traversal() {
        let rt = runtime(&["..\\evil.ps1"], "scripts");
        let mut parameters = HashMap::new();
        parameters.insert("script".to_string(), "..\\evil.ps1".to_string());
        let result = rt.run(action(ActionType::CustomScript, parameters)).await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("invalid script name"));
    }

    #[tokio::test]
    async fn test_custom_script_runs_allowlisted() {
        let dir = tempfile::tempdir().unwrap();
        let script_path = dir.path().join("echo.sh");
        std::fs::write(&script_path, "#!/bin/sh\necho hello-script\n").unwrap();

        let rt = ActionRuntime {
            allowed_scripts: vec!["echo.sh".to_string()],
            scripts_dir: dir.path().to_path_buf(),
            timeout_secs: 10,
        };
        let mut parameters = HashMap::new();
        parameters.insert("script".to_string(), "echo.sh".to_string());
        let result = rt.run(action(ActionType::CustomScript, parameters)).await;

        // On Windows the .sh runs through powershell and fails; the
        // security-critical assertions are the allowlist ones above.
        #[cfg(not(target_os = "windows"))]
        {
            assert!(result.success);
            assert_eq!(result.output.unwrap().trim(), "hello-script");
        }
        let _ = result;
    }

    #[tokio::test]
    async fn test_power_cycle_reports_unavailable_driver_gracefully() {
        // On a machine without NI-DAQmx the action must fail with a
        // clear error rather than panic or hang.
        let rt = runtime(&[], "scripts");
        let result = rt.run(action(ActionType::PowerCycle, HashMap::new())).await;
        if !nimon_ni::daqmx::NiDaqMx::is_available() {
            assert!(!result.success);
            assert!(result.error.unwrap().contains("DAQmx"));
        }
    }
}
