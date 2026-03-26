//! Alert management for NIMon
//!
//! Provides alert rule evaluation, severity classification, and
//! alert status tracking for device anomalies and predictions.

pub mod rules;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Alert severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "info"),
            Self::Warning => write!(f, "warning"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

/// Alert status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertStatus {
    Pending,
    Firing,
    Resolved,
    Suppressed,
}

/// A generated alert
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub rule_id: String,
    pub edge_id: String,
    pub device_id: String,
    pub severity: AlertSeverity,
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
    Email { smtp_server: String, from_addr: String, to_addrs: Vec<String> },
    Webhook { url: String, headers: Option<std::collections::HashMap<String, String>> },
    Slack { webhook_url: String },
    Teams { webhook_url: String },
    Console,
}
