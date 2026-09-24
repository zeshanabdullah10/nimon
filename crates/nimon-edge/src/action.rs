//! Edge-side remediation action execution
//!
//! Executes actions requested by the hub: DAQmx device resets
//! (power cycle / driver reset), OS service restarts, and allowlisted
//! maintenance scripts.
//!
//! Safety rules:
//! - device actions only target devices this edge currently manages
//!   (looked up in the [`DeviceRegistry`]); anything else is rejected
//! - service restarts require `action.allowed_services` (empty = denied)
//! - scripts must be allowlisted and live in `action.scripts_dir`
//! - one action per device at a time (keyed by the resolved registry id);
//!   at most `max_concurrent` overall
//! - every action is bounded by `action.timeout_secs` (a PowerCycle's
//!   `delay_secs` counts toward it and is waited out without holding a
//!   slot); timed-out scripts are killed together with their child
//!   processes, and output held open by background processes is not
//!   waited for

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use nimon_core::actor::messages::{ActionResult, ActionType, ExecuteAction};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{info, warn};

use crate::config::{validate_script_name, ActionConfig};
use crate::devices::{DeviceRegistry, LocalDevice};

/// Captured stdout/stderr per stream
const OUTPUT_CAP: usize = 64 * 1024;
/// Upper bound for PowerCycle's `delay_secs`
const MAX_POWER_CYCLE_DELAY_SECS: u64 = 300;
/// How long to wait for output readers after a killed script
const READER_GRACE: Duration = Duration::from_secs(2);

/// Outcome of one action body: (success, output, error, exit_code)
type Outcome = (bool, String, String, Option<i32>);

fn failed(error: impl Into<String>) -> Outcome {
    (false, String::new(), error.into(), None)
}

/// Runtime configuration + state for edge action execution.
#[derive(Debug, Clone)]
pub struct ActionRuntime {
    /// Script names the edge is willing to run (allowlist)
    pub allowed_scripts: Vec<String>,
    /// Directory where allowlisted scripts live
    pub scripts_dir: PathBuf,
    /// Timeout for any single action
    pub timeout_secs: u64,
    /// OS services that may be restarted (empty = none)
    pub allowed_services: Vec<String>,
    /// `api.daqmx.enabled`: allow DAQmx resets
    pub daqmx_enabled: bool,
    /// This edge's id (hub device ids are `edge-id:local`)
    pub edge_id: String,
    /// Devices managed by this edge
    pub registry: DeviceRegistry,
    busy: Arc<Mutex<HashSet<String>>>,
    permits: Arc<Semaphore>,
}

impl Default for ActionRuntime {
    fn default() -> Self {
        Self::new(
            &ActionConfig::default(),
            true,
            "edge-1",
            DeviceRegistry::new(),
        )
    }
}

/// Marks a device busy; released on drop
struct DeviceLock {
    busy: Arc<Mutex<HashSet<String>>>,
    key: String,
}

impl DeviceLock {
    fn acquire(busy: &Arc<Mutex<HashSet<String>>>, key: &str) -> Option<Self> {
        let mut set = busy.lock().unwrap_or_else(PoisonError::into_inner);
        if !set.insert(key.to_string()) {
            return None;
        }
        Some(Self {
            busy: busy.clone(),
            key: key.to_string(),
        })
    }
}

