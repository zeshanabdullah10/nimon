//! Alert rule evaluation logic
//!
//! Every condition has two predicates:
//! - [`RuleCondition::evaluate`]: should the alert fire (or stay firing)?
//! - [`RuleCondition::is_cleared`]: has the underlying problem gone away,
//!   so an active alert may be auto-resolved?
//!
//! The two are deliberately not complements: hysteresis leaves a band
//! where neither is true, transition rules clear on the resulting state
//! rather than on the transition, and unknown data (missing or NaN
//! metric) never counts as cleared.

use crate::{HealthStatus, MetricValue, Severity};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A self-healing action attached to a rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionRef {
    /// Run a script on the hub host
    Script {
        script: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        timeout_secs: Option<u64>,
    },
    /// Restart an OS service on the hub host
    RestartService { service_name: String },
    /// Power-cycle the device (executed on the edge)
    PowerCycle {
        #[serde(default)]
        delay_secs: Option<u64>,
    },
    /// Send a command to the edge node attached to the device
    EdgeCommand {
        command: String,
        #[serde(default)]
        parameters: HashMap<String, String>,
    },
    /// Run an allowlisted script on the edge node attached to the device
    CustomScript { script: String },
}

/// An alert rule configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub severity: Severity,
    pub condition: RuleCondition,
    pub cooldown_minutes: i32,
    pub notification_channels: Vec<String>,
    pub suppress_repeat: bool,
    /// Cap on simultaneously active alerts for this rule (suppression beyond)
    #[serde(default)]
    pub max_firing_count: Option<i32>,
    /// Self-healing action dispatched when the rule fires
    #[serde(default)]
    pub action: Option<ActionRef>,
}

/// Rule conditions for triggering alerts
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleCondition {
    /// Fires while the metric compares true against `threshold`.
    /// Cleared once the value is back past the threshold by `hysteresis`.
    MetricThreshold {
        metric_name: String,
        operator: ComparisonOp,
        threshold: f64,
        duration_minutes: Option<i32>,
        /// Clear margin (>= 0) in metric units; `None` = 0 (clear as soon
        /// as the comparison is false).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hysteresis: Option<f64>,
    },
    /// Fires on an actual transition into `to` (from `from` when given).
    /// Cleared once the current status is no longer `to`.
    HealthStatusChange {
        from: Option<HealthStatus>,
        to: HealthStatus,
    },
    /// Fires while a matching prediction is present.
    /// Cleared once no matching prediction is present.
    Prediction {
        prediction_type: String,
        min_probability: f64,
        max_eta_minutes: Option<i32>,
    },
    /// Fires while the device is Offline or its last poll is stale.
    /// Cleared once the device is not Offline and its poll is fresh.
    DeviceOffline { max_minutes_since_poll: i32 },
    /// `And` fires when all children fire and clears when any child clears;
    /// `Or` fires when any child fires and clears when all children clear.
    /// An empty composite never fires and is always cleared.
    Composite {
        operator: LogicalOp,
        conditions: Vec<RuleCondition>,
    },
}

/// Comparison operators for metric thresholds.
///
/// Wire form is snake_case (`greater_than`); the pre-1.1 spellings
/// (`greaterthan`) and the short/symbolic forms (`gt`, `>`) are accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonOp {
    #[serde(alias = "greaterthan", alias = "gt", alias = ">")]
    GreaterThan,
    #[serde(alias = "lessthan", alias = "lt", alias = "<")]
    LessThan,
    #[serde(alias = "eq", alias = "==")]
    Equal,
    #[serde(alias = "notequal", alias = "ne", alias = "!=")]
    NotEqual,
    #[serde(alias = "greaterorequal", alias = "ge", alias = ">=")]
    GreaterOrEqual,
    #[serde(alias = "lessorequal", alias = "le", alias = "<=")]
    LessOrEqual,
}

