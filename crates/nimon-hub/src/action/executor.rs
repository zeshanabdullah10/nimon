//! Action executor for self-healing remediation
//!
//! Executes remediation actions when alerts fire, with support for
//! scripts, service restarts, edge commands, and power cycling.

use actix::prelude::*;
use chrono::Utc;
use dashmap::DashMap;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command as TokioCommand;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use nimon_core::actor::messages::{
    ActionType as WireActionType, ExecuteAction as WireExecuteAction,
};
use nimon_core::alert::Alert;
use nimon_core::db::ActionRepository;
use nimon_core::protocol::WsMessage;

use super::actions::{
    map_edge_command, Action, ActionResult, ActionStatus, ActionType, ScriptAction,
};

use crate::db_writer::DbWriter;
use crate::session::SessionStore;

/// How often finished entries are dropped from `running_actions`
const CLEANUP_INTERVAL: Duration = Duration::from_secs(600);

/// Context in which an action is being executed
#[derive(Debug, Clone)]
pub struct ActionContext {
    /// The edge node ID where the alert originated
    pub edge_id: String,
    /// The device ID that triggered the alert
    pub device_id: String,
    /// The alert that triggered this action (empty for manual actions)
    pub alert_id: String,
    /// Current attempt number (1-indexed)
    pub attempt: u32,
    /// Template variables for `${VAR}` substitution
    pub variables: HashMap<String, String>,
}

impl ActionContext {
    /// Create a new action context
    pub fn new(
        edge_id: impl Into<String>,
        device_id: impl Into<String>,
        alert_id: impl Into<String>,
    ) -> Self {
        Self {
            edge_id: edge_id.into(),
            device_id: device_id.into(),
            alert_id: alert_id.into(),
            attempt: 1,
            variables: HashMap::new(),
        }
    }

    /// Context for an action triggered by `alert`, with the template
    /// variables ALERT_ID, RULE_ID, DEVICE_ID, EDGE_ID, SEVERITY, METRIC,
    /// VALUE and THRESHOLD filled (empty strings when unknown).
    pub fn for_alert(alert: &Alert) -> Self {
        let fmt = |v: Option<f64>| v.map(|v| v.to_string()).unwrap_or_default();
        Self::new(&alert.edge_id, &alert.device_id, &alert.id)
            .with_variable("ALERT_ID", &alert.id)
            .with_variable("RULE_ID", &alert.rule_id)
            .with_variable("DEVICE_ID", &alert.device_id)
            .with_variable("EDGE_ID", &alert.edge_id)
            .with_variable("SEVERITY", alert.severity.to_string())
            .with_variable("METRIC", alert.metric_name.clone().unwrap_or_default())
            .with_variable("VALUE", fmt(alert.metric_value))
            .with_variable("THRESHOLD", fmt(alert.threshold))
    }

    /// Set the current attempt number
    pub fn with_attempt(mut self, attempt: u32) -> Self {
        self.attempt = attempt;
        self
    }

    /// Add a variable for template substitution
    pub fn with_variable(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.variables.insert(key.into(), value.into());
        self
    }
}

/// An edge action awaiting its `action_result`.
struct PendingResult {
    tx: oneshot::Sender<nimon_core::actor::messages::ActionResult>,
    /// Wire action id (fallback correlation for peers that set neither
    /// `reply_to` nor echo the request `msg_id`)
    action_id: String,
    /// Edge the request was dispatched to: only that edge may complete it
    edge_id: String,
}

/// Identity of an action run for overlap suppression: (action, edge, device)
type RunKey = (String, String, String);

/// Removes its key from the in-flight set when the run ends (also on
/// cancellation or panic).
struct InFlightGuard {
    set: Arc<DashMap<RunKey, ()>>,
    key: RunKey,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.set.remove(&self.key);
    }
}

/// Action executor actor
///
/// Manages the execution of self-healing actions with retry logic,
/// timeout handling, and tracking of running actions. Edge-routed
/// actions are dispatched over the edge WebSocket session and their
/// results correlated by the request `msg_id` (the edge reply's
/// `reply_to`, see [`WsMessage::correlation_id`]).
pub struct ActionExecutor {
    /// Currently running actions tracked by action ID.
    /// Shared with worker clones (`Arc`) so completion state recorded by
    /// the handler's clone is visible to the actor.
    running_actions: Arc<DashMap<String, ActionStatus>>,
    /// Pending edge command results, keyed by the sent request `msg_id`.
    pending_results: Arc<DashMap<String, PendingResult>>,
    /// Runs in progress per (action, edge, device): a re-fire does not
    /// start a second concurrent run of the same remediation
    in_flight: Arc<DashMap<RunKey, ()>>,
    /// Connected edge sessions for edge-routed actions
    sessions: Option<std::sync::Arc<SessionStore>>,
    /// Database pool for persisting action history
    db_pool: Option<SqlitePool>,
    /// Ordered writer shared with the rest of the hub
    db_writer: Option<DbWriter>,
}

impl ActionExecutor {
    /// Create a new action executor
    pub fn new() -> Self {
        Self {
            running_actions: Arc::new(DashMap::new()),
            pending_results: Arc::new(DashMap::new()),
            in_flight: Arc::new(DashMap::new()),
            sessions: None,
            db_pool: None,
            db_writer: None,
        }
    }

    /// Set the session store used to route actions to edges.
    pub fn with_sessions(mut self, sessions: std::sync::Arc<SessionStore>) -> Self {
        self.sessions = Some(sessions);
        self
    }

    /// Set the database pool for persisting action history (a private
    /// ordered writer is started with the actor).
    pub fn with_db_pool(mut self, pool: SqlitePool) -> Self {
        self.db_pool = Some(pool);
        self
    }

    /// Use a shared ordered writer (and its pool) for persistence.
    pub fn with_db_writer(mut self, writer: DbWriter) -> Self {
        self.db_pool = Some(writer.pool().clone());
        self.db_writer = Some(writer);
        self
    }