impl Drop for DeviceLock {
    fn drop(&mut self) {
        self.busy
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

/// A validated action, ready to execute
enum Plan {
    Reset { daqmx_name: String, delay_secs: u64 },
    Services(Vec<String>),
    Script(PathBuf),
}

impl ActionRuntime {
    pub fn new(
        config: &ActionConfig,
        daqmx_enabled: bool,
        edge_id: impl Into<String>,
        registry: DeviceRegistry,
    ) -> Self {
        Self {
            allowed_scripts: config.allowed_scripts.clone(),
            scripts_dir: PathBuf::from(&config.scripts_dir),
            timeout_secs: config.timeout_secs.max(1),
            allowed_services: config.allowed_services.clone(),
            daqmx_enabled,
            edge_id: edge_id.into(),
            registry,
            busy: Arc::new(Mutex::new(HashSet::new())),
            permits: Arc::new(Semaphore::new(config.max_concurrent.max(1))),
        }
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.max(1))
    }

    /// Execute a hub-requested action and produce a correlated result.
    pub async fn run(&self, action: ExecuteAction) -> ActionResult {
        let start = Instant::now();
        info!(
            "Executing action {} ({}) for device {}",
            action.action_id, action.action_type, action.device_id
        );
        let (success, output, error, exit_code) = self.execute(&action).await;
        if !success {
            warn!(
                "Action {} ({}) failed: {}",
                action.action_id, action.action_type, error
            );
        }
        ActionResult {
            action_id: action.action_id,
            success,
            output: (!output.is_empty()).then_some(output),
            error: (!error.is_empty()).then_some(error),
            exit_code,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    async fn execute(&self, action: &ExecuteAction) -> Outcome {
        let deadline = Instant::now() + self.timeout();
        let plan = match self.plan(action) {
            Ok(p) => p,
            Err(e) => return failed(e),
        };

        // one lock per *managed device*: `edge-1:Dev1` and `Dev1` are the
        // same device
        let lock_key = self
            .resolve_id(&action.device_id)
            .unwrap_or_else(|| action.device_id.clone());
        let Some(lock) = DeviceLock::acquire(&self.busy, &lock_key) else {
            return failed(format!(
                "another action is already running for device {}",
                lock_key
            ));
        };

        // PowerCycle: wait out the requested delay holding only the device
        // lock (no global slot), inside the action's timeout budget
        if let Plan::Reset { delay_secs, .. } = &plan {
            if *delay_secs > 0 {
                tokio::time::sleep(Duration::from_secs(*delay_secs)).await;
            }
        }
        let slot_wait = match &plan {
            Plan::Reset { .. } => remaining(deadline),
            _ => self.timeout(),
        };
        let permit =
            match tokio::time::timeout(slot_wait, self.permits.clone().acquire_owned()).await {
                Ok(Ok(p)) => p,
                _ => {
                    return failed(format!(
                        "edge busy: no action slot became free within {}s",
                        slot_wait.as_secs()
                    ))
                }
            };

        match plan {
            Plan::Reset { daqmx_name, .. } => {
                self.reset_device(daqmx_name, lock, permit, remaining(deadline))
                    .await
            }
            Plan::Services(services) => {
                let _guards = (lock, permit);
                self.restart_services(&services).await
            }
            Plan::Script(path) => {
                let _guards = (lock, permit);
                run_script_process(&path, self.timeout()).await
            }
        }
    }

    /// Validate an action without side effects
    fn plan(&self, action: &ExecuteAction) -> Result<Plan, String> {
        match action.action_type {
            ActionType::PowerCycle | ActionType::ResetDriver => {
                if !self.daqmx_enabled {
                    return Err(
                        "NI-DAQmx actions are disabled on this edge (api.daqmx.enabled: false)"
                            .to_string(),
                    );
                }
                let local = self.resolve_local(&action.device_id)?;
                if local.is_simulated {
                    return Err(format!(
                        "device {} is simulated; nothing to reset",
                        action.device_id
                    ));
                }
                let daqmx_name = local.daqmx_name.ok_or_else(|| {
                    format!(
                        "device {} has no NI-DAQmx device name (no NI MAX alias)",
                        action.device_id
                    )
                })?;
                let delay_secs = if action.action_type == ActionType::PowerCycle {
                    parse_delay(action.parameters.get("delay_secs"))
                } else {
                    0
                };
                if delay_secs > 0 && delay_secs >= self.timeout_secs {
                    return Err(format!(
                        "PowerCycle delay_secs ({delay_secs}) must be shorter than \
                         action.timeout_secs ({}): the delay counts toward the action timeout",
                        self.timeout_secs
                    ));
                }
                Ok(Plan::Reset {
                    daqmx_name,
                    delay_secs,
                })
            }
            ActionType::RestartServices => self.plan_services(&action.parameters),
            ActionType::CustomScript => self.plan_script(&action.parameters),
        }
    }

    /// Map a hub device id to a device this edge manages. Accepts the
    /// full id (`edge-id:local`) or a bare local name; ids of other edges
    /// or unknown devices are rejected.
    fn resolve_local(&self, device_id: &str) -> Result<LocalDevice, String> {
        self.resolve_id(device_id)
            .and_then(|id| self.registry.get(&id))
            .ok_or_else(|| {
                format!(
                    "device {} is not managed by edge {}",
                    device_id, self.edge_id
                )
            })
    }

    /// Registry id (`edge-id:local`) of a managed device given either
    /// form of its id
    fn resolve_id(&self, device_id: &str) -> Option<String> {
        if self.registry.get(device_id).is_some() {
            return Some(device_id.to_string());
        }
        let prefix_ok = device_id
            .strip_prefix(self.edge_id.as_str())
            .is_some_and(|rest| rest.starts_with(':'));
        if prefix_ok {
            return None;
        }
        let full = format!("{}:{}", self.edge_id, device_id);
        self.registry.get(&full).is_some().then_some(full)
    }

    fn plan_services(&self, parameters: &HashMap<String, String>) -> Result<Plan, String> {
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
            return Err("no 'services' parameter provided".to_string());
        }
        for name in &services {
            // Defensive: service names are simple identifiers
            if !name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
            {
                return Err(format!("invalid service name: {}", name));
            }
        }
        if self.allowed_services.is_empty() {
            return Err(
                "service restarts are disabled on this edge (action.allowed_services is empty)"
                    .to_string(),
            );
        }
        for name in &services {
            if !self
                .allowed_services
                .iter()
                .any(|a| a.trim().eq_ignore_ascii_case(name))
            {
                warn!("Rejected non-allowlisted service '{}'", name);
                return Err(format!(
                    "service '{}' is not in action.allowed_services",
                    name
                ));
            }
        }
        Ok(Plan::Services(services))
    }

    fn plan_script(&self, parameters: &HashMap<String, String>) -> Result<Plan, String> {
        let Some(name) = parameters.get("script").map(|s| s.trim()) else {
            return Err("no 'script' parameter provided".to_string());
        };
        if let Err(e) = validate_script_name(name) {
            return Err(format!("invalid script name: {e}"));
        }
        if !self.allowed_scripts.iter().any(|a| a.trim() == name) {
            warn!("Rejected non-allowlisted script '{}'", name);
            return Err(format!("script '{}' is not allowlisted on this edge", name));
        }
        let script_path = self.scripts_dir.join(name);
        let dir = self.scripts_dir.canonicalize().map_err(|e| {
            format!(
                "action.scripts_dir {} is not accessible: {e}",
                self.scripts_dir.display()
            )
        })?;
        let real = script_path
            .canonicalize()
            .map_err(|_| format!("script not found: {}", script_path.display()))?;
        // symlinks / junctions must not lead out of the scripts directory
        if !real.starts_with(&dir) {
            warn!(
                "Rejected script '{}': resolves to {} outside {}",
                name,
                real.display(),
                dir.display()
            );
            return Err(format!(
                "script '{}' resolves outside action.scripts_dir",
                name
            ));
        }
        if !real.is_file() {
            return Err(format!("script not found: {}", script_path.display()));
        }
        Ok(Plan::Script(script_path))
    }

    /// Reset a device through NI-DAQmx on the blocking pool. The device
    /// lock and concurrency permit move into the blocking task, so a
    /// driver call that outlives the timeout keeps the device locked until
    /// it really returns.
    async fn reset_device(
        &self,
        device_name: String,
        lock: DeviceLock,
        permit: OwnedSemaphorePermit,
        timeout: Duration,
    ) -> Outcome {
        let name = device_name.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _guards = (lock, permit);
            let daqmx = nimon_ni::daqmx::NiDaqMx::load()
                .map_err(|e| format!("NI-DAQmx driver unavailable: {}", e))?;
            daqmx
                .reset_device(&name)
                .map_err(|e| match daqmx.last_error_message() {
                    Some(detail) => format!("DAQmxResetDevice({name}) failed: {e}: {detail}"),
                    None => format!("DAQmxResetDevice({name}) failed: {e}"),
                })?;
            Ok::<String, String>(format!("DAQmxResetDevice({name}) completed"))
        });
        match tokio::time::timeout(timeout, task).await {
            Err(_) => failed(format!(
                "DAQmxResetDevice({device_name}) did not return within the \
                 {}s action timeout (device stays locked until the driver call returns)",
                self.timeout_secs
            )),
            Ok(Err(e)) => failed(format!("reset task failed: {}", e)),
            Ok(Ok(Err(e))) => failed(e),
            Ok(Ok(Ok(output))) => (true, output, String::new(), None),
        }
    }