/// Logical operators for composite conditions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogicalOp {
    And,
    Or,
}

/// Context for rule evaluation
#[derive(Debug, Clone)]
pub struct EvaluationContext {
    pub device_id: String,
    pub edge_id: String,
    pub current_status: HealthStatus,
    pub previous_status: Option<HealthStatus>,
    pub metrics: HashMap<String, MetricValue>,
    pub last_poll: DateTime<Utc>,
    pub predictions: Vec<PredictionInfo>,
}

/// Prediction information for rule evaluation
#[derive(Debug, Clone)]
pub struct PredictionInfo {
    pub prediction_type: String,
    pub probability: f64,
    pub eta_minutes: Option<i32>,
}

impl AlertRule {
    /// Evaluate the rule against the current context (disabled rules never fire)
    pub fn evaluate(&self, ctx: &EvaluationContext) -> bool {
        if !self.enabled {
            return false;
        }
        self.condition.evaluate(ctx)
    }

    /// True when an active alert for this rule may be auto-resolved
    /// (a disabled rule is always cleared).
    pub fn is_cleared(&self, ctx: &EvaluationContext) -> bool {
        if !self.enabled {
            return true;
        }
        self.condition.is_cleared(ctx)
    }
}

/// Numeric view of a metric for comparisons: finite numbers as-is,
/// booleans as 1.0/0.0 (flagged so only Equal/NotEqual apply).
enum Numeric {
    Number(f64),
    Bool(f64),
    Unknown,
}

fn numeric(value: Option<&MetricValue>) -> Numeric {
    match value {
        Some(MetricValue::Boolean(b)) => Numeric::Bool(if *b { 1.0 } else { 0.0 }),
        Some(v) => match v.as_f64_finite() {
            Some(n) => Numeric::Number(n),
            None => Numeric::Unknown,
        },
        None => Numeric::Unknown,
    }
}

impl RuleCondition {
    /// Evaluate the condition against the current context
    pub fn evaluate(&self, ctx: &EvaluationContext) -> bool {
        match self {
            RuleCondition::MetricThreshold {
                metric_name,
                operator,
                threshold,
                ..
            } => match numeric(ctx.metrics.get(metric_name)) {
                Numeric::Number(v) => operator.compare(v, *threshold),
                Numeric::Bool(v) => {
                    matches!(operator, ComparisonOp::Equal | ComparisonOp::NotEqual)
                        && operator.compare(v, *threshold)
                }
                Numeric::Unknown => false,
            },
            RuleCondition::HealthStatusChange { from, to } => {
                let Some(previous) = ctx.previous_status else {
                    return false;
                };
                ctx.current_status == *to && previous != *to && from.is_none_or(|f| previous == f)
            }
            RuleCondition::Prediction { .. } => self.matching_prediction_present(ctx),
            RuleCondition::DeviceOffline {
                max_minutes_since_poll,
            } => {
                ctx.current_status == HealthStatus::Offline
                    || poll_is_stale(ctx, *max_minutes_since_poll)
            }
            RuleCondition::Composite {
                operator,
                conditions,
            } => {
                if conditions.is_empty() {
                    return false;
                }
                match operator {
                    LogicalOp::And => conditions.iter().all(|c| c.evaluate(ctx)),
                    LogicalOp::Or => conditions.iter().any(|c| c.evaluate(ctx)),
                }
            }
        }
    }