    /// Execute an action with retry logic. Retries happen only on genuine
    /// failures: a timeout (e.g. no edge result in time — the edge may
    /// still be running it) is never retried.
    pub async fn execute_action(&self, action: &Action, ctx: &ActionContext) -> ActionResult {
        if !action.enabled {
            return ActionResult {
                action_id: action.id.clone(),
                status: ActionStatus::Cancelled,
                exit_code: None,
                output: String::new(),
                error: "Action is disabled".to_string(),
                duration_ms: 0,
                attempt: 0,
                executed_at: Utc::now(),
            };
        }

        self.running_actions
            .insert(action.id.clone(), ActionStatus::Running);

        let max_attempts = action.retry_config.max_attempts.max(1);
        let mut attempt = 1;
        let result = loop {
            let attempt_ctx = ActionContext {
                attempt,
                ..ctx.clone()
            };
            info!(
                "Executing action {} (attempt {}/{})",
                action.id, attempt, max_attempts
            );
            let result = self.execute_action_internal(action, &attempt_ctx).await;
            match result.status {
                ActionStatus::Failed if attempt < max_attempts => {
                    warn!(
                        "Action {} attempt {}/{} failed: {}",
                        action.id, attempt, max_attempts, result.error
                    );
                    let delay_ms = (action.retry_config.delay_ms as f64
                        * action
                            .retry_config
                            .backoff_multiplier
                            .powi(attempt as i32 - 1)) as u64;
                    debug!("Retrying action {} in {}ms", action.id, delay_ms);
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    attempt += 1;
                }
                ActionStatus::Failed | ActionStatus::Timeout => {
                    warn!(
                        "Action {} gave up after attempt {}/{} ({:?}): {}",
                        action.id, attempt, max_attempts, result.status, result.error
                    );
                    break result;
                }
                _ => break result,
            }
        };

        self.running_actions
            .insert(action.id.clone(), result.status.clone());
        result
    }

    /// Execute a single attempt of an action
    async fn execute_action_internal(&self, action: &Action, ctx: &ActionContext) -> ActionResult {
        let start = Instant::now();

        match &action.action_type {
            ActionType::Script(script_config) => {
                self.execute_script(action, ctx, script_config).await
            }
            ActionType::RestartService { service_name } => {
                self.execute_restart(action, ctx, service_name).await
            }
            ActionType::EdgeCommand {
                command,
                parameters,
            } => {
                let (wire, params) = map_edge_command(command, parameters);
                self.execute_edge_routed(action, ctx, wire, params).await
            }
            ActionType::PowerCycle { delay_secs } => {
                let mut params = HashMap::new();
                params.insert("delay_secs".to_string(), delay_secs.to_string());
                self.execute_edge_routed(action, ctx, WireActionType::PowerCycle, params)
                    .await
            }
        }
        .map(|mut result| {
            result.duration_ms = start.elapsed().as_millis() as u64;
            result.attempt = ctx.attempt;
            result.executed_at = Utc::now();
            result
        })
        .unwrap_or_else(|e| ActionResult {
            action_id: action.id.clone(),
            status: ActionStatus::Failed,
            exit_code: None,
            output: String::new(),
            error: e.to_string(),
            duration_ms: start.elapsed().as_millis() as u64,
            attempt: ctx.attempt,
            executed_at: Utc::now(),
        })
    }

    /// Execute a script action (argv only, never through a shell). On
    /// timeout the process is killed (on Windows the whole process tree
    /// via `taskkill /T /F`).
    async fn execute_script(
        &self,
        action: &Action,
        ctx: &ActionContext,
        script_config: &ScriptAction,
    ) -> std::result::Result<ActionResult, anyhow::Error> {
        if let Some(user) = &script_config.run_as {
            anyhow::bail!(
                "run_as '{}' is not supported; refusing to run the script",
                user
            );
        }

        let script = substitute_variables(&script_config.script, &ctx.variables);
        let args: Vec<String> = script_config
            .args
            .iter()
            .map(|a| substitute_variables(a, &ctx.variables))
            .collect();

        debug!("Running script: {} with args {:?}", script, args);

        let mut cmd = TokioCommand::new(&script);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in &script_config.env {
            cmd.env(key, substitute_variables(value, &ctx.variables));
        }
        if let Some(ref dir) = script_config.working_dir {
            cmd.current_dir(substitute_variables(dir, &ctx.variables));
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to execute script: {}", e))?;
        let pid = child.id();
        let (out_task, out_buf) = spawn_pipe_reader(child.stdout.take());
        let (err_task, err_buf) = spawn_pipe_reader(child.stderr.take());

        let timeout = Duration::from_secs(action.timeout_secs.max(1));
        let deadline = tokio::time::Instant::now() + timeout;
        let status = match tokio::time::timeout_at(deadline, child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => {
                out_task.abort();
                err_task.abort();
                anyhow::bail!("Failed to wait for script: {}", e);
            }
            Err(_) => {
                warn!(
                    "Script {} timed out after {}s; killing it",
                    script, action.timeout_secs
                );
                if let Some(pid) = pid {
                    kill_process_tree(pid).await;
                }
                let _ = child.kill().await;
                out_task.abort();
                err_task.abort();
                return Ok(ActionResult {
                    action_id: action.id.clone(),
                    status: ActionStatus::Timeout,
                    exit_code: None,
                    output: String::new(),
                    error: format!("Action timed out after {}s", action.timeout_secs),
                    duration_ms: 0,
                    attempt: ctx.attempt,
                    executed_at: Utc::now(),
                });
            }
        };

        // The pipes reach EOF only when every holder closed them: a
        // detached grandchild (`start /b`, `cmd &`) may keep them open
        // forever. Collect output for a short grace after exit, bounded by
        // the action deadline, then stop reading.
        let grace_end = (tokio::time::Instant::now() + PIPE_GRACE_AFTER_EXIT).min(deadline);
        let (out_abort, err_abort) = (out_task.abort_handle(), err_task.abort_handle());
        let drained = tokio::time::timeout_at(grace_end, async {
            let _ = out_task.await;
            let _ = err_task.await;
        })
        .await
        .is_ok();
        if !drained {
            out_abort.abort();
            err_abort.abort();
            warn!(
                "Script {} exited but its output pipes are still held open (detached child process?); \
                 output may be incomplete",
                script
            );
        }
        let stdout = take_pipe_output(&out_buf);
        let stderr = take_pipe_output(&err_buf);
        let succeeded = status.success();

        Ok(ActionResult {
            action_id: action.id.clone(),
            status: if succeeded {
                ActionStatus::Succeeded
            } else {
                ActionStatus::Failed
            },
            exit_code: status.code(),
            output: stdout,
            error: if succeeded { String::new() } else { stderr },
            duration_ms: 0, // Set by caller
            attempt: ctx.attempt,
            executed_at: Utc::now(),
        })
    }

    /// Execute a service restart action
    async fn execute_restart(
        &self,
        action: &Action,
        ctx: &ActionContext,
        service_name: &str,
    ) -> std::result::Result<ActionResult, anyhow::Error> {
        // Validate service name - alphanumeric, hyphens, underscores, dots only
        if service_name.is_empty()
            || !service_name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Ok(ActionResult {
                action_id: action.id.clone(),
                status: ActionStatus::Failed,
                exit_code: None,
                output: String::new(),
                error: format!("Invalid service name: {}", service_name),
                duration_ms: 0,
                attempt: ctx.attempt,
                executed_at: chrono::Utc::now(),
            });
        }

        #[cfg(target_os = "windows")]
        let result = {
            let stop_output = tokio::time::timeout(
                Duration::from_secs(30),
                TokioCommand::new("sc")
                    .args(["stop", service_name])
                    .output(),
            )
            .await
            .map_err(|_| anyhow::anyhow!("Timed out stopping service after 30s"))?
            .map_err(|e| anyhow::anyhow!("Failed to stop service: {}", e))?;

            // `sc stop` only requests the stop: wait for STOPPED before
            // starting, otherwise `sc start` fails with 1056.
            wait_for_service_stopped(service_name, Duration::from_secs(30)).await?;

            let start_output = tokio::time::timeout(
                Duration::from_secs(30),
                TokioCommand::new("sc")
                    .args(["start", service_name])
                    .output(),
            )
            .await
            .map_err(|_| anyhow::anyhow!("Timed out starting service after 30s"))?
            .map_err(|e| anyhow::anyhow!("Failed to start service: {}", e))?;

            let stdout = String::from_utf8_lossy(&stop_output.stdout).to_string()
                + &*String::from_utf8_lossy(&start_output.stdout);
            let stderr = String::from_utf8_lossy(&stop_output.stderr).to_string()
                + &*String::from_utf8_lossy(&start_output.stderr);

            let succeeded = start_output.status.success();
            (stdout, stderr, succeeded, start_output.status.code())
        };

        #[cfg(not(target_os = "windows"))]
        let result = {
            let output = tokio::time::timeout(
                Duration::from_secs(30),
                TokioCommand::new("systemctl")
                    .args(["restart", service_name])
                    .output(),
            )
            .await
            .map_err(|_| anyhow::anyhow!("Timed out restarting service after 30s"))?
            .map_err(|e| anyhow::anyhow!("Failed to restart service: {}", e))?;

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let succeeded = output.status.success();
            (stdout, stderr, succeeded, output.status.code())
        };

        let (stdout, stderr, succeeded, exit_code) = result;

        Ok(ActionResult {
            action_id: action.id.clone(),
            status: if succeeded {
                ActionStatus::Succeeded
            } else {
                ActionStatus::Failed
            },
            exit_code,
            output: stdout,
            error: if succeeded { String::new() } else { stderr },
            duration_ms: 0, // Set by caller
            attempt: ctx.attempt,
            executed_at: Utc::now(),
        })
    }

