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
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionType {
    /// Run a custom script or command
    Script(ScriptAction),
    /// Restart a system service
    RestartService { service_name: String },
    /// Send a command to an edge node
    EdgeCommand {
        command: String,
        parameters: HashMap<String, String>,
    },
    /// Power cycle a device
    PowerCycle { delay_secs: u64 },
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionType::Script(_) => write!(f, "script"),
            ActionType::RestartService { .. } => write!(f, "restart_service"),
            ActionType::EdgeCommand { .. } => write!(f, "edge_command"),
            ActionType::PowerCycle { .. } => write!(f, "power_cycle"),
        }
    }
}

/// Configuration for a script-type action
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
            action_type: ActionType::Script(ScriptAction {
                script: script.into(),
                args: Vec::new(),
                env: HashMap::new(),
                working_dir: None,
                run_as: None,
            }),
            enabled: true,
            timeout_secs: 30,
            retry_config: RetryConfig::default(),
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
            action_type: ActionType::RestartService {
                service_name: service_name.into(),
            },
            enabled: true,
            timeout_secs: 60,
            retry_config: RetryConfig::default(),
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
            action_type: ActionType::PowerCycle { delay_secs: 0 },
            enabled: true,
            timeout_secs: 120,
            retry_config: RetryConfig {
                max_attempts: 1,
                ..Default::default()
            },
        }
    }

    /// Create a new edge command action
    pub fn edge_command(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            action_type: ActionType::EdgeCommand {
                command: String::new(),
                parameters: HashMap::new(),
            },
            enabled: true,
            timeout_secs: 120,
            retry_config: RetryConfig::default(),
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

/// Executable actions attached to a configured rule.
#[derive(Debug, Clone)]
pub struct RuleActionSpec {
    /// Primary action, dispatched when the rule fires
    pub action: Action,
    /// Fallback executed when the primary action still fails after retries
    pub on_failure: Option<Action>,
}

/// Map a hub `EdgeCommand` onto the wire action family the edge
/// understands:
/// - `power_cycle` / `reset_driver` map to the matching edge action
/// - `restart_services` maps to `RestartServices`; the edge reads the
///   comma-separated service list from parameter `services` (also filled
///   from `service_name`/`service` when only those are given)
/// - `custom_script` runs the allowlisted script named in parameter `script`
/// - anything else runs as an allowlisted edge script named `command`
pub fn map_edge_command(
    command: &str,
    parameters: &HashMap<String, String>,
) -> (
    nimon_core::actor::messages::ActionType,
    HashMap<String, String>,
) {
    use nimon_core::actor::messages::ActionType as Wire;
    let mut params = parameters.clone();
    let wire = match command {
        "power_cycle" => Wire::PowerCycle,
        "reset_driver" => Wire::ResetDriver,
        "restart_services" => {
            if !params.contains_key("services") {
                if let Some(name) = params
                    .get("service_name")
                    .or_else(|| params.get("service"))
                    .cloned()
                {
                    params.insert("services".to_string(), name);
                }
            }
            Wire::RestartServices
        }
        "custom_script" => Wire::CustomScript,
        other => {
            params.insert("script".to_string(), other.to_string());
            Wire::CustomScript
        }
    };
    (wire, params)
}

/// Convert a rule's action reference (core type) into an executable Action.
pub fn action_from_ref(action_ref: &nimon_core::alert::rules::ActionRef) -> Action {
    use nimon_core::alert::rules::ActionRef;
    match action_ref {
        ActionRef::Script {
            script,
            args,
            timeout_secs,
        } => {
            let mut action = Action::script(
                format!("rule-script-{}", script),
                "Rule script",
                "Script action attached to an alert rule",
                script.clone(),
            );
            if let ActionType::Script(ref mut cfg) = action.action_type {
                cfg.args = args.clone();
            }
            if let Some(timeout) = timeout_secs {
                action.timeout_secs = *timeout;
            }
            action
        }
        ActionRef::RestartService { service_name } => Action::restart_service(
            format!("rule-restart-{}", service_name),
            "Rule service restart",
            "Service restart action attached to an alert rule",
            service_name.clone(),
        ),
        ActionRef::PowerCycle { delay_secs } => {
            let mut action = Action::power_cycle(
                "rule-power-cycle",
                "Rule power cycle",
                "Power cycle action attached to an alert rule",
            );
            if let ActionType::PowerCycle { delay_secs: d } = &mut action.action_type {
                *d = delay_secs.unwrap_or(0);
            }
            action
        }
        ActionRef::EdgeCommand {
            command,
            parameters,
        } => {
            let mut action = Action::edge_command(
                format!("rule-edge-{}", command),
                "Rule edge command",
                "Edge command action attached to an alert rule",
            );
            if let ActionType::EdgeCommand {
                command: c,
                parameters: p,
            } = &mut action.action_type
            {
                *c = command.clone();
                *p = parameters.clone();
            }
            action
        }
        ActionRef::CustomScript { script } => {
            let mut action = Action::edge_command(
                format!("rule-custom-script-{}", script),
                "Rule custom script",
                "Allowlisted edge script attached to an alert rule",
            );
            if let ActionType::EdgeCommand {
                command,
                parameters,
            } = &mut action.action_type
            {
                *command = "custom_script".to_string();
                parameters.insert("script".to_string(), script.clone());
            }
            action
        }
    }
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
        assert!(matches!(script_action.action_type, ActionType::Script(_)));
        assert!(script_action.enabled);
        assert_eq!(script_action.timeout_secs, 30);
        if let ActionType::Script(ref script_cfg) = script_action.action_type {
            assert_eq!(script_cfg.script, "/usr/local/bin/restart-app.sh");
            assert!(script_cfg.args.is_empty());
            assert!(script_cfg.env.is_empty());
            assert!(script_cfg.working_dir.is_none());
            assert!(script_cfg.run_as.is_none());
        }

        // Test restart service action
        let restart_action = Action::restart_service(
            "action-restart-svc",
            "Restart Service",
            "Restarts a system service",
            "nimon-agent",
        );
        assert!(matches!(
            restart_action.action_type,
            ActionType::RestartService { .. }
        ));
        if let ActionType::RestartService { ref service_name } = restart_action.action_type {
            assert_eq!(service_name, "nimon-agent");
        }
        assert_eq!(restart_action.timeout_secs, 60);

        // Test power cycle action
        let power_action = Action::power_cycle(
            "action-power-cycle",
            "Power Cycle Device",
            "Power cycles the device hardware",
        );
        assert!(matches!(
            power_action.action_type,
            ActionType::PowerCycle { .. }
        ));
        assert_eq!(power_action.timeout_secs, 120);
        assert_eq!(power_action.retry_config.max_attempts, 1);
    }

    #[test]
    fn test_edge_command_mapping() {
        use nimon_core::actor::messages::ActionType as Wire;
        let empty = HashMap::new();
        assert_eq!(map_edge_command("power_cycle", &empty).0, Wire::PowerCycle);
        assert_eq!(
            map_edge_command("reset_driver", &empty).0,
            Wire::ResetDriver
        );

        let mut params = HashMap::new();
        params.insert("service_name".to_string(), "NiSvc".to_string());
        let (wire, p) = map_edge_command("restart_services", &params);
        assert_eq!(wire, Wire::RestartServices);
        assert_eq!(p.get("services").unwrap(), "NiSvc");

        let mut params = HashMap::new();
        params.insert("script".to_string(), "fix.ps1".to_string());
        let (wire, p) = map_edge_command("custom_script", &params);
        assert_eq!(wire, Wire::CustomScript);
        assert_eq!(p.get("script").unwrap(), "fix.ps1");

        let (wire, p) = map_edge_command("resend_state.ps1", &empty);
        assert_eq!(wire, Wire::CustomScript);
        assert_eq!(p.get("script").unwrap(), "resend_state.ps1");
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
        let script_a = ActionType::Script(ScriptAction {
            script: "a".into(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            run_as: None,
        });
        let script_b = ActionType::Script(ScriptAction {
            script: "a".into(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            run_as: None,
        });
        let restart = ActionType::RestartService {
            service_name: "svc".into(),
        };
        assert_eq!(script_a, script_b);
        assert_ne!(script_a, restart);
    }

    #[test]
    fn test_script_action_with_args_and_env() {
        let mut action = Action::script(
            "action-with-args",
            "Script With Args",
            "A script with arguments and environment",
            "/usr/bin/python3",
        );
        if let ActionType::Script(ref mut cfg) = action.action_type {
            cfg.args.push("-c".to_string());
            cfg.args.push("print('hello')".to_string());
            cfg.env
                .insert("PYTHONPATH".to_string(), "/opt/lib".to_string());
            cfg.working_dir = Some("/tmp".to_string());
            cfg.run_as = Some("nobody".to_string());
        }
        if let ActionType::Script(ref cfg) = action.action_type {
            assert_eq!(cfg.args.len(), 2);
            assert_eq!(cfg.args[0], "-c");
            assert_eq!(cfg.env.get("PYTHONPATH").unwrap(), "/opt/lib");
            assert_eq!(cfg.working_dir.as_deref(), Some("/tmp"));
            assert_eq!(cfg.run_as.as_deref(), Some("nobody"));
        }
    }
}