    /// Restart the (already validated) services, all within the timeout
    async fn restart_services(&self, services: &[String]) -> Outcome {
        let deadline = Instant::now() + self.timeout();
        let mut output = String::new();
        for name in services {
            match restart_one_service(name, deadline).await {
                Ok(out) => output.push_str(&out),
                Err(e) => return (false, output, e, None),
            }
        }
        (true, output, String::new(), Some(0))
    }
}

fn parse_delay(value: Option<&String>) -> u64 {
    value
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
        .min(MAX_POWER_CYCLE_DELAY_SECS)
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Run a short command with a timeout (the process is killed on timeout)
async fn run_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => Err(format!(
            "`{} {}` timed out after {}s",
            program,
            args.join(" "),
            timeout.as_secs()
        )),
        Ok(Err(e)) => Err(format!("`{} {}` failed: {}", program, args.join(" "), e)),
        Ok(Ok(out)) => Ok(out),
    }
}

fn command_text(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Output captured so far from one stream (shared with its reader task, so
/// partial output survives when the reader has to be abandoned)
#[derive(Debug, Default)]
struct Captured {
    buf: Vec<u8>,
    truncated: bool,
}

type SharedCapture = Arc<Mutex<Captured>>;

impl Captured {
    fn text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.buf).into_owned();
        if self.truncated {
            text.push_str("\n[output truncated]");
        }
        text
    }
}