    /// Route an action to the edge node owning the device and await the
    /// correlated `action_result` (with timeout). A missing result within
    /// the timeout yields `Timeout` (not retried).
    async fn execute_edge_routed(
        &self,
        action: &Action,
        ctx: &ActionContext,
        wire_type: WireActionType,
        parameters: HashMap<String, String>,
    ) -> std::result::Result<ActionResult, anyhow::Error> {
        let failed = |error: String| ActionResult {
            action_id: action.id.clone(),
            status: ActionStatus::Failed,
            exit_code: None,
            output: String::new(),
            error,
            duration_ms: 0,
            attempt: ctx.attempt,
            executed_at: Utc::now(),
        };

        let Some(sessions) = &self.sessions else {
            return Ok(failed(
                "Edge routing unavailable: no session store".to_string(),
            ));
        };
        let Some(session) = sessions.get(&ctx.edge_id) else {
            return Ok(failed(format!("Edge '{}' is not connected", ctx.edge_id)));
        };

        let wire_action = WireExecuteAction {
            action_id: action.id.clone(),
            device_id: ctx.device_id.clone(),
            action_type: wire_type,
            parameters,
        };
        let msg = WsMessage::execute_action(wire_action);
        let msg_id = msg.msg_id.clone();

        let (tx, rx) = oneshot::channel();
        self.pending_results.insert(
            msg_id.clone(),
            PendingResult {
                tx,
                action_id: action.id.clone(),
                edge_id: ctx.edge_id.clone(),
            },
        );

        if let Err(e) = session.send(msg) {
            self.pending_results.remove(&msg_id);
            return Ok(failed(format!("Failed to dispatch action to edge: {}", e)));
        }

        debug!(
            "Action {} dispatched to edge {} (msg {})",
            action.id, ctx.edge_id, msg_id
        );

        let timeout = Duration::from_secs(action.timeout_secs.max(1));
        let outcome = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => return Ok(failed("Edge connection dropped before result".to_string())),
            Err(_) => {
                self.pending_results.remove(&msg_id);
                return Ok(ActionResult {
                    status: ActionStatus::Timeout,
                    ..failed(format!(
                        "Timed out waiting for edge result after {}s",
                        action.timeout_secs
                    ))
                });
            }
        };

