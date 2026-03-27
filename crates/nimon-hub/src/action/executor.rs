//! Action executor for self-healing remediation
//!
//! Executes remediation actions when alerts fire, with support for
//! scripts, service restarts, edge commands, and power cycling.

use actix::prelude::*;
use chrono::Utc;
use dashmap::DashMap;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::process::Command as TokioCommand;
use tracing::{debug, error, info, warn};

use super::actions::{
    Action, ActionResult, ActionStatus, ActionType,
};

/// Context in which an action is being executed
#[derive(Debug, Clone)]
pub struct ActionContext {
    /// The edge node ID where the alert originated
    pub edge_id: String,
    /// The device ID that triggered the alert
    pub device_id: String,
    /// The alert that triggered this action
    pub alert_id: String,
    /// Current attempt number (1-indexed)
    pub attempt: u32,
    /// Template variables for substitution
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

/// Action executor actor
///
/// Manages the execution of self-healing actions with retry logic,
/// timeout handling, and tracking of running actions.
pub struct ActionExecutor {
    /// Currently running actions tracked by action ID
    running_actions: DashMap<String, ActionStatus>,
}

impl ActionExecutor {
    /// Create a new action executor
    pub fn new() -> Self {
        Self {
            running_actions: DashMap::new(),
        }
    }

    /// Execute an action with full retry logic
    pub async fn execute_action(
        &self,
        action: &Action,
        ctx: &ActionContext,
    ) -> ActionResult {
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

        // Mark as running
        self.running_actions
            .insert(action.id.clone(), ActionStatus::Running);

        let mut last_result: Option<ActionResult> = None;

        for attempt in 1..=action.retry_config.max_attempts {
            let attempt_ctx = ActionContext {
                attempt,
                ..ctx.clone()
            };

            info!(
                "Executing action {} (attempt {}/{})",
                action.id, attempt, action.retry_config.max_attempts
            );

            let result = self.execute_action_internal(action, &attempt_ctx).await;

            match result.status {
                ActionStatus::Succeeded => {
                    self.running_actions
                        .insert(action.id.clone(), ActionStatus::Succeeded);
                    return result;
                }
                ActionStatus::Cancelled => {
                    self.running_actions
                        .insert(action.id.clone(), ActionStatus::Cancelled);
                    return result;
                }
                ActionStatus::Failed | ActionStatus::Timeout => {
                    warn!(
                        "Action {} attempt {}/{} failed: {}",
                        action.id,
                        attempt,
                        action.retry_config.max_attempts,
                        result.error
                    );
                    last_result = Some(result);

                    if attempt < action.retry_config.max_attempts {
                        let delay_ms = (action.retry_config.delay_ms as f64
                            * action.retry_config.backoff_multiplier.powi(attempt as i32 - 1))
                            as u64;
                        debug!(
                            "Retrying action {} in {}ms",
                            action.id, delay_ms
                        );
                        tokio::time::sleep(Duration::from_millis(delay_ms as u64)).await;
                    }
                }
                _ => {
                    last_result = Some(result);
                }
            }
        }

        // All retries exhausted
        if let Some(mut final_result) = last_result {
            final_result.status = ActionStatus::Failed;
            self.running_actions
                .insert(action.id.clone(), ActionStatus::Failed);
            final_result
        } else {
            // Should not reach here, but provide a fallback
            self.running_actions
                .insert(action.id.clone(), ActionStatus::Failed);
            ActionResult {
                action_id: action.id.clone(),
                status: ActionStatus::Failed,
                exit_code: None,
                output: String::new(),
                error: "No execution result".to_string(),
                duration_ms: 0,
                attempt: action.retry_config.max_attempts,
                executed_at: Utc::now(),
            }
        }
    }