fn capture_text(c: &SharedCapture) -> String {
    c.lock().unwrap_or_else(PoisonError::into_inner).text()
}

async fn read_into<R: AsyncRead + Unpin>(reader: Option<R>, cap: usize, sink: SharedCapture) {
    let Some(mut reader) = reader else {
        return;
    };
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut c = sink.lock().unwrap_or_else(PoisonError::into_inner);
                let room = cap.saturating_sub(c.buf.len());
                if n > room {
                    c.truncated = true;
                }
                c.buf.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    }
}

#[cfg(test)]
async fn read_capped<R: AsyncRead + Unpin>(reader: Option<R>, cap: usize) -> String {
    let sink = SharedCapture::default();
    read_into(reader, cap, sink.clone()).await;
    capture_text(&sink)
}

/// Wait for a reader task until `deadline`; abort it when it is still
/// running. Returns true when the stream reached EOF.
async fn finish_reader(
    mut task: tokio::task::JoinHandle<()>,
    deadline: tokio::time::Instant,
) -> bool {
    match tokio::time::timeout_at(deadline, &mut task).await {
        Ok(_) => true,
        Err(_) => {
            task.abort();
            false
        }
    }
}

/// Kill a process and all of its descendants
async fn kill_process_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    let result = run_command(
        "taskkill",
        &["/PID", &pid.to_string(), "/T", "/F"],
        Duration::from_secs(10),
    )
    .await;
    #[cfg(not(target_os = "windows"))]
    let result = run_command(
        "kill",
        &["-KILL", "--", &format!("-{pid}")],
        Duration::from_secs(5),
    )
    .await;
    if let Err(e) = result {
        warn!("Failed to kill process tree of {pid}: {e}");
    }
}

/// Run a script; on timeout kill its whole process tree
async fn run_script_process(script_path: &Path, timeout: Duration) -> Outcome {
    run_captured(script_command(script_path), timeout).await
}

/// Run a process with captured, capped output. On timeout its whole
/// process tree is killed. Output readers are only waited for
/// [`READER_GRACE`] after the process exits: a background process that
/// inherited the pipes (`start /b`, `&`, a detached child) must not keep
/// the action (and its device lock + slot) running forever.
async fn run_captured(mut cmd: tokio::process::Command, timeout: Duration) -> Outcome {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0); // own group: `kill -- -pid` reaches children
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return failed(format!("script failed to start: {}", e)),
    };
    let pid = child.id();
    let out_buf = SharedCapture::default();
    let err_buf = SharedCapture::default();
    let stdout = tokio::spawn(read_into(child.stdout.take(), OUTPUT_CAP, out_buf.clone()));
    let stderr = tokio::spawn(read_into(child.stderr.take(), OUTPUT_CAP, err_buf.clone()));

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let grace = tokio::time::Instant::now() + READER_GRACE;
            let out_done = finish_reader(stdout, grace).await;
            let err_done = finish_reader(stderr, grace).await;
            let mut out = capture_text(&out_buf);
            let err = capture_text(&err_buf);
            if !(out_done && err_done) {
                warn!(
                    "Script exited but a background process still holds its output open; \
                     not waiting for it"
                );
                out.push_str(
                    "\n[output incomplete: a background process started by the script \
                     kept its output open]",
                );
            }
            let success = status.success();
            (
                success,
                out,
                if success {
                    String::new()
                } else if err.is_empty() {
                    format!("script exited with {}", status)
                } else {
                    err
                },
                status.code(),
            )
        }
        Ok(Err(e)) => failed(format!("script failed: {}", e)),
        Err(_) => {
            if let Some(pid) = pid {
                kill_process_tree(pid).await;
            }
            let _ = child.kill().await;
            // grandchildren that escaped the kill may hold the pipes open
            let grace = tokio::time::Instant::now() + READER_GRACE;
            finish_reader(stdout, grace).await;
            finish_reader(stderr, grace).await;
            (
                false,
                capture_text(&out_buf),
                format!(
                    "script timed out after {}s and was killed",
                    timeout.as_secs()
                ),
                None,
            )
        }
    }
}

#[cfg(target_os = "windows")]
fn script_command(script_path: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ]);
    cmd.arg(script_path);
    cmd
}

#[cfg(not(target_os = "windows"))]
fn script_command(script_path: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg(script_path);
    cmd
}

