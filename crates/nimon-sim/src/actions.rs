//! Pure protocol decisions: how to answer `execute_action`, which hub
//! error codes are fatal, and reconnect backoff.

use nimon_core::actor::messages::{ActionResult, ExecuteAction};
use nimon_core::protocol::WsMessage;
use rand::Rng;
use std::time::Duration;

use crate::cli::ActionMode;

/// Simulated execution time for immediate success/fail replies.
pub const SIMULATED_ACTION_MS: u64 = 50;

/// What to do with an incoming `execute_action`.
#[derive(Debug)]
pub enum ActionPlan {
    /// Send `reply` after waiting `after`
    Reply { after: Duration, reply: WsMessage },
    /// Never answer (timeout mode)
    Withhold,
}

/// Decide the reply for `action` (request envelope id `request_msg_id`).
/// The reply always carries `reply_to = request_msg_id` so the hub can
/// correlate it.
pub fn plan_action_reply(
    mode: ActionMode,
    request_msg_id: &str,
    action: &ExecuteAction,
    known_device: bool,
) -> ActionPlan {
    let note = if known_device {
        String::new()
    } else {
        " (device not managed by this simulated edge)".to_string()
    };
    let (success, ms) = match mode {
        ActionMode::Timeout => return ActionPlan::Withhold,
        ActionMode::Success => (true, SIMULATED_ACTION_MS),
        ActionMode::Fail => (false, SIMULATED_ACTION_MS),
        ActionMode::Delay(ms) => (true, ms),
    };
    let result = ActionResult {
        action_id: action.action_id.clone(),
        success,
        output: success.then(|| {
            format!(
                "simulated {} on {} completed{note}",
                action.action_type, action.device_id
            )
        }),
        error: (!success).then(|| {
            format!(
                "simulated {} on {} failed (--action-mode fail){note}",
                action.action_type, action.device_id
            )
        }),
        exit_code: Some(if success { 0 } else { 1 }),
        duration_ms: ms,
    };
    ActionPlan::Reply {
        after: Duration::from_millis(ms),
        reply: WsMessage::action_result(result).with_reply_to(request_msg_id),
    }
}

/// Hub error codes after which retrying is pointless (wrong protocol
/// version or rejected credentials): the simulator exits non-zero.
pub fn is_fatal_error_code(code: &str) -> bool {
    let c = code.to_ascii_uppercase();
    ["VERSION", "AUTH", "UNAUTHORIZED", "FORBIDDEN", "TOKEN"]
        .iter()
        .any(|needle| c.contains(needle))
}

/// Exponential backoff with +/-25% jitter, capped at `max`.
pub fn backoff_delay<R: Rng>(attempt: u32, base: Duration, max: Duration, rng: &mut R) -> Duration {
    let exp = base.as_secs_f64() * 2f64.powi(attempt.min(30) as i32);
    let capped = exp.min(max.as_secs_f64());
    let jittered = capped * rng.gen_range(0.75..=1.25);
    Duration::from_secs_f64(jittered).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_core::actor::messages::ActionType;
    use nimon_core::protocol::WsMessageType;
    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use std::collections::HashMap;

    fn request() -> (WsMessage, ExecuteAction) {
        let action = ExecuteAction {
            action_id: "act-9".to_string(),
            device_id: "sim-edge-01:PXI1Slot2".to_string(),
            action_type: ActionType::ResetDriver,
            parameters: HashMap::new(),
        };
        (WsMessage::execute_action(action.clone()), action)
    }

    fn reply_of(plan: ActionPlan) -> (Duration, WsMessage, ActionResult) {
        match plan {
            ActionPlan::Reply { after, reply } => {
                let r = reply.payload::<ActionResult>().unwrap();
                (after, reply, r)
            }
            ActionPlan::Withhold => panic!("expected a reply"),
        }
    }

    #[test]
    fn success_reply_sets_reply_to() {
        let (req, action) = request();
        let (after, msg, result) = reply_of(plan_action_reply(
            ActionMode::Success,
            &req.msg_id,
            &action,
            true,
        ));
        assert_eq!(msg.msg_type, WsMessageType::ActionResult);
        assert_eq!(msg.reply_to.as_deref(), Some(req.msg_id.as_str()));
        assert_eq!(msg.correlation_id(), req.msg_id);
        assert_ne!(msg.msg_id, req.msg_id);
        // survives the wire
        let parsed = WsMessage::from_json(&msg.to_json().unwrap()).unwrap();
        assert_eq!(parsed.reply_to.as_deref(), Some(req.msg_id.as_str()));
        assert!(result.success);
        assert_eq!(result.action_id, "act-9");
        assert_eq!(result.exit_code, Some(0));
        assert!(result.error.is_none());
        assert_eq!(after, Duration::from_millis(SIMULATED_ACTION_MS));
    }

    #[test]
    fn fail_and_delay_modes() {
        let (req, action) = request();
        let (_, msg, result) = reply_of(plan_action_reply(
            ActionMode::Fail,
            &req.msg_id,
            &action,
            true,
        ));
        assert_eq!(msg.reply_to.as_deref(), Some(req.msg_id.as_str()));
        assert!(!result.success);
        assert!(result.error.is_some());
        assert_eq!(result.exit_code, Some(1));

        let (after, msg, result) = reply_of(plan_action_reply(
            ActionMode::Delay(1234),
            &req.msg_id,
            &action,
            false,
        ));
        assert_eq!(after, Duration::from_millis(1234));
        assert_eq!(result.duration_ms, 1234);
        assert!(result.success);
        assert!(result.output.unwrap().contains("not managed"));
        assert_eq!(msg.reply_to.as_deref(), Some(req.msg_id.as_str()));
    }

    #[test]
    fn timeout_mode_withholds() {
        let (req, action) = request();
        assert!(matches!(
            plan_action_reply(ActionMode::Timeout, &req.msg_id, &action, true),
            ActionPlan::Withhold
        ));
    }

    #[test]
    fn fatal_codes() {
        assert!(is_fatal_error_code("VERSION_MISMATCH"));
        assert!(is_fatal_error_code("AUTH_FAILED"));
        assert!(is_fatal_error_code("unauthorized"));
        assert!(is_fatal_error_code("INVALID_TOKEN"));
        assert!(!is_fatal_error_code("PARSE_ERROR"));
        assert!(!is_fatal_error_code("RATE_LIMITED"));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let mut rng = StdRng::seed_from_u64(1);
        let base = Duration::from_millis(500);
        let max = Duration::from_secs(30);
        for _ in 0..200 {
            let d0 = backoff_delay(0, base, max, &mut rng);
            assert!(d0 >= Duration::from_millis(375) && d0 <= Duration::from_millis(625));
            let d3 = backoff_delay(3, base, max, &mut rng);
            assert!(d3 >= Duration::from_millis(3000) && d3 <= Duration::from_millis(5000));
            let big = backoff_delay(50, base, max, &mut rng);
            assert!(big >= Duration::from_millis(22_500) && big <= max);
        }
    }
}
