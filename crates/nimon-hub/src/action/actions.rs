//! Self-healing action definitions
//!
//! Defines the types and structures for remediation actions
//! that can be executed when alerts fire.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for retry behavior on failed actions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_attempts: u32,
    /// Delay between retries in milliseconds
    pub delay_ms: u64,
    /// Multiplier for exponential backoff
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            delay_ms: 1000,
            backoff_multiplier: 2.0,
        }
    }
}

/// The type of remediation action to execute
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ActionType {
    /// Run a custom script or command
    Script,
    /// Restart a system service
    RestartService,
    /// Send a command to an edge node
    EdgeCommand,
    /// Power cycle a device
    PowerCycle,
}

/// Configuration for a script-type action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptAction {
    /// The script or command to execute
    pub script: String,
    /// Arguments to pass to the script
    pub args: Vec<String>,
    /// Environment variables to set
    pub env: HashMap<String, String>,
    /// Working directory for the script
    pub working_dir: Option<String>,
    /// User to run the script as (e.g., "root")
    pub run_as: Option<String>,
}

/// Status of an action execution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ActionStatus {
    /// Action is waiting to be executed
    Pending,
    /// Action is currently running
    Running,
    /// Action completed successfully
    Succeeded,
    /// Action failed
    Failed,
    /// Action timed out
    Timeout,
    /// Action was cancelled
    Cancelled,
}

/// A self-healing action definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    /// Unique identifier for this action
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Description of what this action does
    pub description: String,
    /// The type of action
    pub action_type: ActionType,
    /// Whether this action is enabled
    pub enabled: bool,
    /// Timeout in seconds
    pub timeout_secs: u64,
    /// Retry configuration
    pub retry_config: RetryConfig,
    /// Script-specific configuration (only for Script type)
    pub script_config: Option<ScriptAction>,
    /// Service name (only for RestartService type)
    pub service_name: Option<String>,
}

impl Action {
    /// Create a new script action
    pub fn script(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        script: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            action_type: ActionType::Script,
            enabled: true,
            timeout_secs: 30,
            retry_config: RetryConfig::default(),
            script_config: Some(ScriptAction {
                script: script.into(),
                args: Vec::new(),
                env: HashMap::new(),
                working_dir: None,
                run_as: None,
            }),
            service_name: None,
        }
    }

    /// Create a new restart service action
    pub fn restart_service(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        service_name: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            action_type: ActionType::RestartService,
            enabled: true,
            timeout_secs: 60,
            retry_config: RetryConfig::default(),
            script_config: None,
            service_name: Some(service_name.into()),
        }
    }

    /// Create a new power cycle action
    pub fn power_cycle(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            action_type: ActionType::PowerCycle,
            enabled: true,
            timeout_secs: 120,
            retry_config: RetryConfig {
                max_attempts: 1,
                ..Default::default()
            },
            script_config: None,
            service_name: None,
        }
    }
}

/// Result of an action execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    /// The action that was executed
    pub action_id: String,
    /// Final status of the action
    pub status: ActionStatus,
    /// Exit code from the process (if applicable)
    pub exit_code: Option<i32>,
    /// Standard output from the process
    pub output: String,
    /// Error output or message
    pub error: String,
    /// Duration of execution in milliseconds
    pub duration_ms: u64,
    /// Which attempt this result represents (1-indexed)
    pub attempt: u32,
    /// Timestamp when the action was executed
    pub executed_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_creation() {
        // Test script action
        let script_action = Action::script(
            "action-restart-app",
            "Restart Application",
            "Restarts the application service",
            "/usr/local/bin/restart-app.sh",
        );
        assert_eq!(script_action.id, "action-restart-app");
        assert_eq!(script_action.name, "Restart Application");
        assert_eq!(script_action.action_type, ActionType::Script);
        assert!(script_action.enabled);
        assert_eq!(script_action.timeout_secs, 30);
        assert!(script_action.script_config.is_some());
        let script_cfg = script_action.script_config.unwrap();
        assert_eq!(script_cfg.script, "/usr/local/bin/restart-app.sh");
        assert!(script_cfg.args.is_empty());
        assert!(script_cfg.env.is_empty());
        assert!(script_cfg.working_dir.is_none());
        assert!(script_cfg.run_as.is_none());

        // Test restart service action
        let restart_action = Action::restart_service(
            "action-restart-svc",
            "Restart Service",
            "Restarts a system service",
            "nimon-agent",
        );
        assert_eq!(restart_action.action_type, ActionType::RestartService);
        assert_eq!(restart_action.service_name.as_deref(), Some("nimon-agent"));
        assert_eq!(restart_action.timeout_secs, 60);

        // Test power cycle action
        let power_action = Action::power_cycle(
            "action-power-cycle",
            "Power Cycle Device",
            "Power cycles the device hardware",
        );
        assert_eq!(power_action.action_type, ActionType::PowerCycle);
        assert_eq!(power_action.timeout_secs, 120);
        assert_eq!(power_action.retry_config.max_attempts, 1);
        assert!(power_action.script_config.is_none());
        assert!(power_action.service_name.is_none());
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.delay_ms, 1000);
        assert!((config.backoff_multiplier - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_action_status_equality() {
        assert_eq!(ActionStatus::Pending, ActionStatus::Pending);
        assert_ne!(ActionStatus::Pending, ActionStatus::Running);
        assert_ne!(ActionStatus::Failed, ActionStatus::Timeout);
        assert_eq!(ActionStatus::Cancelled, ActionStatus::Cancelled);
    }

    #[test]
    fn test_action_type_equality() {
        assert_eq!(ActionType::Script, ActionType::Script);
        assert_ne!(ActionType::Script, ActionType::RestartService);
        assert_ne!(ActionType::EdgeCommand, ActionType::PowerCycle);
    }

    #[test]
    fn test_script_action_with_args_and_env() {
        let mut action = Action::script(
            "action-with-args",
            "Script With Args",
            "A script with arguments and environment",
            "/usr/bin/python3",
        );
        if let Some(ref mut cfg) = action.script_config {
            cfg.args.push("-c".to_string());
            cfg.args.push("print('hello')".to_string());
            cfg.env.insert("PYTHONPATH".to_string(), "/opt/lib".to_string());
            cfg.working_dir = Some("/tmp".to_string());
            cfg.run_as = Some("nobody".to_string());
        }
        let cfg = action.script_config.as_ref().unwrap();
        assert_eq!(cfg.args.len(), 2);
        assert_eq!(cfg.args[0], "-c");
        assert_eq!(cfg.env.get("PYTHONPATH").unwrap(), "/opt/lib");
        assert_eq!(cfg.working_dir.as_deref(), Some("/tmp"));
        assert_eq!(cfg.run_as.as_deref(), Some("nobody"));
    }
}
