//! Repository for action history operations

use crate::NimonResult;
use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

const ACTION_COLUMNS: &str = "id, alert_id, device_id, action_id, action_type, command, \
     exit_code, output, duration_ms, success, retry_count, executed_at";

/// Filters for [`ActionRepository::list`], applied in SQL before `LIMIT`.
/// `edge_id` matches actions whose device belongs to that edge.
/// `limit: None` means no limit.
#[derive(Debug, Clone, Default)]
pub struct ActionHistoryFilter {
    pub device_id: Option<String>,
    pub edge_id: Option<String>,
    pub limit: Option<i64>,
}

/// Database record for an action history entry
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActionRecord {
    pub id: i64,
    pub alert_id: Option<String>,
    pub device_id: String,
    pub action_id: String,
    pub action_type: String,
    pub command: Option<String>,
    pub exit_code: Option<i64>,
    pub output: Option<String>,
    pub duration_ms: Option<i64>,
    pub success: Option<i64>,
    pub retry_count: i64,
    pub executed_at: String,
}

pub struct ActionRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ActionRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a new action history entry (`executed_at` = now, RFC3339).
    /// Returns the new action ID.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert(
        &self,
        alert_id: Option<String>,
        device_id: &str,
        action_id: &str,
        action_type: &str,
        command: Option<&str>,
        exit_code: Option<i64>,
        output: Option<&str>,
        duration_ms: Option<i64>,
        success: bool,
        retry_count: i64,
    ) -> NimonResult<i64> {
        let success_int: i64 = if success { 1 } else { 0 };
        let result = sqlx::query(
            r#"
            INSERT INTO action_history (alert_id, device_id, action_id, action_type, command, exit_code, output, duration_ms, success, retry_count, executed_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(alert_id)
        .bind(device_id)
        .bind(action_id)
        .bind(action_type)
        .bind(command)
        .bind(exit_code)
        .bind(output)
        .bind(duration_ms)
        .bind(success_int)
        .bind(retry_count)
        .bind(Utc::now().to_rfc3339())
        .execute(self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Filtered action history, most recent first.
    pub async fn list(&self, filter: &ActionHistoryFilter) -> NimonResult<Vec<ActionRecord>> {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(format!(
            "SELECT {ACTION_COLUMNS} FROM action_history WHERE 1 = 1"
        ));
        if let Some(device_id) = &filter.device_id {
            qb.push(" AND device_id = ").push_bind(device_id.clone());
        }
        if let Some(edge_id) = &filter.edge_id {
            qb.push(" AND device_id IN (SELECT id FROM devices WHERE edge_id = ")
                .push_bind(edge_id.clone())
                .push(")");
        }
        qb.push(" ORDER BY executed_at DESC, id DESC LIMIT ")
            .push_bind(filter.limit.unwrap_or(-1));
        Ok(qb
            .build_query_as::<ActionRecord>()
            .fetch_all(self.pool)
            .await?)
    }

    /// Delete action history entries executed before `cutoff`.
    pub async fn prune_older_than(&self, cutoff: DateTime<Utc>) -> NimonResult<u64> {
        let result = sqlx::query("DELETE FROM action_history WHERE executed_at < ?")
            .bind(cutoff.to_rfc3339())
            .execute(self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// List action history entries for a specific device, most recent first, up to `limit` rows.
    pub async fn list_by_device(
        &self,
        device_id: &str,
        limit: i64,
    ) -> NimonResult<Vec<ActionRecord>> {
        let records = sqlx::query_as::<_, ActionRecord>(
            "SELECT id, alert_id, device_id, action_id, action_type, command, exit_code, output, duration_ms, success, retry_count, executed_at FROM action_history WHERE device_id = ? ORDER BY executed_at DESC LIMIT ?"
        )
        .bind(device_id)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }

    /// List the most recent action history entries across all devices, up to `limit` rows.
    pub async fn list_recent(&self, limit: i64) -> NimonResult<Vec<ActionRecord>> {
        let records = sqlx::query_as::<_, ActionRecord>(
            "SELECT id, alert_id, device_id, action_id, action_type, command, exit_code, output, duration_ms, success, retry_count, executed_at FROM action_history ORDER BY executed_at DESC LIMIT ?"
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alert::{Alert, AlertStatus, Severity};
    use crate::db::alert_repo::AlertRepository;
    use crate::db::create_test_db;
    use crate::db::device_repo::DeviceRepository;
    use crate::db::edge_repo::EdgeRepository;
    use crate::{Device, DeviceType, EdgeNode, EdgeStatus};

    async fn setup_test_data(pool: &SqlitePool) -> String {
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

        let alert_repo = AlertRepository::new(pool);
        let alert = Alert {
            id: ulid::Ulid::new().to_string(),
            rule_id: "high_temp".to_string(),
            edge_id: "edge-1".to_string(),
            device_id: "device-1".to_string(),
            severity: Severity::Warning,
            status: AlertStatus::Firing,
            title: "High Temperature".to_string(),
            message: "Temperature too high".to_string(),
            metric_name: None,
            metric_value: None,
            threshold: None,
            triggered_at: chrono::Utc::now(),
            resolved_at: None,
            fired_count: 1,
            notification_sent: false,
        };
        alert_repo.insert(&alert).await.unwrap();

        alert.id
    }

    #[tokio::test]
    async fn test_insert_and_list_by_device() {
        let pool = create_test_db().await;
        let alert_id = setup_test_data(&pool).await;
        let repo = ActionRepository::new(&pool);

        let id = repo
            .insert(
                Some(alert_id.clone()),
                "device-1",
                "action-001",
                "restart",
                Some("systemctl restart ni-daqmx"),
                Some(0),
                Some("Restarted successfully"),
                Some(1500),
                true,
                0,
            )
            .await
            .unwrap();

        assert!(id > 0);

        let actions = repo.list_by_device("device-1", 10).await.unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].action_id, "action-001");
        assert_eq!(actions[0].action_type, "restart");
        assert_eq!(actions[0].success, Some(1));
        assert_eq!(actions[0].alert_id, Some(alert_id.clone()));
    }

    #[tokio::test]
    async fn test_list_recent() {
        let pool = create_test_db().await;
        let alert_id = setup_test_data(&pool).await;
        let repo = ActionRepository::new(&pool);

        repo.insert(
            Some(alert_id.clone()),
            "device-1",
            "act-1",
            "restart",
            None,
            Some(0),
            None,
            Some(100),
            true,
            0,
        )
        .await
        .unwrap();
        repo.insert(
            Some(alert_id.clone()),
            "device-1",
            "act-2",
            "reset",
            None,
            Some(1),
            Some("reset failed"),
            Some(200),
            false,
            1,
        )
        .await
        .unwrap();

        let recent = repo.list_recent(10).await.unwrap();
        assert_eq!(recent.len(), 2);
    }

    #[tokio::test]
    async fn test_list_recent_with_limit() {
        let pool = create_test_db().await;
        let alert_id = setup_test_data(&pool).await;
        let repo = ActionRepository::new(&pool);

        for i in 0..5 {
            repo.insert(
                Some(alert_id.clone()),
                "device-1",
                &format!("act-{}", i),
                "restart",
                None,
                Some(0),
                None,
                Some(100),
                true,
                0,
            )
            .await
            .unwrap();
        }

        let limited = repo.list_recent(3).await.unwrap();
        assert_eq!(limited.len(), 3);
    }

    #[tokio::test]
    async fn test_insert_failed_action() {
        let pool = create_test_db().await;
        let alert_id = setup_test_data(&pool).await;
        let repo = ActionRepository::new(&pool);

        repo.insert(
            Some(alert_id.clone()),
            "device-1",
            "act-fail",
            "reboot",
            Some("shutdown -r now"),
            Some(1),
            Some("Permission denied"),
            Some(50),
            false,
            2,
        )
        .await
        .unwrap();

        let actions = repo.list_by_device("device-1", 10).await.unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].success, Some(0));
        assert_eq!(actions[0].exit_code, Some(1));
        assert_eq!(actions[0].retry_count, 2);
        assert!(chrono::DateTime::parse_from_rfc3339(&actions[0].executed_at).is_ok());
    }

    async fn insert_simple(repo: &ActionRepository<'_>, device_id: &str, action_id: &str) {
        repo.insert(
            None,
            device_id,
            action_id,
            "restart",
            None,
            Some(0),
            None,
            Some(1),
            true,
            0,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_list_with_filter() {
        let pool = create_test_db().await;
        setup_test_data(&pool).await;
        // Second edge + device
        EdgeRepository::new(&pool)
            .upsert(&EdgeNode {
                id: "edge-2".to_string(),
                name: "Edge 2".to_string(),
                hostname: None,
                ip_address: None,
                last_seen: None,
                status: EdgeStatus::Online,
            })
            .await
            .unwrap();
        DeviceRepository::new(&pool)
            .upsert_snapshot("edge-2:Dev1", "edge-2")
            .await
            .unwrap();

        let repo = ActionRepository::new(&pool);
        for i in 0..3 {
            insert_simple(&repo, "device-1", &format!("a{}", i)).await;
        }
        insert_simple(&repo, "edge-2:Dev1", "b0").await;

        let all = repo.list(&ActionHistoryFilter::default()).await.unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].action_id, "b0", "most recent first");

        let dev = repo
            .list(&ActionHistoryFilter {
                device_id: Some("device-1".to_string()),
                limit: Some(2),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(dev.len(), 2);
        assert!(dev.iter().all(|a| a.device_id == "device-1"));

        let edge = repo
            .list(&ActionHistoryFilter {
                edge_id: Some("edge-2".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(edge.len(), 1);
        assert_eq!(edge[0].action_id, "b0");

        let none = repo
            .list(&ActionHistoryFilter {
                device_id: Some("device-1".to_string()),
                edge_id: Some("edge-2".to_string()),
                limit: None,
            })
            .await
            .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn test_prune_older_than() {
        let pool = create_test_db().await;
        setup_test_data(&pool).await;
        let repo = ActionRepository::new(&pool);
        insert_simple(&repo, "device-1", "old").await;
        insert_simple(&repo, "device-1", "new").await;
        let old = (chrono::Utc::now() - chrono::Duration::days(90)).to_rfc3339();
        sqlx::query("UPDATE action_history SET executed_at = ? WHERE action_id = 'old'")
            .bind(&old)
            .execute(&pool)
            .await
            .unwrap();

        let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
        assert_eq!(repo.prune_older_than(cutoff).await.unwrap(), 1);
        let left = repo.list_recent(10).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].action_id, "new");
    }
}
