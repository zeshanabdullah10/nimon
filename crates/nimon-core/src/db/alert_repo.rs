//! Repository for alert operations

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::alert::{Alert, AlertStatus, Severity};
use crate::NimonResult;

/// Database record for an alert
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AlertRecord {
    pub id: String,
    pub device_id: Option<String>,
    pub edge_id: Option<String>,
    pub rule_name: String,
    pub severity: String,
    pub message: String,
    pub title: Option<String>,
    pub metric_name: Option<String>,
    pub metric_value: Option<f64>,
    pub threshold: Option<f64>,
    pub channels: Option<String>,
    pub status: String,
    pub action_taken: Option<String>,
    pub action_result: Option<String>,
    pub notification_sent: i64,
    pub fired_count: i64,
    pub triggered_at: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

impl AlertRecord {
    /// Convert the DB record into the core `Alert` type.
    pub fn to_alert(&self) -> Alert {
        Alert {
            id: self.id.clone(),
            rule_id: self.rule_name.clone(),
            edge_id: self.edge_id.clone().unwrap_or_default(),
            device_id: self.device_id.clone().unwrap_or_default(),
            severity: Severity::parse(&self.severity).unwrap_or(Severity::Warning),
            status: AlertStatus::parse(&self.status),
            title: self.title.clone().unwrap_or_else(|| self.rule_name.clone()),
            message: self.message.clone(),
            metric_name: self.metric_name.clone(),
            metric_value: self.metric_value,
            threshold: self.threshold,
            triggered_at: self
                .triggered_at
                .as_deref()
                .and_then(parse_ts)
                .unwrap_or_else(Utc::now),
            resolved_at: self.resolved_at.as_deref().and_then(parse_ts),
            fired_count: self.fired_count as i32,
            notification_sent: self.notification_sent != 0,
        }
    }
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn optional_str(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

const ALERT_COLUMNS: &str = "id, device_id, edge_id, rule_name, severity, message, title, \
     metric_name, metric_value, threshold, channels, status, action_taken, action_result, \
     notification_sent, fired_count, triggered_at, created_at, resolved_at";

pub struct AlertRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> AlertRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a new alert. The caller supplies the (ULID) id.
    pub async fn insert(&self, alert: &Alert) -> NimonResult<()> {
        sqlx::query(
            r#"
            INSERT INTO alerts (id, device_id, edge_id, rule_name, severity, message, title,
                                metric_name, metric_value, threshold, status, notification_sent,
                                fired_count, triggered_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&alert.id)
        .bind(optional_str(&alert.device_id))
        .bind(optional_str(&alert.edge_id))
        .bind(&alert.rule_id)
        .bind(alert.severity.to_string())
        .bind(&alert.message)
        .bind(&alert.title)
        .bind(&alert.metric_name)
        .bind(alert.metric_value)
        .bind(alert.threshold)
        .bind(alert.status.to_string())
        .bind(alert.notification_sent as i64)
        .bind(alert.fired_count as i64)
        .bind(alert.triggered_at.to_rfc3339())
        .execute(self.pool)
        .await?;

        Ok(())
    }

    /// Resolve an alert by setting status='resolved' and resolved_at to now.
    /// Returns the number of rows affected (0 when the id is unknown).
    pub async fn resolve(&self, alert_id: &str) -> NimonResult<u64> {
        let now = Utc::now().to_rfc3339();
        let result =
            sqlx::query("UPDATE alerts SET status = 'resolved', resolved_at = ? WHERE id = ?")
                .bind(&now)
                .bind(alert_id)
                .execute(self.pool)
                .await?;
        Ok(result.rows_affected())
    }

    /// Mark that notifications were dispatched for this alert.
    pub async fn mark_notified(&self, alert_id: &str) -> NimonResult<()> {
        sqlx::query("UPDATE alerts SET notification_sent = 1 WHERE id = ?")
            .bind(alert_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Record the remediation action taken for this alert.
    pub async fn update_action(
        &self,
        alert_id: &str,
        action_taken: &str,
        action_result: &str,
    ) -> NimonResult<()> {
        sqlx::query("UPDATE alerts SET action_taken = ?, action_result = ? WHERE id = ?")
            .bind(action_taken)
            .bind(action_result)
            .bind(alert_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// List all alerts that are not yet resolved.
    pub async fn list_active(&self) -> NimonResult<Vec<AlertRecord>> {
        let records = sqlx::query_as::<_, AlertRecord>(&format!(
            "SELECT {ALERT_COLUMNS} FROM alerts WHERE status != 'resolved' ORDER BY created_at DESC"
        ))
        .fetch_all(self.pool)
        .await?;
        Ok(records)
    }

    /// List alerts for a specific device, most recent first, up to `limit` rows.
    pub async fn list_by_device(
        &self,
        device_id: &str,
        limit: i64,
    ) -> NimonResult<Vec<AlertRecord>> {
        let records = sqlx::query_as::<_, AlertRecord>(&format!(
            "SELECT {ALERT_COLUMNS} FROM alerts WHERE device_id = ? ORDER BY created_at DESC LIMIT ?"
        ))
        .bind(device_id)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(records)
    }

    /// List recent alerts (all statuses), most recent first.
    pub async fn list_recent(&self, limit: i64) -> NimonResult<Vec<AlertRecord>> {
        let records = sqlx::query_as::<_, AlertRecord>(&format!(
            "SELECT {ALERT_COLUMNS} FROM alerts ORDER BY created_at DESC LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(records)
    }

    /// Prune resolved alerts older than the given number of days.
    pub async fn prune_resolved(&self, older_than_days: i64) -> NimonResult<u64> {
        let cutoff = (Utc::now() - chrono::Duration::days(older_than_days)).to_rfc3339();
        let result = sqlx::query("DELETE FROM alerts WHERE status = 'resolved' AND resolved_at IS NOT NULL AND resolved_at < ?")
            .bind(&cutoff)
            .execute(self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::create_test_db;
    use crate::db::device_repo::DeviceRepository;
    use crate::db::edge_repo::EdgeRepository;
    use crate::{Device, DeviceType, EdgeNode, EdgeStatus};

    async fn setup_test_data(pool: &SqlitePool) -> Alert {
        let edge_repo = EdgeRepository::new(pool);
        edge_repo
            .upsert(&EdgeNode {
                id: "edge-1".to_string(),
                name: "Edge 1".to_string(),
                hostname: None,
                ip_address: None,
                last_seen: None,
                status: EdgeStatus::Online,
            })
            .await
            .unwrap();

        let device_repo = DeviceRepository::new(pool);
        device_repo
            .upsert(&Device {
                id: "device-1".to_string(),
                edge_id: "edge-1".to_string(),
                device_name: "DAQ-1".to_string(),
                device_type: DeviceType::Daq,
                model: None,
                serial_number: None,
                firmware_version: None,
                driver_version: None,
                ip_address: None,
                slot: None,
                chassis: None,
                is_simulated: false,
            })
            .await
            .unwrap();

        Alert {
            id: ulid::Ulid::new().to_string(),
            rule_id: "high_temperature".to_string(),
            edge_id: "edge-1".to_string(),
            device_id: "device-1".to_string(),
            severity: Severity::Warning,
            status: crate::alert::AlertStatus::Firing,
            title: "High Temperature: device-1".to_string(),
            message: "Temperature exceeded 80C".to_string(),
            metric_name: Some("temperature".to_string()),
            metric_value: Some(81.2),
            threshold: Some(80.0),
            triggered_at: Utc::now(),
            resolved_at: None,
            fired_count: 1,
            notification_sent: false,
        }
    }

    #[tokio::test]
    async fn test_insert_and_list_active() {
        let pool = create_test_db().await;
        let alert = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);

        repo.insert(&alert).await.unwrap();

        let active = repo.list_active().await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].rule_name, "high_temperature");
        assert_eq!(active[0].status, "firing");
        assert_eq!(active[0].metric_value, Some(81.2));
    }

    #[tokio::test]
    async fn test_record_to_alert_roundtrip() {
        let pool = create_test_db().await;
        let alert = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);

        repo.insert(&alert).await.unwrap();
        let records = repo.list_active().await.unwrap();
        let converted = records[0].to_alert();
        assert_eq!(converted.id, alert.id);
        assert_eq!(converted.severity, Severity::Warning);
        assert_eq!(converted.metric_name.as_deref(), Some("temperature"));
    }

    #[tokio::test]
    async fn test_resolve_alert() {
        let pool = create_test_db().await;
        let alert = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);

        repo.insert(&alert).await.unwrap();
        repo.resolve(&alert.id).await.unwrap();

        let active = repo.list_active().await.unwrap();
        assert_eq!(active.len(), 0);

        let recent = repo.list_recent(10).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].status, "resolved");
        assert!(recent[0].resolved_at.is_some());
    }

    #[tokio::test]
    async fn test_list_by_device() {
        let pool = create_test_db().await;
        let alert = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);

        repo.insert(&alert).await.unwrap();
        let other = Alert {
            id: ulid::Ulid::new().to_string(),
            rule_id: "rule_b".to_string(),
            ..alert.clone()
        };
        repo.insert(&other).await.unwrap();
        let edge_only = Alert {
            id: ulid::Ulid::new().to_string(),
            device_id: String::new(),
            rule_id: "rule_c".to_string(),
            ..alert.clone()
        };
        repo.insert(&edge_only).await.unwrap();

        let device_alerts = repo.list_by_device("device-1", 10).await.unwrap();
        assert_eq!(device_alerts.len(), 2);
    }

    #[tokio::test]
    async fn test_notification_and_action_updates() {
        let pool = create_test_db().await;
        let alert = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);

        repo.insert(&alert).await.unwrap();
        repo.mark_notified(&alert.id).await.unwrap();
        repo.update_action(&alert.id, "restart_service", "success: exit 0")
            .await
            .unwrap();

        let records = repo.list_recent(10).await.unwrap();
        assert_eq!(records[0].notification_sent, 1);
        assert_eq!(records[0].action_taken.as_deref(), Some("restart_service"));
        assert_eq!(records[0].action_result.as_deref(), Some("success: exit 0"));
    }

    #[tokio::test]
    async fn test_prune_resolved() {
        let pool = create_test_db().await;
        let alert = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);

        repo.insert(&alert).await.unwrap();
        // Nothing resolved yet: nothing pruned
        assert_eq!(repo.prune_resolved(30).await.unwrap(), 0);

        repo.resolve(&alert.id).await.unwrap();
        // Resolved now, but recent: nothing pruned
        assert_eq!(repo.prune_resolved(30).await.unwrap(), 0);

        // Backdate resolution far enough to fall outside retention
        let old = (Utc::now() - chrono::Duration::days(60)).to_rfc3339();
        sqlx::query("UPDATE alerts SET resolved_at = ? WHERE id = ?")
            .bind(&old)
            .bind(&alert.id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(repo.prune_resolved(30).await.unwrap(), 1);
        assert_eq!(repo.list_recent(10).await.unwrap().len(), 0);
    }
}