        Ok(ActionResult {
            action_id: action.id.clone(),
            status: if outcome.success {
                ActionStatus::Succeeded
            } else {
                ActionStatus::Failed
            },
            exit_code: outcome.exit_code,
            output: outcome.output.unwrap_or_default(),
            error: outcome.error.unwrap_or_default(),
            duration_ms: 0, // Set by caller
            attempt: ctx.attempt,
            executed_at: Utc::now(),
        })
    }

    /// Deliver an `action_result` sent by `edge_id` (the registered edge of
    /// the connection it arrived on). `correlation_id` is the reply's
    /// `reply_to` (or its own `msg_id` for peers that echo the request id).
    /// When no request matches, a unique pending request of the same edge
    /// with the same `action_id` is used. Only requests dispatched to
    /// `edge_id` can be completed. Returns true when a waiter was completed.
    pub fn complete(
        &self,
        edge_id: &str,
        correlation_id: &str,
        result: nimon_core::actor::messages::ActionResult,
    ) -> bool {
        let entry = self
            .pending_results
            .remove_if(correlation_id, |_, p| p.edge_id == edge_id)
            .or_else(|| {
                let matches: Vec<String> = self
                    .pending_results
                    .iter()
                    .filter(|e| {
                        e.value().edge_id == edge_id && e.value().action_id == result.action_id
                    })
                    .map(|e| e.key().clone())
                    .collect();
                match matches.as_slice() {
                    [only] => self
                        .pending_results
                        .remove_if(only, |_, p| p.edge_id == edge_id),
                    _ => None,
                }
            });
        match entry {
            Some((_, pending)) => {
                let _ = pending.tx.send(result);
                true
            }
            None => {
                if self
                    .pending_results
                    .get(correlation_id)
                    .is_some_and(|p| p.edge_id != edge_id)
                {
                    warn!(
                        "Ignoring action_result from edge {} for request {} dispatched to another edge",
                        edge_id, correlation_id
                    );
                } else {
                    debug!(
                        "action_result from edge {} for unknown request {} (action {})",
                        edge_id, correlation_id, result.action_id
                    );
                }
                false
            }
        }
    }

    /// The connection of `edge_id` is gone: fail its pending edge actions
    /// now ("Edge connection dropped before result") instead of letting
    /// them wait for their full timeout. Returns how many were failed.
    pub fn fail_edge(&self, edge_id: &str) -> usize {
        let keys: Vec<String> = self
            .pending_results
            .iter()
            .filter(|e| e.value().edge_id == edge_id)
            .map(|e| e.key().clone())
            .collect();
        let mut failed = 0;
        for key in keys {
            // Dropping the sender wakes the waiter with a channel error
            if self
                .pending_results
                .remove_if(&key, |_, p| p.edge_id == edge_id)
                .is_some()
            {
                failed += 1;
            }
        }
        if failed > 0 {
            info!(
                "Edge {} disconnected: failed {} pending edge action(s)",
                edge_id, failed
            );
        }
        failed
    }

    /// Claim the (action, edge, device) run slot; None while a run of the
    /// same action for the same device is still in progress.
    fn try_begin_run(&self, action: &Action, ctx: &ActionContext) -> Option<InFlightGuard> {
        use dashmap::mapref::entry::Entry;
        let key = (
            action.id.clone(),
            ctx.edge_id.clone(),
            ctx.device_id.clone(),
        );
        match self.in_flight.entry(key.clone()) {
            Entry::Occupied(_) => None,
            Entry::Vacant(v) => {
                v.insert(());
                Some(InFlightGuard {
                    set: Arc::clone(&self.in_flight),
                    key,
                })
            }
        }
    }

    /// Result returned for a dispatch skipped because a run is in flight
    fn skipped(action: &Action, ctx: &ActionContext) -> ActionResult {
        info!(
            "Action {} for device {} (edge {}) is still running; skipping this dispatch",
            action.id, ctx.device_id, ctx.edge_id
        );
        ActionResult {
            action_id: action.id.clone(),
            status: ActionStatus::Cancelled,
            exit_code: None,
            output: String::new(),
            error: "Skipped: a previous run of this action for the device is still in progress"
                .to_string(),
            duration_ms: 0,
            attempt: 0,
            executed_at: Utc::now(),
        }
    }

    /// Check if an action is currently running
    pub fn is_running(&self, action_id: &str) -> bool {
        self.running_actions
            .get(action_id)
            .map(|s| *s == ActionStatus::Running)
            .unwrap_or(false)
    }

    /// Get the status of an action
    pub fn get_status(&self, action_id: &str) -> Option<ActionStatus> {
        self.running_actions.get(action_id).map(|s| s.clone())
    }

    /// Clear completed actions from tracking
    pub fn cleanup(&self) {
        self.running_actions
            .retain(|_, status| *status == ActionStatus::Running);
    }

    /// Number of edge actions waiting for a result
    pub fn pending_count(&self) -> usize {
        self.pending_results.len()
    }
}

/// How long pipe output is still collected after a script exited
const PIPE_GRACE_AFTER_EXIT: Duration = Duration::from_secs(2);
/// Output kept per pipe (the rest is discarded)
const MAX_PIPE_OUTPUT: usize = 1024 * 1024;

type PipeBuffer = Arc<std::sync::Mutex<Vec<u8>>>;

/// Read a child pipe into a shared buffer (so partial output survives
/// aborting the reader).
fn spawn_pipe_reader<R>(pipe: Option<R>) -> (tokio::task::JoinHandle<()>, PipeBuffer)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let buf: PipeBuffer = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&buf);
    let task = tokio::spawn(async move {
        let Some(mut pipe) = pipe else {
            return;
        };
        let mut chunk = [0u8; 4096];
        loop {
            match pipe.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut out = sink.lock().unwrap_or_else(|e| e.into_inner());
                    let room = MAX_PIPE_OUTPUT.saturating_sub(out.len());
                    out.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
    });
    (task, buf)
}

fn take_pipe_output(buf: &PipeBuffer) -> String {
    let bytes = std::mem::take(&mut *buf.lock().unwrap_or_else(|e| e.into_inner()));
    String::from_utf8_lossy(&bytes).to_string()
}

/// Kill a process and its children (Windows: `taskkill /T /F`).
async fn kill_process_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            TokioCommand::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status(),
        )
        .await;
        if !matches!(result, Ok(Ok(ref s)) if s.success()) {
            debug!("taskkill for pid {} did not succeed: {:?}", pid, result);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Only the direct child is killed (by the caller); descendants
        // that detached from it are not tracked.
        let _ = pid;
    }
}