/// Numeric service state from `sc query` output (1 = STOPPED,
/// 2 = START_PENDING, 3 = STOP_PENDING, 4 = RUNNING). The numeric code is
/// locale independent; the `STATE` label is preferred, otherwise the
/// second numeric field (after TYPE) is used.
pub fn parse_sc_state(output: &str) -> Option<u32> {
    let numeric = |line: &str| -> Option<u32> {
        let value = line.split_once(':')?.1.trim_start();
        let digits: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    };
    output
        .lines()
        .find(|l| l.trim_start().starts_with("STATE"))
        .and_then(numeric)
        .or_else(|| output.lines().filter_map(numeric).nth(1))
}

#[cfg(target_os = "windows")]
async fn wait_for_service_state(name: &str, wanted: u32, deadline: Instant) -> Result<(), String> {
    const POLL: Duration = Duration::from_millis(500);
    loop {
        let out = run_command("sc", &["query", name], remaining(deadline)).await?;
        let state = parse_sc_state(&String::from_utf8_lossy(&out.stdout));
        if state == Some(wanted) {
            return Ok(());
        }
        if remaining(deadline) <= POLL {
            return Err(format!(
                "service {} did not reach state {} in time (last state {:?})",
                name, wanted, state
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(target_os = "windows")]
async fn restart_one_service(name: &str, deadline: Instant) -> Result<String, String> {
    const STOPPED: u32 = 1;
    const RUNNING: u32 = 4;
    const NOT_ACTIVE: i32 = 1062; // ERROR_SERVICE_NOT_ACTIVE
    const ALREADY_RUNNING: i32 = 1056; // ERROR_SERVICE_ALREADY_RUNNING

    let mut output = String::new();
    let stop = run_command("sc", &["stop", name], remaining(deadline)).await?;
    output.push_str(&format!("sc stop {}: {}", name, command_text(&stop)));
    match stop.status.code() {
        Some(0) | Some(NOT_ACTIVE) => {}
        code => {
            return Err(format!(
                "failed to stop service {} (exit {:?}): {}",
                name,
                code,
                command_text(&stop)
            ))
        }
    }
    // `sc stop` returns while STOP_PENDING; starting now fails with 1056
    wait_for_service_state(name, STOPPED, deadline).await?;

    let start = run_command("sc", &["start", name], remaining(deadline)).await?;
    output.push_str(&format!("sc start {}: {}", name, command_text(&start)));
    match start.status.code() {
        Some(0) | Some(ALREADY_RUNNING) => {}
        code => {
            return Err(format!(
                "failed to start service {} (exit {:?}): {}",
                name,
                code,
                command_text(&start)
            ))
        }
    }
    wait_for_service_state(name, RUNNING, deadline).await?;
    Ok(output)
}

#[cfg(not(target_os = "windows"))]
async fn restart_one_service(name: &str, deadline: Instant) -> Result<String, String> {
    // systemctl restart blocks until the unit is up (or failed)
    let result = run_command("systemctl", &["restart", name], remaining(deadline)).await?;
    if result.status.success() {
        Ok(format!("systemctl restart {} succeeded", name))
    } else {
        Err(format!(
            "systemctl restart {} failed: {}",
            name,
            command_text(&result)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::DeviceSource;

    fn runtime(scripts: &[&str], dir: &str) -> ActionRuntime {
        let config = ActionConfig {
            allowed_scripts: scripts.iter().map(|s| s.to_string()).collect(),
            scripts_dir: dir.to_string(),
            timeout_secs: 30,
            ..ActionConfig::default()
        };
        ActionRuntime::new(&config, true, "edge-1", DeviceRegistry::new())
    }

    fn action(action_type: ActionType, parameters: HashMap<String, String>) -> ExecuteAction {
        ExecuteAction {
            action_id: "test-action".to_string(),
            device_id: "edge-1:daq-1".to_string(),
            action_type,
            parameters,
        }
    }

    fn params(key: &str, value: &str) -> HashMap<String, String> {
        let mut p = HashMap::new();
        p.insert(key.to_string(), value.to_string());
        p
    }

    fn local(daqmx: Option<&str>, simulated: bool) -> LocalDevice {
        LocalDevice {
            daqmx_name: daqmx.map(str::to_string),
            is_simulated: simulated,
            source: DeviceSource::SysCfg,
        }
    }

    #[tokio::test]
    async fn test_reset_rejects_unmanaged_devices() {
        // never touches DAQmx: the device is not managed by this edge
        let rt = runtime(&[], "scripts");
        for id in ["edge-1:daq-1", "other-edge:Dev1", "Dev1"] {
            let mut a = action(ActionType::PowerCycle, HashMap::new());
            a.device_id = id.to_string();
            let result = rt.run(a).await;
            assert!(!result.success);
            assert!(
                result.error.as_deref().unwrap().contains("not managed"),
                "{id}: {:?}",
                result.error
            );
        }
    }

    #[tokio::test]
    async fn test_resolve_local_accepts_full_and_bare_ids() {
        let rt = runtime(&[], "scripts");
        rt.registry
            .insert("edge-1:Dev1".into(), local(Some("Dev1"), false));
        assert!(rt.resolve_local("edge-1:Dev1").is_ok());
        assert!(rt.resolve_local("Dev1").is_ok());
        assert!(rt.resolve_local("edge-2:Dev1").is_err());
        assert!(rt.resolve_local("edge-1:Dev2").is_err());
        // prefix must be followed by ':'
        assert!(rt.resolve_local("edge-10:Dev1").is_err());
    }

    #[tokio::test]
    async fn test_reset_rejects_simulated_and_nameless() {
        let rt = runtime(&[], "scripts");
        rt.registry.insert("edge-1:sim".into(), local(None, true));
        rt.registry
            .insert("edge-1:chassis".into(), local(None, false));
        let mut a = action(ActionType::ResetDriver, HashMap::new());
        a.device_id = "edge-1:sim".into();
        assert!(rt.run(a.clone()).await.error.unwrap().contains("simulated"));
        a.device_id = "edge-1:chassis".into();
        assert!(rt.run(a).await.error.unwrap().contains("no NI-DAQmx"));
    }

    #[tokio::test]
    async fn test_daqmx_disabled_rejects_resets() {
        let rt = ActionRuntime::new(
            &ActionConfig::default(),
            false,
            "edge-1",
            DeviceRegistry::new(),
        );
        rt.registry
            .insert("edge-1:Dev1".into(), local(Some("Dev1"), false));
        let mut a = action(ActionType::PowerCycle, HashMap::new());
        a.device_id = "edge-1:Dev1".into();
        let result = rt.run(a).await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("api.daqmx.enabled"));
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
        let result = rt
            .run(action(
                ActionType::RestartServices,
                params("services", "svc; rm -rf /"),
            ))
            .await;
        assert!(result.error.unwrap().contains("invalid service name"));
    }

    #[tokio::test]
    async fn test_restart_services_denied_without_allowlist() {
        let rt = runtime(&[], "scripts");
        let result = rt
            .run(action(
                ActionType::RestartServices,
                params("services", "Spooler"),
            ))
            .await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("allowed_services is empty"));
    }

    #[tokio::test]
    async fn test_restart_services_rejects_non_allowlisted() {
        let config = ActionConfig {
            allowed_services: vec!["nidevldu".into()],
            ..ActionConfig::default()
        };
        let rt = ActionRuntime::new(&config, true, "edge-1", DeviceRegistry::new());
        let result = rt
            .run(action(
                ActionType::RestartServices,
                params("services", "nidevldu,Spooler"),
            ))
            .await;
        assert!(result
            .error
            .unwrap()
            .contains("'Spooler' is not in action.allowed_services"));
    }

    #[tokio::test]
    async fn test_custom_script_rejects_non_allowlisted() {
        let rt = runtime(&["safe.ps1"], "scripts");
        let result = rt
            .run(action(
                ActionType::CustomScript,
                params("script", "evil.ps1"),
            ))
            .await;
        assert!(result.error.unwrap().contains("not allowlisted"));
    }

    #[tokio::test]
    async fn test_custom_script_rejects_traversal() {
        let rt = runtime(&["..\\evil.ps1"], "scripts");
        let result = rt
            .run(action(
                ActionType::CustomScript,
                params("script", "..\\evil.ps1"),
            ))
            .await;
        assert!(result.error.unwrap().contains("invalid script name"));
    }

    #[tokio::test]
    async fn test_per_device_lock_rejects_concurrent_action() {
        let rt = runtime(&[], "scripts");
        let _held = DeviceLock::acquire(&rt.busy, "edge-1:daq-1").unwrap();
        // valid plan (allowlisted script) but the device is busy
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.ps1"), "exit 0").unwrap();
        let mut rt2 = rt.clone();
        rt2.allowed_scripts = vec!["x.ps1".into()];
        rt2.scripts_dir = dir.path().to_path_buf();
        let result = rt2
            .run(action(ActionType::CustomScript, params("script", "x.ps1")))
            .await;
        assert!(result.error.unwrap().contains("already running"));
        drop(_held);
        assert!(DeviceLock::acquire(&rt.busy, "edge-1:daq-1").is_some());
    }

    #[tokio::test]
    async fn test_global_limit_times_out() {
        let config = ActionConfig {
            timeout_secs: 1,
            allowed_scripts: vec!["x.ps1".into()],
            ..ActionConfig::default()
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.ps1"), "exit 0").unwrap();
        let mut rt = ActionRuntime::new(&config, true, "edge-1", DeviceRegistry::new());
        rt.scripts_dir = dir.path().to_path_buf();
        let _slot = rt.permits.clone().acquire_owned().await.unwrap();
        let result = rt
            .run(action(ActionType::CustomScript, params("script", "x.ps1")))
            .await;
        assert!(result.error.unwrap().contains("edge busy"));
    }

    #[test]
    fn test_parse_delay() {
        assert_eq!(parse_delay(None), 0);
        assert_eq!(parse_delay(Some(&"5".to_string())), 5);
        assert_eq!(parse_delay(Some(&"99999".to_string())), 300);
        assert_eq!(parse_delay(Some(&"x".to_string())), 0);
    }

    fn power_cycle(device: &str, delay: &str) -> ExecuteAction {
        let mut a = action(ActionType::PowerCycle, params("delay_secs", delay));
        a.device_id = device.to_string();
        a
    }

    #[tokio::test]
    async fn test_power_cycle_delay_must_fit_timeout() {
        let config = ActionConfig {
            timeout_secs: 10,
            ..ActionConfig::default()
        };
        let rt = ActionRuntime::new(&config, true, "edge-1", DeviceRegistry::new());
        rt.registry
            .insert("edge-1:Dev1".into(), local(Some("NimonNoSuchDev"), false));
        for delay in ["10", "60", "300"] {
            let r = rt.run(power_cycle("edge-1:Dev1", delay)).await;
            assert!(!r.success);
            let e = r.error.unwrap();
            assert!(
                e.contains("delay_secs") && e.contains("timeout_secs"),
                "{e}"
            );
        }
    }

    #[tokio::test]
    async fn test_power_cycle_delay_holds_no_slot() {
        let config = ActionConfig {
            timeout_secs: 30,
            ..ActionConfig::default()
        };
        let rt = ActionRuntime::new(&config, true, "edge-1", DeviceRegistry::new());
        rt.registry
            .insert("edge-1:Dev1".into(), local(Some("NimonNoSuchDev"), false));
        let rt2 = rt.clone();
        // aborted during the delay: the reset itself never runs
        let task = tokio::spawn(async move { rt2.run(power_cycle("Dev1", "20")).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            rt.permits.available_permits(),
            1,
            "delay must not hold the slot"
        );
        assert!(
            rt.busy.lock().unwrap().contains("edge-1:Dev1"),
            "device stays locked during its delay"
        );
        task.abort();
        let _ = task.await;
        assert!(rt.busy.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_device_lock_uses_resolved_id() {
        let rt = runtime(&[], "scripts");
        rt.registry
            .insert("edge-1:Dev1".into(), local(Some("NimonNoSuchDev"), false));
        // bare and full ids are the same device
        let _held = DeviceLock::acquire(&rt.busy, "edge-1:Dev1").unwrap();
        let r = rt.run(power_cycle("Dev1", "0")).await;
        assert!(r.error.unwrap().contains("already running"));
        drop(_held);
        let _held = DeviceLock::acquire(&rt.busy, "edge-1:Dev1").unwrap();
        let r = rt.run(power_cycle("edge-1:Dev1", "0")).await;
        assert!(r.error.unwrap().contains("already running"));
    }

    #[tokio::test]
    async fn test_custom_script_rejects_drive_relative_and_streams() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.ps1"), "exit 0").unwrap();
        for name in ["C:evil.ps1", "x.ps1:stream", ".", "..", "a/b.ps1"] {
            let mut rt = runtime(&[name], "unused");
            rt.scripts_dir = dir.path().to_path_buf();
            let r = rt
                .run(action(ActionType::CustomScript, params("script", name)))
                .await;
            let e = r.error.unwrap();
            assert!(e.contains("invalid script name"), "{name}: {e}");
        }
        assert!(validate_script_name("reset-relays.ps1").is_ok());
        assert!(validate_script_name("..x.ps1").is_ok(), "just a file name");
    }

    #[tokio::test]
    async fn test_custom_script_symlink_outside_dir_rejected() {
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("evil.ps1");
        std::fs::write(&target, "exit 0").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("safe.ps1");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &link);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link);
        if made.is_err() {
            eprintln!("symlinks not permitted here; skipping");
            return;
        }
        let mut rt = runtime(&["safe.ps1"], "unused");
        rt.scripts_dir = dir.path().to_path_buf();
        let r = rt
            .run(action(
                ActionType::CustomScript,
                params("script", "safe.ps1"),
            ))
            .await;
        assert!(r.error.unwrap().contains("outside action.scripts_dir"));
    }

    #[tokio::test]
    async fn test_background_process_holding_output_does_not_hang() {
        // the process exits at once, leaving a child that holds the
        // inherited stdout/stderr pipes open for ~13 s
        #[cfg(windows)]
        let cmd = {
            let mut c = tokio::process::Command::new("cmd");
            c.raw_arg("/C echo started& start /B ping -n 14 127.0.0.1");
            c
        };
        #[cfg(not(windows))]
        let cmd = {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", "echo started; sleep 14 &"]);
            c
        };
        let started = Instant::now();
        let (success, out, err, code) = run_captured(cmd, Duration::from_secs(30)).await;
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "took {:?}",
            started.elapsed()
        );
        assert!(success, "{err}");
        assert_eq!(code, Some(0));
        assert!(out.contains("started"), "{out}");
        assert!(out.contains("output incomplete"), "{out}");
    }

    #[test]
    fn test_parse_sc_state() {
        let running = "SERVICE_NAME: Spooler \r\n        TYPE               : 110  WIN32_OWN_PROCESS\r\n        STATE              : 4  RUNNING\r\n                                (STOPPABLE)\r\n        WIN32_EXIT_CODE    : 0  (0x0)\r\n";
        assert_eq!(parse_sc_state(running), Some(4));
        let pending = "        TYPE               : 10  WIN32_OWN_PROCESS\n        STATE              : 3  STOP_PENDING\n";
        assert_eq!(parse_sc_state(pending), Some(3));
        // localized label: fall back to the second numeric field
        let localized = "DIENST_NAME: x\n        TYP                : 10  WIN32\n        STATUS             : 1  STOPPED\n";
        assert_eq!(parse_sc_state(localized), Some(1));
        assert_eq!(parse_sc_state("garbage"), None);
    }

    #[tokio::test]
    async fn test_read_capped_truncates() {
        let data: &[u8] = &[b'a'; 10_000];
        let text = read_capped(Some(data), 100).await;
        assert!(text.starts_with(&"a".repeat(100)));
        assert!(text.ends_with("[output truncated]"));
        assert_eq!(read_capped(None::<&[u8]>, 10).await, "");
    }

    #[tokio::test]
    async fn test_custom_script_runs_allowlisted() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(target_os = "windows")]
        let (name, body) = ("echo.ps1", "Write-Output 'hello-script'\n");
        #[cfg(not(target_os = "windows"))]
        let (name, body) = ("echo.sh", "#!/bin/sh\necho hello-script\n");
        std::fs::write(dir.path().join(name), body).unwrap();

        let mut rt = runtime(&[name], "unused");
        rt.scripts_dir = dir.path().to_path_buf();
        let result = rt
            .run(action(ActionType::CustomScript, params("script", name)))
            .await;
        assert!(result.success, "{:?}", result.error);
        assert_eq!(result.output.unwrap().trim(), "hello-script");
        assert_eq!(result.exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_script_timeout_kills_process_tree() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("child.pid");
        #[cfg(target_os = "windows")]
        let (name, body) = (
            "hang.ps1",
            format!(
                "$p = Start-Process -FilePath powershell -ArgumentList '-NoProfile','-Command','Start-Sleep 60' -WindowStyle Hidden -PassThru\n\
                 Set-Content -Path '{}' -Value $p.Id\n\
                 Start-Sleep 60\n",
                pid_file.display()
            ),
        );
        #[cfg(not(target_os = "windows"))]
        let (name, body) = (
            "hang.sh",
            format!("sleep 60 &\necho $! > '{}'\nsleep 60\n", pid_file.display()),
        );
        std::fs::write(dir.path().join(name), body).unwrap();

        let config = ActionConfig {
            allowed_scripts: vec![name.to_string()],
            scripts_dir: dir.path().display().to_string(),
            timeout_secs: 5,
            ..ActionConfig::default()
        };
        let rt = ActionRuntime::new(&config, true, "edge-1", DeviceRegistry::new());
        let started = Instant::now();
        let result = rt
            .run(action(ActionType::CustomScript, params("script", name)))
            .await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(30));

        // the grandchild must be gone too
        let child_pid: u32 = match std::fs::read_to_string(&pid_file) {
            Ok(s) => s.trim().parse().unwrap(),
            Err(_) => return, // script was killed before it wrote the pid
        };
        tokio::time::sleep(Duration::from_millis(500)).await;
        #[cfg(target_os = "windows")]
        {
            let out = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {child_pid}"), "/NH", "/FO", "CSV"])
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(
                !text.contains(&format!("\"{child_pid}\"")),
                "grandchild {child_pid} still running: {text}"
            );
        }
        #[cfg(not(target_os = "windows"))]
        {
            let alive = std::process::Command::new("kill")
                .args(["-0", &child_pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            assert!(!alive, "grandchild {child_pid} still running");
        }
    }
}