    /// True when the problem this condition detects has gone away, so an
    /// active alert may be auto-resolved (see the per-variant docs).
    pub fn is_cleared(&self, ctx: &EvaluationContext) -> bool {
        match self {
            RuleCondition::MetricThreshold {
                metric_name,
                operator,
                threshold,
                hysteresis,
                ..
            } => {
                let margin = hysteresis
                    .filter(|h| h.is_finite() && *h > 0.0)
                    .unwrap_or(0.0);
                match numeric(ctx.metrics.get(metric_name)) {
                    Numeric::Number(v) => operator.is_cleared(v, *threshold, margin),
                    // Booleans have no "distance": cleared == not firing
                    Numeric::Bool(v) => {
                        !(matches!(operator, ComparisonOp::Equal | ComparisonOp::NotEqual)
                            && operator.compare(v, *threshold))
                    }
                    // Missing or NaN: unknown is not "recovered"
                    Numeric::Unknown => false,
                }
            }
            RuleCondition::HealthStatusChange { to, .. } => ctx.current_status != *to,
            RuleCondition::Prediction { .. } => !self.matching_prediction_present(ctx),
            RuleCondition::DeviceOffline {
                max_minutes_since_poll,
            } => {
                ctx.current_status != HealthStatus::Offline
                    && !poll_is_stale(ctx, *max_minutes_since_poll)
            }
            RuleCondition::Composite {
                operator,
                conditions,
            } => {
                if conditions.is_empty() {
                    return true;
                }
                match operator {
                    LogicalOp::And => conditions.iter().any(|c| c.is_cleared(ctx)),
                    LogicalOp::Or => conditions.iter().all(|c| c.is_cleared(ctx)),
                }
            }
        }
    }

    fn matching_prediction_present(&self, ctx: &EvaluationContext) -> bool {
        let RuleCondition::Prediction {
            prediction_type,
            min_probability,
            max_eta_minutes,
        } = self
        else {
            return false;
        };
        ctx.predictions.iter().any(|p| {
            p.prediction_type == *prediction_type
                && p.probability >= *min_probability
                && match (max_eta_minutes, &p.eta_minutes) {
                    (Some(max), Some(eta)) => eta <= max,
                    (Some(_), None) => false,
                    _ => true,
                }
        })
    }
}

fn poll_is_stale(ctx: &EvaluationContext, max_minutes_since_poll: i32) -> bool {
    (Utc::now() - ctx.last_poll).num_minutes() > max_minutes_since_poll as i64
}

/// Relative float equality: |a-b| <= 1e-9 * max(1, |a|, |b|).
/// Non-finite operands compare exactly; NaN is never equal.
fn approx_eq(a: f64, b: f64) -> bool {
    if a.is_nan() || b.is_nan() {
        return false;
    }
    if !a.is_finite() || !b.is_finite() {
        return a == b;
    }
    (a - b).abs() <= 1e-9 * 1f64.max(a.abs()).max(b.abs())
}