/// Poll `sc query` until the service reports STOPPED.
#[cfg(target_os = "windows")]
async fn wait_for_service_stopped(service_name: &str, timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let output = TokioCommand::new("sc")
            .args(["query", service_name])
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to query service: {}", e))?;
        let text = String::from_utf8_lossy(&output.stdout);
        if text.contains("STOPPED") {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "Timed out after {}s waiting for service {} to stop",
                timeout.as_secs(),
                service_name
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

impl Default for ActionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ActionExecutor {
    fn clone(&self) -> Self {
        Self {
            running_actions: Arc::clone(&self.running_actions),
            pending_results: Arc::clone(&self.pending_results),
            in_flight: Arc::clone(&self.in_flight),
            sessions: self.sessions.clone(),
            db_pool: self.db_pool.clone(),
            db_writer: self.db_writer.clone(),
        }
    }
}

impl Actor for ActionExecutor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        if self.db_writer.is_none() {
            if let Some(pool) = &self.db_pool {
                self.db_writer = Some(DbWriter::spawn(pool.clone()));
            }
        }
        ctx.run_interval(CLEANUP_INTERVAL, |actor, _| actor.cleanup());
        info!("ActionExecutor actor started");
    }
}

/// Message to execute an action
#[derive(Message)]
#[rtype(result = "ActionResult")]
pub struct ExecuteAction {
    pub action: Action,
    pub context: ActionContext,
}

impl Handler<ExecuteAction> for ActionExecutor {
    type Result = ResponseFuture<ActionResult>;

    fn handle(&mut self, msg: ExecuteAction, _ctx: &mut Self::Context) -> Self::Result {
        let executor = self.clone();
        let Some(guard) = executor.try_begin_run(&msg.action, &msg.context) else {
            let skipped = Self::skipped(&msg.action, &msg.context);
            return Box::pin(async move { skipped });
        };
        Box::pin(async move {
            let _guard = guard;
            let result = executor.execute_action(&msg.action, &msg.context).await;
            executor.persist_result(&msg.action, &msg.context, &result);
            executor.update_alert_action(&msg.context, &msg.action, &result, None);
            result
        })
    }
}

/// Execute a rule's primary action; when it still fails (or times out)
/// after its retries, execute `on_failure`. Returns the primary result.
#[derive(Message)]
#[rtype(result = "ActionResult")]
pub struct ExecuteRuleAction {
    pub action: Action,
    pub on_failure: Option<Action>,
    pub context: ActionContext,
}

impl Handler<ExecuteRuleAction> for ActionExecutor {
    type Result = ResponseFuture<ActionResult>;

    fn handle(&mut self, msg: ExecuteRuleAction, _ctx: &mut Self::Context) -> Self::Result {
        let executor = self.clone();
        // The primary action and its on_failure fallback form one run
        let Some(guard) = executor.try_begin_run(&msg.action, &msg.context) else {
            let skipped = Self::skipped(&msg.action, &msg.context);
            return Box::pin(async move { skipped });
        };
        Box::pin(async move {
            let _guard = guard;
            let result = executor.execute_action(&msg.action, &msg.context).await;
            executor.persist_result(&msg.action, &msg.context, &result);
            let failed = matches!(result.status, ActionStatus::Failed | ActionStatus::Timeout);
            match (failed, msg.on_failure) {
                (true, Some(fallback)) => {
                    warn!(
                        "Action {} failed; running on_failure action {}",
                        msg.action.id, fallback.id
                    );
                    let fallback_result = executor.execute_action(&fallback, &msg.context).await;
                    executor.persist_result(&fallback, &msg.context, &fallback_result);
                    executor.update_alert_action(
                        &msg.context,
                        &msg.action,
                        &result,
                        Some((&fallback, &fallback_result)),
                    );
                }
                _ => executor.update_alert_action(&msg.context, &msg.action, &result, None),
            }
            result
        })
    }
}

/// An `action_result` arrived from an edge node. `edge_id` is the
/// registered edge of the connection it arrived on (never a payload
/// field); `msg_id` is the correlation id: the reply's `reply_to`, or its
/// own `msg_id` when the peer echoes the request id instead.
#[derive(Message)]
#[rtype(result = "()")]
pub struct CompleteAction {
    pub edge_id: String,
    pub msg_id: String,
    pub result: nimon_core::actor::messages::ActionResult,
}

impl Handler<CompleteAction> for ActionExecutor {
    type Result = ();

    fn handle(&mut self, msg: CompleteAction, _ctx: &mut Self::Context) -> Self::Result {
        self.complete(&msg.edge_id, &msg.msg_id, msg.result);
    }
}

/// The edge's connection is gone: fail its pending edge actions now.
#[derive(Message)]
#[rtype(result = "usize")]
pub struct EdgeDisconnected {
    pub edge_id: String,
}

impl Handler<EdgeDisconnected> for ActionExecutor {
    type Result = usize;

    fn handle(&mut self, msg: EdgeDisconnected, _ctx: &mut Self::Context) -> Self::Result {
        self.fail_edge(&msg.edge_id)
    }
}

impl ActionExecutor {
    fn submit_write<F, Fut>(&self, op: F)
    where
        F: FnOnce(SqlitePool) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        if let Some(writer) = &self.db_writer {
            writer.submit(op);
        } else if let Some(pool) = &self.db_pool {
            let pool = pool.clone();
            tokio::spawn(op(pool));
        }
    }

    /// Persist an execution outcome to `action_history`.
    fn persist_result(&self, action: &Action, ctx: &ActionContext, result: &ActionResult) {
        let alert_id = (!ctx.alert_id.is_empty()).then(|| ctx.alert_id.clone());
        let output = (!result.output.is_empty()).then(|| result.output.clone());
        let command = match &action.action_type {
            ActionType::Script(s) => Some(s.script.clone()),
            ActionType::RestartService { service_name } => Some(service_name.clone()),
            ActionType::EdgeCommand { command, .. } => Some(command.clone()),
            ActionType::PowerCycle { .. } => None,
        };
        let device_id = ctx.device_id.clone();
        let action_id = action.id.clone();
        let action_type = action.action_type.to_string();
        let exit_code = result.exit_code.map(|c| c as i64);
        let duration_ms = result.duration_ms as i64;
        let success = result.status == ActionStatus::Succeeded;
        let retry_count = (result.attempt as i64 - 1).max(0);
        self.submit_write(move |pool| async move {
            let repo = ActionRepository::new(&pool);
            if let Err(e) = repo
                .insert(
                    alert_id,
                    &device_id,
                    &action_id,
                    &action_type,
                    command.as_deref(),
                    exit_code,
                    output.as_deref(),
                    Some(duration_ms),
                    success,
                    retry_count,
                )
                .await
            {
                warn!("Failed to persist action history: {}", e);
            }
        });
    }

    /// Record the outcome on the triggering alert.
    fn update_alert_action(
        &self,
        ctx: &ActionContext,
        action: &Action,
        result: &ActionResult,
        fallback: Option<(&Action, &ActionResult)>,
    ) {
        if ctx.alert_id.is_empty() {
            return;
        }
        let describe = |r: &ActionResult| {
            if r.status == ActionStatus::Succeeded {
                format!("success (exit {:?}, {}ms)", r.exit_code, r.duration_ms)
            } else {
                format!("{:?}: {}", r.status, r.error).to_lowercase()
            }
        };
        let (taken, outcome) = match fallback {
            None => (action.action_type.to_string(), describe(result)),
            Some((fb, fb_result)) => (
                format!("{} -> {}", action.action_type, fb.action_type),
                format!("{}; fallback {}", describe(result), describe(fb_result)),
            ),
        };
        let alert_id = ctx.alert_id.clone();
        self.submit_write(move |pool| async move {
            let repo = nimon_core::db::AlertRepository::new(&pool);
            if let Err(e) = repo.update_action(&alert_id, &taken, &outcome).await {
                warn!("Failed to update alert action outcome: {}", e);
            }
        });
    }
}

