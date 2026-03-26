//! Alert rule evaluation logic

use crate::{HealthStatus, MetricValue};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// An alert rule configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub severity: crate::alert::AlertSeverity,
    pub condition: RuleCondition,
    pub cooldown_minutes: i32,
    pub notification_channels: Vec<String>,
    pub suppress_repeat: bool,
}

/// Rule conditions for triggering alerts
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleCondition {
    MetricThreshold {
        metric_name: String,
        operator: ComparisonOp,
        threshold: f64,
        duration_minutes: Option<i32>,
    },
    HealthStatusChange {
        from: Option<HealthStatus>,
        to: HealthStatus,
    },
    Prediction {
        prediction_type: String,
        min_probability: f64,
        max_eta_minutes: Option<i32>,
    },
    DeviceOffline {
        max_minutes_since_poll: i32,
    },
    Composite {
        operator: LogicalOp,
        conditions: Vec<RuleCondition>,
    },
}

/// Comparison operators for metric thresholds
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComparisonOp {
    GreaterThan,
    LessThan,
    Equal,
    NotEqual,
    GreaterOrEqual,
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
    /// Evaluate the rule against the current context
    pub fn evaluate(&self, ctx: &EvaluationContext) -> bool {
        if !self.enabled {
            return false;
        }
        self.condition.evaluate(ctx)
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
                duration_minutes: _,
            } => {
                if let Some(MetricValue::Float(value)) = ctx.metrics.get(metric_name) {
                    operator.compare(*value, *threshold)
                } else if let Some(MetricValue::Integer(value)) = ctx.metrics.get(metric_name) {
                    operator.compare(*value as f64, *threshold)
                } else {
                    false
                }
            }
            RuleCondition::HealthStatusChange { from, to } => {
                let current_matches = ctx.current_status == *to;
                let previous_matches = match (from, &ctx.previous_status) {
                    (Some(expected), Some(previous)) => previous == expected,
                    (None, _) => true,
                    _ => false,
                };
                current_matches && previous_matches
            }
            RuleCondition::Prediction {
                prediction_type,
                min_probability,
                max_eta_minutes,
            } => {
                ctx.predictions.iter().any(|p| {
                    p.prediction_type == *prediction_type
                        && p.probability >= *min_probability
                        && match (&max_eta_minutes, &p.eta_minutes) {
                            (Some(max), Some(eta)) => eta <= max,
                            (Some(_), None) => false,
                            _ => true,
                        }
                })
            }
            RuleCondition::DeviceOffline {
                max_minutes_since_poll,
            } => {
                let now = Utc::now();
                let minutes_since_poll = (now - ctx.last_poll).num_minutes();
                ctx.current_status == HealthStatus::Offline
                    || minutes_since_poll > *max_minutes_since_poll as i64
            }
            RuleCondition::Composite { operator, conditions } => {
                match operator {
                    LogicalOp::And => conditions.iter().all(|c| c.evaluate(ctx)),
                    LogicalOp::Or => conditions.iter().any(|c| c.evaluate(ctx)),
                }
            }
        }
    }
}

impl ComparisonOp {
    fn compare(&self, value: f64, threshold: f64) -> bool {
        match self {
            ComparisonOp::GreaterThan => value > threshold,
            ComparisonOp::LessThan => value < threshold,
            ComparisonOp::Equal => (value - threshold).abs() < f64::EPSILON,
            ComparisonOp::NotEqual => (value - threshold).abs() >= f64::EPSILON,
            ComparisonOp::GreaterOrEqual => value >= threshold,
            ComparisonOp::LessOrEqual => value <= threshold,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alert::AlertSeverity;
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

    #[test]
    fn test_metric_threshold_greater_than() {
        let rule = AlertRule {
            id: "rule-1".to_string(),
            name: "High Temperature".to_string(),
            description: "Temperature exceeds threshold".to_string(),
            enabled: true,
            severity: AlertSeverity::Critical,
            condition: RuleCondition::MetricThreshold {
                metric_name: "temperature".to_string(),
                operator: ComparisonOp::GreaterThan,
                threshold: 40.0,
                duration_minutes: None,
            },
            cooldown_minutes: 5,
            notification_channels: vec![],
            suppress_repeat: false,
        };

        let ctx = create_test_context();
        assert!(rule.evaluate(&ctx));
    }

    #[test]
    fn test_metric_threshold_below_threshold() {
        let rule = AlertRule {
            id: "rule-2".to_string(),
            name: "Low CPU".to_string(),
            description: "CPU usage below threshold".to_string(),
            enabled: true,
            severity: AlertSeverity::Info,
            condition: RuleCondition::MetricThreshold {
                metric_name: "cpu_usage".to_string(),
                operator: ComparisonOp::LessThan,
                threshold: 90.0,
                duration_minutes: None,
            },
            cooldown_minutes: 5,
            notification_channels: vec![],
            suppress_repeat: false,
        };

        let ctx = create_test_context();
        assert!(rule.evaluate(&ctx));
    }

    #[test]
    fn test_health_status_change() {
        let rule = AlertRule {
            id: "rule-3".to_string(),
            name: "Status Changed".to_string(),
            description: "Device status changed".to_string(),
            enabled: true,
            severity: AlertSeverity::Warning,
            condition: RuleCondition::HealthStatusChange {
                from: Some(HealthStatus::Healthy),
                to: HealthStatus::Error,
            },
            cooldown_minutes: 5,
            notification_channels: vec![],
            suppress_repeat: false,
        };

        let mut ctx = create_test_context();
        ctx.current_status = HealthStatus::Error;
        ctx.previous_status = Some(HealthStatus::Healthy);

        assert!(rule.evaluate(&ctx));
    }

    #[test]
    fn test_disabled_rule() {
        let rule = AlertRule {
            id: "rule-4".to_string(),
            name: "Disabled Rule".to_string(),
            description: "This rule is disabled".to_string(),
            enabled: false,
            severity: AlertSeverity::Warning,
            condition: RuleCondition::MetricThreshold {
                metric_name: "temperature".to_string(),
                operator: ComparisonOp::GreaterThan,
                threshold: 0.0,
                duration_minutes: None,
            },
            cooldown_minutes: 5,
            notification_channels: vec![],
            suppress_repeat: false,
        };

        let ctx = create_test_context();
        assert!(!rule.evaluate(&ctx));
    }
}
