//! Repository for alert operations

use sqlx::SqlitePool;
use crate::NimonResult;

/// Database record for an alert
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AlertRecord {
    pub id: i64,
    pub device_id: Option<String>,
    pub edge_id: Option<String>,
    pub rule_name: String,
    pub severity: String,
    pub message: String,
    pub channels: Option<String>,
    pub status: String,
    pub action_taken: Option<String>,
    pub action_result: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

pub struct AlertRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> AlertRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a new alert with status='pending'. Returns the new alert ID.
    pub async fn insert(
        &self,
        device_id: Option<&str>,
        edge_id: Option<&str>,
        rule_name: &str,
        severity: &str,
        message: &str,
    ) -> NimonResult<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO alerts (device_id, edge_id, rule_name, severity, message, status)
            VALUES (?, ?, ?, ?, ?, 'pending')
            "#
        )
        .bind(device_id)
        .bind(edge_id)
        .bind(rule_name)
        .bind(severity)
        .bind(message)
        .execute(self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Resolve an alert by setting status='resolved' and resolved_at to now.
    pub async fn resolve(&self, alert_id: i64) -> NimonResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE alerts SET status = 'resolved', resolved_at = ? WHERE id = ?"
        )
        .bind(&now)
        .bind(alert_id)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    /// List all alerts that are not yet resolved.
    pub async fn list_active(&self) -> NimonResult<Vec<AlertRecord>> {
        let records = sqlx::query_as::<_, AlertRecord>(
            "SELECT id, device_id, edge_id, rule_name, severity, message, channels, status, action_taken, action_result, created_at, resolved_at FROM alerts WHERE status != 'resolved' ORDER BY created_at DESC"
        )
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }

    /// List alerts for a specific device, most recent first, up to `limit` rows.
    pub async fn list_by_device(&self, device_id: &str, limit: i64) -> NimonResult<Vec<AlertRecord>> {
        let records = sqlx::query_as::<_, AlertRecord>(
            "SELECT id, device_id, edge_id, rule_name, severity, message, channels, status, action_taken, action_result, created_at, resolved_at FROM alerts WHERE device_id = ? ORDER BY created_at DESC LIMIT ?"
        )
        .bind(device_id)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::create_test_db;
    use crate::db::edge_repo::EdgeRepository;
    use crate::db::device_repo::DeviceRepository;
    use crate::{EdgeNode, Device, DeviceType, EdgeStatus};

    async fn setup_test_data(pool: &SqlitePool) {
        let edge_repo = EdgeRepository::new(pool);
        edge_repo.upsert(&EdgeNode {
            id: "edge-1".to_string(),
            name: "Edge 1".to_string(),
            hostname: None,
            ip_address: None,
            last_seen: None,
            status: EdgeStatus::Online,
        }).await.unwrap();

        let device_repo = DeviceRepository::new(pool);
        device_repo.upsert(&Device {
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
        }).await.unwrap();
    }

    #[tokio::test]
    async fn test_insert_and_list_active() {
        let pool = create_test_db().await;
        setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);

        let id = repo.insert(
            Some("device-1"),
            Some("edge-1"),
            "high_temperature",
            "warning",
            "Temperature exceeded 80C",
        ).await.unwrap();

        assert!(id > 0);

        let active = repo.list_active().await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].rule_name, "high_temperature");
        assert_eq!(active[0].status, "pending");
    }

    #[tokio::test]
    async fn test_resolve_alert() {
        let pool = create_test_db().await;
        setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);

        let id = repo.insert(
            Some("device-1"),
            Some("edge-1"),
            "disk_full",
            "critical",
            "Disk usage at 99%",
        ).await.unwrap();

        repo.resolve(id).await.unwrap();

        let active = repo.list_active().await.unwrap();
        assert_eq!(active.len(), 0);
    }

    #[tokio::test]
    async fn test_list_by_device() {
        let pool = create_test_db().await;
        setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);

        repo.insert(Some("device-1"), Some("edge-1"), "rule_a", "warning", "msg a").await.unwrap();
        repo.insert(Some("device-1"), Some("edge-1"), "rule_b", "critical", "msg b").await.unwrap();
        repo.insert(None, None, "rule_c", "info", "msg c").await.unwrap();

        let device_alerts = repo.list_by_device("device-1", 10).await.unwrap();
        assert_eq!(device_alerts.len(), 2);
    }

    #[tokio::test]
    async fn test_list_by_device_with_limit() {
        let pool = create_test_db().await;
        setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);

        for i in 0..5 {
            repo.insert(
                Some("device-1"),
                Some("edge-1"),
                &format!("rule_{}", i),
                "warning",
                &format!("msg {}", i),
            ).await.unwrap();
        }

        let limited = repo.list_by_device("device-1", 3).await.unwrap();
        assert_eq!(limited.len(), 3);
    }
}