/// Substitute template variables in a string
///
/// Replaces occurrences of `${VAR_NAME}` with the corresponding value
/// from the variables map.
fn substitute_variables(template: &str, variables: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in variables {
        let pattern = format!("${{{}}}", key);
        result = result.replace(&pattern, value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_context_creation() {
        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1");
        assert_eq!(ctx.edge_id, "edge-1");
        assert_eq!(ctx.device_id, "dev-1");
        assert_eq!(ctx.alert_id, "alert-1");
        assert_eq!(ctx.attempt, 1);
        assert!(ctx.variables.is_empty());
    }

    #[test]
    fn test_action_context_with_attempt() {
        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1").with_attempt(3);
        assert_eq!(ctx.attempt, 3);
    }

    #[test]
    fn test_action_context_with_variables() {
        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1")
            .with_variable("DEVICE", "PXI1Slot2")
            .with_variable("TEMP", "95.5");

        assert_eq!(ctx.variables.get("DEVICE").unwrap(), "PXI1Slot2");
        assert_eq!(ctx.variables.get("TEMP").unwrap(), "95.5");
        assert_eq!(ctx.variables.len(), 2);
    }

    #[test]
    fn test_action_context_for_alert_fills_variables() {
        let alert = Alert {
            id: "A1".into(),
            rule_id: "hot".into(),
            edge_id: "e1".into(),
            device_id: "e1:Dev1".into(),
            severity: nimon_core::Severity::Critical,
            status: nimon_core::alert::AlertStatus::Firing,
            title: "t".into(),
            message: "m".into(),
            metric_name: Some("temperature".into()),
            metric_value: Some(91.5),
            threshold: Some(85.0),
            triggered_at: Utc::now(),
            resolved_at: None,
            fired_count: 1,
            notification_sent: false,
        };
        let ctx = ActionContext::for_alert(&alert);
        let s = substitute_variables(
            "${ALERT_ID} ${RULE_ID} ${DEVICE_ID} ${EDGE_ID} ${SEVERITY} ${METRIC} ${VALUE} ${THRESHOLD}",
            &ctx.variables,
        );
        assert_eq!(s, "A1 hot e1:Dev1 e1 critical temperature 91.5 85");
    }

    #[test]
    fn test_substitute_variables() {
        let mut vars = HashMap::new();
        vars.insert("DEVICE".to_string(), "PXI1Slot2".to_string());
        vars.insert("TEMP".to_string(), "95.5".to_string());

        let template = "Device ${DEVICE} has temperature ${TEMP}C";
        let result = substitute_variables(template, &vars);
        assert_eq!(result, "Device PXI1Slot2 has temperature 95.5C");
    }

    #[test]
    fn test_substitute_variables_no_matches() {
        let vars = HashMap::new();
        let template = "No variables here";
        let result = substitute_variables(template, &vars);
        assert_eq!(result, "No variables here");
    }

    #[test]
    fn test_substitute_variables_partial() {
        let mut vars = HashMap::new();
        vars.insert("KNOWN".to_string(), "value".to_string());

        let template = "${KNOWN} and ${UNKNOWN}";
        let result = substitute_variables(template, &vars);
        assert_eq!(result, "value and ${UNKNOWN}");
    }

    #[tokio::test]
    async fn test_disabled_action() {
        let executor = ActionExecutor::new();
        let mut action = Action::script("test-action", "Test", "Test action", "echo hello");
        action.enabled = false;

        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1");
        let result = executor.execute_action(&action, &ctx).await;

        assert_eq!(result.status, ActionStatus::Cancelled);
        assert!(result.error.contains("disabled"));
    }

    fn echo_action(id: &str, text: &str) -> Action {
        #[cfg(target_os = "windows")]
        let script = "cmd";
        #[cfg(not(target_os = "windows"))]
        let script = "echo";
        let mut action = Action::script(id, "Echo Test", "Test echo command", script);
        if let ActionType::Script(ref mut sc) = action.action_type {
            #[cfg(target_os = "windows")]
            {
                sc.args.push("/C".to_string());
                sc.args.push(format!("echo {}", text));
            }
            #[cfg(not(target_os = "windows"))]
            sc.args.push(text.to_string());
        }
        action
    }

    #[tokio::test]
    async fn test_execute_script_success() {
        let executor = ActionExecutor::new();
        let action = echo_action("test-echo", "hello");
        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1");

        let result = executor.execute_action(&action, &ctx).await;
        assert_eq!(result.status, ActionStatus::Succeeded);
        assert!(result.output.contains("hello"));
        assert_eq!(result.attempt, 1);
    }

    #[tokio::test]
    async fn test_script_variables_are_substituted_as_argv() {
        let executor = ActionExecutor::new();
        let action = echo_action("test-vars", "${DEVICE_ID}");
        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1")
            .with_variable("DEVICE_ID", "PXI1Slot3");
        let result = executor.execute_action(&action, &ctx).await;
        assert_eq!(result.status, ActionStatus::Succeeded);
        assert!(result.output.contains("PXI1Slot3"), "{}", result.output);
    }

    #[tokio::test]
    async fn test_script_env_and_working_dir() {
        let executor = ActionExecutor::new();
        let dir = std::env::temp_dir();
        #[cfg(target_os = "windows")]
        let mut action = {
            let mut a = Action::script("test-env", "Env", "env", "cmd");
            if let ActionType::Script(ref mut sc) = a.action_type {
                sc.args = vec!["/C".into(), "echo %NIMON_TEST_VAR% & cd".into()];
            }
            a
        };
        #[cfg(not(target_os = "windows"))]
        let mut action = {
            let mut a = Action::script("test-env", "Env", "env", "sh");
            if let ActionType::Script(ref mut sc) = a.action_type {
                sc.args = vec!["-c".into(), "echo $NIMON_TEST_VAR; pwd".into()];
            }
            a
        };
        if let ActionType::Script(ref mut sc) = action.action_type {
            sc.env
                .insert("NIMON_TEST_VAR".into(), "value-${SEVERITY}".into());
            sc.working_dir = Some(dir.display().to_string());
        }
        let ctx = ActionContext::new("e", "d", "a").with_variable("SEVERITY", "critical");
        let result = executor.execute_action(&action, &ctx).await;
        assert_eq!(result.status, ActionStatus::Succeeded, "{}", result.error);
        assert!(
            result.output.contains("value-critical"),
            "{}",
            result.output
        );
    }

    #[tokio::test]
    async fn test_script_timeout_kills_and_does_not_retry() {
        let executor = ActionExecutor::new();
        #[cfg(target_os = "windows")]
        let mut action = {
            let mut a = Action::script("test-timeout", "Sleep", "sleep", "powershell");
            if let ActionType::Script(ref mut sc) = a.action_type {
                sc.args = vec![
                    "-NoProfile".into(),
                    "-Command".into(),
                    "Start-Sleep -Seconds 30".into(),
                ];
            }
            a
        };
        #[cfg(not(target_os = "windows"))]
        let mut action = {
            let mut a = Action::script("test-timeout", "Sleep", "sleep", "sleep");
            if let ActionType::Script(ref mut sc) = a.action_type {
                sc.args = vec!["30".into()];
            }
            a
        };
        action.timeout_secs = 1;
        action.retry_config.delay_ms = 10;
        let started = Instant::now();
        let result = executor
            .execute_action(&action, &ActionContext::new("e", "d", "a"))
            .await;
        assert_eq!(result.status, ActionStatus::Timeout);
        assert_eq!(result.attempt, 1, "timeouts are not retried");
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[tokio::test]
    async fn test_script_with_detached_child_holding_pipes_completes() {
        // The script exits at once, but a background grandchild inherits
        // stdout/stderr and keeps them open for 10s.
        let executor = ActionExecutor::new();
        #[cfg(target_os = "windows")]
        let mut action = {
            let mut a = Action::script("test-detached", "Detached", "detached", "cmd");
            if let ActionType::Script(ref mut sc) = a.action_type {
                sc.args = vec![
                    "/C".into(),
                    "start /b powershell -NoProfile -Command Start-Sleep -Seconds 10 & echo launched"
                        .into(),
                ];
            }
            a
        };
        #[cfg(not(target_os = "windows"))]
        let mut action = {
            let mut a = Action::script("test-detached", "Detached", "detached", "sh");
            if let ActionType::Script(ref mut sc) = a.action_type {
                sc.args = vec!["-c".into(), "sleep 10 & echo launched".into()];
            }
            a
        };
        action.timeout_secs = 15;
        action.retry_config.max_attempts = 1;
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            executor.execute_action(&action, &ActionContext::new("e", "d", "a")),
        )
        .await
        .expect("action must not hang on pipes held by a detached child");
        assert_eq!(result.status, ActionStatus::Succeeded, "{}", result.error);
        assert!(result.output.contains("launched"), "{}", result.output);
        assert!(started.elapsed() < Duration::from_secs(8));
        assert!(!executor.is_running("test-detached"));
    }

    #[tokio::test]
    async fn test_run_as_is_refused() {
        let executor = ActionExecutor::new();
        let mut action = echo_action("test-run-as", "x");
        action.retry_config.max_attempts = 1;
        if let ActionType::Script(ref mut sc) = action.action_type {
            sc.run_as = Some("root".into());
        }
        let result = executor
            .execute_action(&action, &ActionContext::new("e", "d", "a"))
            .await;
        assert_eq!(result.status, ActionStatus::Failed);
        assert!(result.error.contains("run_as"));
    }

    #[tokio::test]
    async fn test_execute_script_not_found() {
        let executor = ActionExecutor::new();

        let action = Action::script(
            "test-nonexistent",
            "Nonexistent Script",
            "Test nonexistent command",
            "this_command_does_not_exist_anywhere_12345",
        );

        // Disable retries for faster test
        let action_with_no_retry = Action {
            retry_config: super::super::actions::RetryConfig {
                max_attempts: 1,
                ..Default::default()
            },
            ..action
        };

        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1");
        let result = executor.execute_action(&action_with_no_retry, &ctx).await;
        assert_eq!(result.status, ActionStatus::Failed);
    }

    #[tokio::test]
    async fn test_running_action_tracking() {
        let executor = ActionExecutor::new();
        let action = echo_action("test-tracking", "x");

        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1");
        let _ = executor.execute_action(&action, &ctx).await;

        // After execution, status should be Succeeded, not Running
        assert!(!executor.is_running("test-tracking"));
        assert_eq!(
            executor.get_status("test-tracking"),
            Some(ActionStatus::Succeeded)
        );
        executor.cleanup();
        assert_eq!(executor.get_status("test-tracking"), None);
    }

    fn wire_result(action_id: &str) -> nimon_core::actor::messages::ActionResult {
        nimon_core::actor::messages::ActionResult {
            action_id: action_id.to_string(),
            success: true,
            output: None,
            error: None,
            exit_code: Some(0),
            duration_ms: 1,
        }
    }

    fn pending(
        executor: &ActionExecutor,
        req: &str,
        action_id: &str,
        edge_id: &str,
    ) -> oneshot::Receiver<nimon_core::actor::messages::ActionResult> {
        let (tx, rx) = oneshot::channel();
        executor.pending_results.insert(
            req.into(),
            PendingResult {
                tx,
                action_id: action_id.into(),
                edge_id: edge_id.into(),
            },
        );
        rx
    }

    #[test]
    fn test_complete_correlates_by_request_id_then_action_id() {
        let executor = ActionExecutor::new();
        let mut rx = pending(&executor, "REQ1", "act-a", "edge-1");
        // Unknown correlation id and unknown action id: nothing completes
        assert!(!executor.complete("edge-1", "OTHER", wire_result("act-zzz")));
        // Correlation by request id (reply_to)
        assert!(executor.complete("edge-1", "REQ1", wire_result("whatever")));
        assert!(rx.try_recv().is_ok());

        // Fallback: fresh reply msg_id, unique action_id
        let mut rx = pending(&executor, "REQ2", "act-b", "edge-1");
        assert!(executor.complete("edge-1", "FRESH-REPLY-ID", wire_result("act-b")));
        assert!(rx.try_recv().is_ok());
        assert_eq!(executor.pending_count(), 0);
    }

    #[test]
    fn test_complete_rejects_result_from_another_edge() {
        let executor = ActionExecutor::new();
        let mut rx = pending(&executor, "REQ-A", "rule-restart", "edge-a");
        // Edge B knows (or guesses) both the request id and the action id
        assert!(!executor.complete("edge-b", "REQ-A", wire_result("rule-restart")));
        assert!(!executor.complete("edge-b", "FRESH", wire_result("rule-restart")));
        assert!(rx.try_recv().is_err(), "edge A's waiter must stay pending");
        assert_eq!(executor.pending_count(), 1);
        // The edge it was dispatched to completes it
        assert!(executor.complete("edge-a", "REQ-A", wire_result("rule-restart")));
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn test_fail_edge_drops_only_that_edges_pending_results() {
        let executor = ActionExecutor::new();
        let mut rx_a = pending(&executor, "REQ-A", "x", "edge-a");
        let mut rx_b = pending(&executor, "REQ-B", "x", "edge-b");
        assert_eq!(executor.fail_edge("edge-a"), 1);
        assert!(matches!(
            rx_a.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
        assert!(matches!(
            rx_b.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(executor.pending_count(), 1);
    }

    #[actix::test]
    async fn test_edge_action_fails_promptly_when_edge_disconnects() {
        let sessions = Arc::new(SessionStore::new());
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        sessions.add(crate::session::EdgeSession::with_outbound(
            "edge-1".into(),
            "Edge 1".into(),
            None,
            None,
            tx,
        ));
        let executor = ActionExecutor::new().with_sessions(sessions).start();
        let mut action = Action::power_cycle("act-drop", "Power cycle", "test");
        action.timeout_secs = 60;
        let addr = executor.clone();
        let task = tokio::task::spawn_local(async move {
            addr.send(ExecuteAction {
                action,
                context: ActionContext::new("edge-1", "edge-1:daq-1", ""),
            })
            .await
            .unwrap()
        });
        rx.recv().await.expect("dispatched");
        let started = Instant::now();
        assert_eq!(
            executor
                .send(EdgeDisconnected {
                    edge_id: "edge-1".into()
                })
                .await
                .unwrap(),
            1
        );
        let result = task.await.unwrap();
        assert_eq!(result.status, ActionStatus::Failed);
        assert!(result.error.contains("dropped"), "{}", result.error);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[actix::test]
    async fn test_rule_action_is_not_run_concurrently_for_the_same_device() {
        let sessions = Arc::new(SessionStore::new());
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        sessions.add(crate::session::EdgeSession::with_outbound(
            "edge-1".into(),
            "Edge 1".into(),
            None,
            None,
            tx,
        ));
        let executor = ActionExecutor::new().with_sessions(sessions).start();
        let rule_action = |device: &str| {
            let mut action = Action::power_cycle("rule-restart", "Power cycle", "test");
            action.timeout_secs = 30;
            ExecuteRuleAction {
                action,
                on_failure: None,
                context: ActionContext::new("edge-1", device, "alert-1"),
            }
        };
        let addr = executor.clone();
        let first =
            tokio::task::spawn_local(async move { addr.send(rule_action("edge-1:d1")).await });
        let request = rx.recv().await.expect("first run dispatched");

        // Re-fire while the first run is in flight: skipped, nothing sent
        let second = executor.send(rule_action("edge-1:d1")).await.unwrap();
        assert_eq!(second.status, ActionStatus::Cancelled);
        assert!(second.error.contains("still in progress"));
        assert!(rx.try_recv().is_err(), "no second dispatch");

        // Another device is independent
        let addr = executor.clone();
        let other =
            tokio::task::spawn_local(async move { addr.send(rule_action("edge-1:d2")).await });
        let other_request = rx.recv().await.expect("other device dispatched");

        for req in [request, other_request] {
            executor
                .send(CompleteAction {
                    edge_id: "edge-1".into(),
                    msg_id: req.msg_id,
                    result: wire_result("rule-restart"),
                })
                .await
                .unwrap();
        }
        assert_eq!(
            first.await.unwrap().unwrap().status,
            ActionStatus::Succeeded
        );
        assert_eq!(
            other.await.unwrap().unwrap().status,
            ActionStatus::Succeeded
        );

        // Finished: the next re-fire runs again
        let addr = executor.clone();
        let third =
            tokio::task::spawn_local(async move { addr.send(rule_action("edge-1:d1")).await });
        let req = rx.recv().await.expect("dispatched again after completion");
        executor
            .send(CompleteAction {
                edge_id: "edge-1".into(),
                msg_id: req.msg_id,
                result: wire_result("rule-restart"),
            })
            .await
            .unwrap();
        assert_eq!(
            third.await.unwrap().unwrap().status,
            ActionStatus::Succeeded
        );
    }

    #[test]
    fn test_executor_default() {
        let executor = ActionExecutor::default();
        assert!(executor.running_actions.is_empty());
    }

    #[test]
    fn test_executor_clone() {
        let executor = ActionExecutor::new();
        let cloned = executor.clone();
        assert_eq!(executor.running_actions.len(), cloned.running_actions.len());
    }

    #[test]
    fn test_action_context_clone() {
        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1")
            .with_attempt(2)
            .with_variable("KEY", "value");
        let cloned = ctx.clone();
        assert_eq!(cloned.edge_id, ctx.edge_id);
        assert_eq!(cloned.attempt, ctx.attempt);
        assert_eq!(cloned.variables.len(), ctx.variables.len());
    }
}
