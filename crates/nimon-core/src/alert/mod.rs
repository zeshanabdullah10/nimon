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
/// `Acknowledged` (operator saw it; still active, re-notification
/// suppressed) → `Resolved` (cleared); `Suppressed` (beyond
/// max_firing_count).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertStatus {
    Pending,
    Firing,
    Resolved,
    Suppressed,
    /// Still active (not resolved) but acknowledged: suppresses re-notification.
    Acknowledged,
}

impl AlertStatus {
    /// Every variant, in declaration order.
    pub const ALL: [AlertStatus; 5] = [
        AlertStatus::Pending,
        AlertStatus::Firing,
        AlertStatus::Resolved,
        AlertStatus::Suppressed,
        AlertStatus::Acknowledged,
    ];

    /// The wire/DB string form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Firing => "firing",
            Self::Resolved => "resolved",
            Self::Suppressed => "suppressed",
            Self::Acknowledged => "acknowledged",
        }
    }
}

impl std::fmt::Display for AlertStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AlertStatus {
    /// Parse from the DB/wire string form. Unknown values map to `Pending`.
    pub fn parse(s: &str) -> Self {
        match s {
            "firing" => AlertStatus::Firing,
            "resolved" => AlertStatus::Resolved,
            "suppressed" => AlertStatus::Suppressed,
            "acknowledged" => AlertStatus::Acknowledged,
            _ => AlertStatus::Pending,
        }
    }

    /// True while the alert still requires attention (anything but `Resolved`,
    /// including `Suppressed`).
    pub fn is_open(&self) -> bool {
        !matches!(self, AlertStatus::Resolved)
    }

    /// True for the active lifecycle states: `Pending`, `Firing`, `Acknowledged`.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            AlertStatus::Pending | AlertStatus::Firing | AlertStatus::Acknowledged
        )
    }

    /// True when (re-)notification should be sent for this state
    /// (`Acknowledged` and `Suppressed` alerts are silent).
    pub fn should_notify(&self) -> bool {
        matches!(self, AlertStatus::Pending | AlertStatus::Firing)
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
        assert!(AlertStatus::Acknowledged.is_open());
    }

    #[test]
    fn test_alert_status_acknowledged() {
        assert_eq!(
            serde_json::to_string(&AlertStatus::Acknowledged).unwrap(),
            "\"acknowledged\""
        );
        assert_eq!(AlertStatus::Acknowledged.to_string(), "acknowledged");
        assert_eq!(
            AlertStatus::parse("acknowledged"),
            AlertStatus::Acknowledged
        );
        assert!(AlertStatus::Acknowledged.is_active());
        assert!(!AlertStatus::Acknowledged.should_notify());
    }

    #[test]
    fn test_alert_status_is_active_and_roundtrip() {
        assert!(AlertStatus::Pending.is_active());
        assert!(AlertStatus::Firing.is_active());
        assert!(!AlertStatus::Resolved.is_active());
        assert!(!AlertStatus::Suppressed.is_active());
        for s in AlertStatus::ALL {
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(json, format!("\"{}\"", s));
            assert_eq!(AlertStatus::parse(s.as_str()), s);
            assert_eq!(serde_json::from_str::<AlertStatus>(&json).unwrap(), s);
        }
    }
}