    /// Execute a single attempt of an action
    async fn execute_action_internal(
        &self,
        action: &Action,
        ctx: &ActionContext,
    ) -> ActionResult {
        let start = Instant::now();

        match action.action_type {
            ActionType::Script => self.execute_script(action, ctx).await,
            ActionType::RestartService => self.execute_restart(action, ctx).await,
            ActionType::EdgeCommand => self.execute_edge_command(action, ctx).await,
            ActionType::PowerCycle => self.execute_power_cycle(action, ctx).await,
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

    /// Execute a script action
    async fn execute_script(
        &self,
        action: &Action,
        ctx: &ActionContext,
    ) -> std::result::Result<ActionResult, anyhow::Error> {
        let script_config = action
            .script_config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Script action has no script configuration"))?;

        // Perform variable substitution on script and args
        let script = substitute_variables(&script_config.script, &ctx.variables);
        let args: Vec<String> = script_config
            .args
            .iter()
            .map(|a| substitute_variables(a, &ctx.variables))
            .collect();

        debug!(
            "Running script: {} with args {:?}",
            script, args
        );

        let mut cmd = TokioCommand::new(&script);
        cmd.args(&args);

        // Set environment variables
        for (key, value) in &script_config.env {
            let substituted = substitute_variables(value, &ctx.variables);
            cmd.env(key, substituted);
        }

        // Set working directory
        if let Some(ref dir) = script_config.working_dir {
            cmd.current_dir(dir);
        }

        // Execute with timeout
        let output = tokio::time::timeout(
            Duration::from_secs(action.timeout_secs),
            cmd.output(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Action timed out after {}s", action.timeout_secs))?
        .map_err(|e| anyhow::anyhow!("Failed to execute script: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let succeeded = output.status.success();

        Ok(ActionResult {
            action_id: action.id.clone(),
            status: if succeeded {
                ActionStatus::Succeeded
            } else {
                ActionStatus::Failed
            },
            exit_code: output.status.code(),
            output: stdout,
            error: if succeeded {
                String::new()
            } else {
                stderr
            },
            duration_ms: 0, // Set by caller
            attempt: ctx.attempt,
            executed_at: Utc::now(),
        })
    }

    /// Execute a service restart action
    async fn execute_restart(
        &self,
        action: &Action,
        _ctx: &ActionContext,
    ) -> std::result::Result<ActionResult, anyhow::Error> {
        let service_name = action
            .service_name
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Restart action has no service name"))?;

        #[cfg(target_os = "windows")]
        let result = {
            let stop_output = TokioCommand::new("sc")
                .args(&["stop", service_name])
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to stop service: {}", e))?;

            let start_output = TokioCommand::new("sc")
                .args(&["start", service_name])
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to start service: {}", e))?;

            let stdout = String::from_utf8_lossy(&stop_output.stdout).to_string()
                + &String::from_utf8_lossy(&start_output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&stop_output.stderr).to_string()
                + &String::from_utf8_lossy(&start_output.stderr).to_string();

            let succeeded = start_output.status.success();
            (stdout, stderr, succeeded, start_output.status.code())
        };

        #[cfg(not(target_os = "windows"))]
        let result = {
            let output = TokioCommand::new("systemctl")
                .args(&["restart", service_name])
                .output()
                .await
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
            attempt: _ctx.attempt,
            executed_at: Utc::now(),
        })
    }

    /// Execute an edge command action
    ///
    /// TODO: Implement edge node command dispatch via WebSocket
    async fn execute_edge_command(
        &self,
        action: &Action,
        _ctx: &ActionContext,
    ) -> std::result::Result<ActionResult, anyhow::Error> {
        // TODO: Send command to edge node via WebSocket and wait for response
        warn!(
            "Edge command execution not yet implemented for action {}",
            action.id
        );

        Ok(ActionResult {
            action_id: action.id.clone(),
            status: ActionStatus::Failed,
            exit_code: None,
            output: String::new(),
            error: "Edge command execution not yet implemented".to_string(),
            duration_ms: 0,
            attempt: _ctx.attempt,
            executed_at: Utc::now(),
        })
    }

    /// Execute a power cycle action
    ///
    /// TODO: Implement power cycling via NI-DCPower or relay control
    async fn execute_power_cycle(
        &self,
        action: &Action,
        _ctx: &ActionContext,
    ) -> std::result::Result<ActionResult, anyhow::Error> {
        // TODO: Implement power cycling through hardware interface
        warn!(
            "Power cycle execution not yet implemented for action {}",
            action.id
        );

        Ok(ActionResult {
            action_id: action.id.clone(),
            status: ActionStatus::Failed,
            exit_code: None,
            output: String::new(),
            error: "Power cycle execution not yet implemented".to_string(),
            duration_ms: 0,
            attempt: _ctx.attempt,
            executed_at: Utc::now(),
        })
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
        self.running_actions.retain(|_, status| {
            *status == ActionStatus::Running
        });
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
            running_actions: self.running_actions.clone(),
        }
    }
}

impl Actor for ActionExecutor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
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
        Box::pin(async move {
            executor.execute_action(&msg.action, &msg.context).await
        })
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
        let mut action = Action::script(
            "test-action",
            "Test",
            "Test action",
            "echo hello",
        );
        action.enabled = false;

        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1");
        let result = executor.execute_action(&action, &ctx).await;

        assert_eq!(result.status, ActionStatus::Cancelled);
        assert!(result.error.contains("disabled"));
    }

    #[tokio::test]
    async fn test_execute_script_success() {
        let executor = ActionExecutor::new();

        // Use a command that exists on all platforms
        #[cfg(target_os = "windows")]
        let script = "cmd";
        #[cfg(not(target_os = "windows"))]
        let script = "echo";

        let mut action = Action::script(
            "test-echo",
            "Echo Test",
            "Test echo command",
            script,
        );

        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1");

        // Set up args based on platform
        #[cfg(target_os = "windows")]
        {
            if let Some(ref mut sc) = action.script_config {
                sc.args.push("/C".to_string());
                sc.args.push("echo hello".to_string());
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            if let Some(ref mut sc) = action.script_config {
                sc.args.push("hello".to_string());
            }
        }

        let result = executor.execute_action(&action, &ctx).await;
        assert_eq!(result.status, ActionStatus::Succeeded);
        assert!(result.duration_ms > 0);
        assert_eq!(result.attempt, 1);
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

        #[cfg(target_os = "windows")]
        let script = "cmd";
        #[cfg(not(target_os = "windows"))]
        let script = "echo";

        let action = Action::script(
            "test-tracking",
            "Tracking Test",
            "Test action tracking",
            script,
        );

        let ctx = ActionContext::new("edge-1", "dev-1", "alert-1");
        let _ = executor.execute_action(&action, &ctx).await;

        // After execution, status should be Succeeded, not Running
        assert!(!executor.is_running("test-tracking"));
        assert_eq!(
            executor.get_status("test-tracking"),
            Some(ActionStatus::Succeeded)
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
