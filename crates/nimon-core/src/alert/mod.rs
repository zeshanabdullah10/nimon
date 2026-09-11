//! Alert management for NIMon
//!
//! Provides alert rule evaluation, severity classification, and
//! alert status tracking for device anomalies and predictions.

pub mod rules;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use crate::types::Severity;

/// Alert status lifecycle:
/// `Pending` (duration window not yet satisfied) → `Firing` (active) →
/// `Resolved` (cleared/acknowledged); `Suppressed` (beyond max_firing_count).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertStatus {
    Pending,
    Firing,
    Resolved,
    Suppressed,
}

impl std::fmt::Display for AlertStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Firing => write!(f, "firing"),
            Self::Resolved => write!(f, "resolved"),
            Self::Suppressed => write!(f, "suppressed"),
        }
    }
}

impl AlertStatus {
    /// Parse from the DB/wire string form. Unknown values map to `Pending`.
    pub fn parse(s: &str) -> Self {
        match s {
            "firing" => AlertStatus::Firing,
            "resolved" => AlertStatus::Resolved,
            "suppressed" => AlertStatus::Suppressed,
            _ => AlertStatus::Pending,
        }
    }

    /// True while the alert still requires attention.
    pub fn is_open(&self) -> bool {
        !matches!(self, AlertStatus::Resolved)
    }
}

/// A generated alert (the single alert type for the whole workspace)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub rule_id: String,
    pub edge_id: String,
    pub device_id: String,
    pub severity: Severity,
    pub status: AlertStatus,
    pub title: String,
    pub message: String,
    pub metric_name: Option<String>,
    pub metric_value: Option<f64>,
    pub threshold: Option<f64>,
    pub triggered_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub fired_count: i32,
    pub notification_sent: bool,
}

/// Notification channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannel {
    pub id: String,
    pub name: String,
    pub channel_type: ChannelType,
    pub enabled: bool,
    pub config: serde_json::Value,
}

/// Types of notification channels
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChannelType {
    Email {
        smtp_server: String,
        smtp_port: u16,
        username: Option<String>,
        from_addr: String,
        to_addrs: Vec<String>,
    },
    Webhook {
        url: String,
        headers: Option<std::collections::HashMap<String, String>>,
    },
    Slack {
        webhook_url: String,
    },
    Teams {
        webhook_url: String,
    },
    Console,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_status_wire_format() {
        assert_eq!(
            serde_json::to_string(&AlertStatus::Firing).unwrap(),
            "\"firing\""
        );
        assert_eq!(
            serde_json::to_string(&AlertStatus::Resolved).unwrap(),
            "\"resolved\""
        );
        assert_eq!(AlertStatus::parse("firing"), AlertStatus::Firing);
        assert_eq!(AlertStatus::parse("legacy-pending"), AlertStatus::Pending);
    }

    #[test]
    fn test_alert_status_is_open() {
        assert!(AlertStatus::Pending.is_open());
        assert!(AlertStatus::Firing.is_open());
        assert!(!AlertStatus::Resolved.is_open());
    }
}