impl ComparisonOp {
    /// Parse the config/wire spellings (`greater_than`, `greaterthan`,
    /// `gt`, `>` ...). Returns `None` for unknown operators.
    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
    }

    /// Compare `value` against `threshold`. NaN on either side is never
    /// true (including `NotEqual`); `Equal`/`NotEqual` use a relative
    /// tolerance.
    pub fn compare(&self, value: f64, threshold: f64) -> bool {
        if value.is_nan() || threshold.is_nan() {
            return false;
        }
        match self {
            ComparisonOp::GreaterThan => value > threshold,
            ComparisonOp::LessThan => value < threshold,
            ComparisonOp::Equal => approx_eq(value, threshold),
            ComparisonOp::NotEqual => !approx_eq(value, threshold),
            ComparisonOp::GreaterOrEqual => value >= threshold,
            ComparisonOp::LessOrEqual => value <= threshold,
        }
    }

    /// True when `value` is back past `threshold` by at least `margin`
    /// (e.g. `GreaterThan 75`, margin 2: cleared at <= 73). With margin 0
    /// this is exactly "the comparison is false". NaN is never cleared.
    pub fn is_cleared(&self, value: f64, threshold: f64, margin: f64) -> bool {
        if value.is_nan() || threshold.is_nan() {
            return false;
        }
        let margin = if margin.is_finite() && margin > 0.0 {
            margin
        } else {
            0.0
        };
        match self {
            ComparisonOp::GreaterThan => value <= threshold - margin,
            ComparisonOp::GreaterOrEqual => value < threshold - margin,
            ComparisonOp::LessThan => value >= threshold + margin,
            ComparisonOp::LessOrEqual => value > threshold + margin,
            ComparisonOp::Equal => {
                !approx_eq(value, threshold) && (value - threshold).abs() > margin
            }
            // Fires on any deviation, so only an exact return can clear it;
            // a margin here would make fire and clear overlap.
            ComparisonOp::NotEqual => approx_eq(value, threshold),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;
    use std::collections::HashMap;

    fn create_test_context() -> EvaluationContext {
        let mut metrics = HashMap::new();
        metrics.insert("temperature".to_string(), MetricValue::Float(45.0));
        metrics.insert("cpu_usage".to_string(), MetricValue::Float(85.0));
        metrics.insert("pressure".to_string(), MetricValue::Integer(100));

        EvaluationContext {
            device_id: "dev-1".to_string(),
            edge_id: "edge-1".to_string(),
            current_status: HealthStatus::Healthy,
            previous_status: Some(HealthStatus::Healthy),
            metrics,
            last_poll: Utc::now(),
            predictions: vec![],
        }
    }

    fn rule_with(condition: RuleCondition) -> AlertRule {
        AlertRule {
            id: "r".to_string(),
            name: "r".to_string(),
            description: String::new(),
            enabled: true,
            severity: Severity::Warning,
            condition,
            cooldown_minutes: 5,
            notification_channels: vec![],
            suppress_repeat: false,
            max_firing_count: None,
            action: None,
        }
    }

    fn threshold(metric: &str, op: ComparisonOp, t: f64, h: Option<f64>) -> RuleCondition {
        RuleCondition::MetricThreshold {
            metric_name: metric.to_string(),
            operator: op,
            threshold: t,
            duration_minutes: None,
            hysteresis: h,
        }
    }

    fn ctx_with_metric(name: &str, v: MetricValue) -> EvaluationContext {
        let mut ctx = create_test_context();
        ctx.metrics.insert(name.to_string(), v);
        ctx
    }

    #[test]
    fn test_metric_threshold_greater_than() {
        let rule = AlertRule {
            id: "rule-1".to_string(),
            name: "High Temperature".to_string(),
            description: "Temperature exceeds threshold".to_string(),
            enabled: true,
            severity: Severity::Critical,
            condition: threshold("temperature", ComparisonOp::GreaterThan, 40.0, None),
            cooldown_minutes: 5,
            notification_channels: vec![],
            suppress_repeat: false,
            max_firing_count: None,
            action: None,
        };

        let ctx = create_test_context();
        assert!(rule.evaluate(&ctx));
        assert!(!rule.is_cleared(&ctx));
    }

    #[test]
    fn test_metric_threshold_below_threshold() {
        let rule = rule_with(threshold("cpu_usage", ComparisonOp::LessThan, 90.0, None));
        let ctx = create_test_context();
        assert!(rule.evaluate(&ctx));
    }

    #[test]
    fn test_integer_metric() {
        let rule = rule_with(threshold(
            "pressure",
            ComparisonOp::GreaterOrEqual,
            100.0,
            None,
        ));
        assert!(rule.evaluate(&create_test_context()));
    }

    #[test]
    fn test_health_status_change() {
        let rule = rule_with(RuleCondition::HealthStatusChange {
            from: Some(HealthStatus::Healthy),
            to: HealthStatus::Error,
        });

        let mut ctx = create_test_context();
        ctx.current_status = HealthStatus::Error;
        ctx.previous_status = Some(HealthStatus::Healthy);
        assert!(rule.evaluate(&ctx));

        // Wrong origin state: no fire
        ctx.previous_status = Some(HealthStatus::Warning);
        assert!(!rule.evaluate(&ctx));
    }

    #[test]
    fn test_health_status_change_from_none_fires_only_on_transition() {
        let rule = rule_with(RuleCondition::HealthStatusChange {
            from: None,
            to: HealthStatus::Error,
        });
        let mut ctx = create_test_context();

        // Transition Healthy -> Error: fires
        ctx.previous_status = Some(HealthStatus::Healthy);
        ctx.current_status = HealthStatus::Error;
        assert!(rule.evaluate(&ctx));

        // Staying in Error: must NOT fire again
        ctx.previous_status = Some(HealthStatus::Error);
        assert!(!rule.evaluate(&ctx));
        // ... but is not cleared either (still in `to`)
        assert!(!rule.is_cleared(&ctx));

        // First sighting (no previous): not a transition
        ctx.previous_status = None;
        assert!(!rule.evaluate(&ctx));

        // Recovered: cleared even though the transition condition is false
        ctx.previous_status = Some(HealthStatus::Error);
        ctx.current_status = HealthStatus::Healthy;
        assert!(!rule.evaluate(&ctx));
        assert!(rule.is_cleared(&ctx));
    }

    #[test]
    fn test_disabled_rule() {
        let mut rule = rule_with(threshold(
            "temperature",
            ComparisonOp::GreaterThan,
            0.0,
            None,
        ));
        rule.enabled = false;
        let ctx = create_test_context();
        assert!(!rule.evaluate(&ctx));
        assert!(rule.is_cleared(&ctx));
    }

    #[test]
    fn test_comparison_op_serde_snake_case_and_aliases() {
        assert_eq!(
            serde_json::to_string(&ComparisonOp::GreaterThan).unwrap(),
            "\"greater_than\""
        );
        assert_eq!(
            serde_json::to_string(&ComparisonOp::GreaterOrEqual).unwrap(),
            "\"greater_or_equal\""
        );
        let cases = [
            ("greater_than", ComparisonOp::GreaterThan),
            ("greaterthan", ComparisonOp::GreaterThan),
            ("gt", ComparisonOp::GreaterThan),
            ("less_than", ComparisonOp::LessThan),
            ("lessthan", ComparisonOp::LessThan),
            ("equal", ComparisonOp::Equal),
            ("not_equal", ComparisonOp::NotEqual),
            ("notequal", ComparisonOp::NotEqual),
            ("greater_or_equal", ComparisonOp::GreaterOrEqual),
            ("greaterorequal", ComparisonOp::GreaterOrEqual),
            ("less_or_equal", ComparisonOp::LessOrEqual),
            ("lessorequal", ComparisonOp::LessOrEqual),
            ("<=", ComparisonOp::LessOrEqual),
        ];
        for (s, op) in cases {
            assert_eq!(
                serde_json::from_str::<ComparisonOp>(&format!("\"{}\"", s)).unwrap(),
                op,
                "{}",
                s
            );
            assert_eq!(ComparisonOp::parse(s), Some(op));
        }
        assert_eq!(ComparisonOp::parse("bogus"), None);

        // Whole condition, both spellings
        for op in ["greater_than", "greaterthan"] {
            let json = format!(
                r#"{{"type":"metric_threshold","metric_name":"t","operator":"{}","threshold":75.0,"duration_minutes":null}}"#,
                op
            );
            let c: RuleCondition = serde_json::from_str(&json).unwrap();
            assert!(matches!(
                c,
                RuleCondition::MetricThreshold {
                    operator: ComparisonOp::GreaterThan,
                    hysteresis: None,
                    ..
                }
            ));
        }
        let c: RuleCondition = serde_json::from_str(
            r#"{"type":"metric_threshold","metric_name":"t","operator":"greater_than","threshold":75.0,"hysteresis":2.0}"#,
        )
        .unwrap();
        assert!(matches!(
            c,
            RuleCondition::MetricThreshold {
                hysteresis: Some(h),
                duration_minutes: None,
                ..
            } if h == 2.0
        ));
    }

    #[test]
    fn test_equal_relative_tolerance() {
        let eq = ComparisonOp::Equal;
        let ne = ComparisonOp::NotEqual;
        assert!(eq.compare(0.1 + 0.2, 0.3));
        assert!(!ne.compare(0.1 + 0.2, 0.3));
        // Large magnitudes: absolute EPSILON would fail here
        assert!(eq.compare(1.0e12 + 1.0e-4, 1.0e12));
        assert!(!eq.compare(1.0e12 + 1.0e4, 1.0e12));
        assert!(eq.compare(5.0, 5.0));
        assert!(!eq.compare(5.0, 5.001));
        assert!(ne.compare(5.0, 5.001));
        assert!(eq.compare(f64::INFINITY, f64::INFINITY));
        assert!(!eq.compare(f64::INFINITY, 5.0));
    }

    #[test]
    fn test_nan_never_fires() {
        for op in [
            ComparisonOp::GreaterThan,
            ComparisonOp::LessThan,
            ComparisonOp::Equal,
            ComparisonOp::NotEqual,
            ComparisonOp::GreaterOrEqual,
            ComparisonOp::LessOrEqual,
        ] {
            assert!(!op.compare(f64::NAN, 1.0), "{:?}", op);
            assert!(!op.compare(1.0, f64::NAN), "{:?}", op);
            assert!(!op.is_cleared(f64::NAN, 1.0, 0.0), "{:?}", op);
            let rule = rule_with(threshold("t", op, 1.0, None));
            let ctx = ctx_with_metric("t", MetricValue::Float(f64::NAN));
            assert!(!rule.evaluate(&ctx), "{:?}", op);
            // Unknown value is not treated as recovered
            assert!(!rule.is_cleared(&ctx), "{:?}", op);
        }
    }

    #[test]
    fn test_boolean_metric_equal_not_equal() {
        let ctx_true = ctx_with_metric("ok", MetricValue::Boolean(true));
        let ctx_false = ctx_with_metric("ok", MetricValue::Boolean(false));

        let is_false = rule_with(threshold("ok", ComparisonOp::Equal, 0.0, None));
        assert!(is_false.evaluate(&ctx_false));
        assert!(!is_false.evaluate(&ctx_true));
        assert!(is_false.is_cleared(&ctx_true));
        assert!(!is_false.is_cleared(&ctx_false));

        let not_true = rule_with(threshold("ok", ComparisonOp::NotEqual, 1.0, None));
        assert!(not_true.evaluate(&ctx_false));
        assert!(!not_true.evaluate(&ctx_true));

        // Ordering operators do not apply to booleans
        let gt = rule_with(threshold("ok", ComparisonOp::GreaterThan, 0.5, None));
        assert!(!gt.evaluate(&ctx_true));
    }

    #[test]
    fn test_string_and_missing_metric() {
        let ctx = ctx_with_metric("s", MetricValue::String("hot".into()));
        let rule = rule_with(threshold("s", ComparisonOp::NotEqual, 0.0, None));
        assert!(!rule.evaluate(&ctx));
        assert!(!rule.is_cleared(&ctx));
        let missing = rule_with(threshold("nope", ComparisonOp::GreaterThan, 0.0, None));
        assert!(!missing.evaluate(&ctx));
        assert!(!missing.is_cleared(&ctx));
    }

    #[test]
    fn test_threshold_hysteresis() {
        let rule = rule_with(threshold("t", ComparisonOp::GreaterThan, 75.0, Some(2.0)));
        let at = |v: f64| ctx_with_metric("t", MetricValue::Float(v));

        assert!(rule.evaluate(&at(76.0)));
        assert!(!rule.is_cleared(&at(76.0)));
        // Inside the band: neither firing nor cleared
        assert!(!rule.evaluate(&at(74.0)));
        assert!(!rule.is_cleared(&at(74.0)));
        assert!(!rule.is_cleared(&at(73.5)));
        // Past the band: cleared
        assert!(rule.is_cleared(&at(73.0)));
        assert!(rule.is_cleared(&at(60.0)));

        // No hysteresis: cleared exactly when not firing
        let plain = rule_with(threshold("t", ComparisonOp::GreaterThan, 75.0, None));
        assert!(plain.is_cleared(&at(75.0)));
        assert!(!plain.is_cleared(&at(75.1)));

        // Negative / NaN hysteresis treated as 0
        let neg = rule_with(threshold("t", ComparisonOp::GreaterThan, 75.0, Some(-5.0)));
        assert!(neg.is_cleared(&at(75.0)));
        let nan = rule_with(threshold(
            "t",
            ComparisonOp::GreaterThan,
            75.0,
            Some(f64::NAN),
        ));
        assert!(nan.is_cleared(&at(75.0)));
    }

    #[test]
    fn test_hysteresis_all_operators() {
        let lt = ComparisonOp::LessThan;
        assert!(!lt.is_cleared(11.0, 10.0, 2.0));
        assert!(lt.is_cleared(12.0, 10.0, 2.0));
        assert!(lt.is_cleared(10.0, 10.0, 0.0));

        let le = ComparisonOp::LessOrEqual;
        assert!(!le.is_cleared(10.0, 10.0, 0.0));
        assert!(le.is_cleared(10.1, 10.0, 0.0));
        assert!(!le.is_cleared(12.0, 10.0, 2.0));
        assert!(le.is_cleared(12.1, 10.0, 2.0));

        let ge = ComparisonOp::GreaterOrEqual;
        assert!(!ge.is_cleared(10.0, 10.0, 0.0));
        assert!(ge.is_cleared(9.9, 10.0, 0.0));
        assert!(!ge.is_cleared(8.0, 10.0, 2.0));
        assert!(ge.is_cleared(7.9, 10.0, 2.0));

        let eq = ComparisonOp::Equal;
        assert!(!eq.is_cleared(5.0, 5.0, 0.0));
        assert!(eq.is_cleared(5.1, 5.0, 0.0));
        assert!(!eq.is_cleared(5.5, 5.0, 1.0));
        assert!(eq.is_cleared(6.5, 5.0, 1.0));

        let ne = ComparisonOp::NotEqual;
        assert!(ne.is_cleared(5.0, 5.0, 0.0));
        assert!(!ne.is_cleared(5.1, 5.0, 0.0));
        // Never fire and clear on the same value, whatever the margin
        assert!(ne.compare(5.5, 5.0));
        assert!(!ne.is_cleared(5.5, 5.0, 1.0));
        assert!(ne.is_cleared(5.0, 5.0, 1.0));
    }

    #[test]
    fn test_prediction_fire_and_clear() {
        let rule = rule_with(RuleCondition::Prediction {
            prediction_type: "overheating".to_string(),
            min_probability: 0.7,
            max_eta_minutes: Some(60),
        });
        let mut ctx = create_test_context();
        assert!(!rule.evaluate(&ctx));
        assert!(rule.is_cleared(&ctx));

        ctx.predictions.push(PredictionInfo {
            prediction_type: "overheating".to_string(),
            probability: 0.9,
            eta_minutes: Some(30),
        });
        assert!(rule.evaluate(&ctx));
        assert!(!rule.is_cleared(&ctx));

        // Below probability / beyond ETA does not match
        ctx.predictions[0].probability = 0.5;
        assert!(rule.is_cleared(&ctx));
        ctx.predictions[0].probability = 0.9;
        ctx.predictions[0].eta_minutes = Some(120);
        assert!(rule.is_cleared(&ctx));
        ctx.predictions[0].eta_minutes = None;
        assert!(rule.is_cleared(&ctx));
    }

    #[test]
    fn test_device_offline_fire_and_clear() {
        let rule = rule_with(RuleCondition::DeviceOffline {
            max_minutes_since_poll: 5,
        });
        let mut ctx = create_test_context();
        assert!(!rule.evaluate(&ctx));
        assert!(rule.is_cleared(&ctx));

        ctx.current_status = HealthStatus::Offline;
        assert!(rule.evaluate(&ctx));
        assert!(!rule.is_cleared(&ctx));

        // Back online with a fresh poll: cleared
        ctx.current_status = HealthStatus::Warning;
        assert!(rule.is_cleared(&ctx));

        // Stale poll counts as offline: fires, not cleared
        ctx.last_poll = Utc::now() - chrono::Duration::minutes(30);
        assert!(rule.evaluate(&ctx));
        assert!(!rule.is_cleared(&ctx));
    }

    #[test]
    fn test_composite_and_or_clear_semantics() {
        let hot = threshold("temperature", ComparisonOp::GreaterThan, 40.0, None); // fires (45)
        let busy = threshold("cpu_usage", ComparisonOp::GreaterThan, 80.0, None); // fires (85)
        let idle = threshold("cpu_usage", ComparisonOp::LessThan, 10.0, None); // cleared (85)

        let and_fire = rule_with(RuleCondition::Composite {
            operator: LogicalOp::And,
            conditions: vec![hot.clone(), busy.clone()],
        });
        let ctx = create_test_context();
        assert!(and_fire.evaluate(&ctx));
        assert!(!and_fire.is_cleared(&ctx));

        // And clears as soon as any child clears
        let and_one_clear = rule_with(RuleCondition::Composite {
            operator: LogicalOp::And,
            conditions: vec![hot.clone(), idle.clone()],
        });
        assert!(!and_one_clear.evaluate(&ctx));
        assert!(and_one_clear.is_cleared(&ctx));

        // Or: fires with one child, clears only when all clear
        let or_rule = rule_with(RuleCondition::Composite {
            operator: LogicalOp::Or,
            conditions: vec![hot.clone(), idle.clone()],
        });
        assert!(or_rule.evaluate(&ctx));
        assert!(!or_rule.is_cleared(&ctx));

        let mut cool = create_test_context();
        cool.metrics
            .insert("temperature".to_string(), MetricValue::Float(20.0));
        assert!(!or_rule.evaluate(&cool));
        assert!(or_rule.is_cleared(&cool));

        // Or with an unknown child is not cleared (unknown != recovered)
        let or_unknown = rule_with(RuleCondition::Composite {
            operator: LogicalOp::Or,
            conditions: vec![
                idle,
                threshold("missing", ComparisonOp::GreaterThan, 1.0, None),
            ],
        });
        assert!(!or_unknown.is_cleared(&ctx));

        // Empty composite never fires, always cleared
        for op in [LogicalOp::And, LogicalOp::Or] {
            let empty = rule_with(RuleCondition::Composite {
                operator: op,
                conditions: vec![],
            });
            assert!(!empty.evaluate(&ctx));
            assert!(empty.is_cleared(&ctx));
        }
    }

    #[test]
    fn test_composite_with_transition_child() {
        // And(transition to Error, hot): clears once status leaves Error
        let rule = rule_with(RuleCondition::Composite {
            operator: LogicalOp::And,
            conditions: vec![
                RuleCondition::HealthStatusChange {
                    from: None,
                    to: HealthStatus::Error,
                },
                threshold("temperature", ComparisonOp::GreaterThan, 40.0, None),
            ],
        });
        let mut ctx = create_test_context();
        ctx.previous_status = Some(HealthStatus::Healthy);
        ctx.current_status = HealthStatus::Error;
        assert!(rule.evaluate(&ctx));
        ctx.previous_status = Some(HealthStatus::Error);
        assert!(!rule.evaluate(&ctx));
        assert!(!rule.is_cleared(&ctx));
        ctx.current_status = HealthStatus::Healthy;
        assert!(rule.is_cleared(&ctx));
    }
}
