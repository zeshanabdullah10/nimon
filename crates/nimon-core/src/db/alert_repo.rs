//! Repository for alert operations

use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::alert::{Alert, AlertStatus, Severity};
use crate::{NimonError, NimonResult};

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
    /// Last time the alert (re-)fired (RFC3339)
    pub last_fired_at: Option<String>,
    /// When the alert was acknowledged (RFC3339)
    pub acknowledged_at: Option<String>,
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

    /// Parsed status.
    pub fn status(&self) -> AlertStatus {
        AlertStatus::parse(&self.status)
    }
}

/// Filters for [`AlertRepository::list_history`]. Every filter is applied
/// in SQL before `LIMIT`. `since`/`until` bound `triggered_at`
/// (inclusive). `limit: None` means no limit.
#[derive(Debug, Clone, Default)]
pub struct AlertHistoryFilter {
    pub severity: Option<Severity>,
    pub edge_id: Option<String>,
    pub device_id: Option<String>,
    pub status: Option<AlertStatus>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
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
     notification_sent, fired_count, triggered_at, created_at, resolved_at, \
     last_fired_at, acknowledged_at";

/// `WHERE` clause (without the keyword) selecting prunable alerts; `?1` is the cutoff.
const PRUNABLE: &str = "status = 'resolved' AND resolved_at IS NOT NULL AND resolved_at < ?1";

pub struct AlertRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> AlertRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a new alert. The caller supplies the (ULID) id. Fails if the
    /// id already exists; use [`Self::upsert`] for re-fires.
    pub async fn insert(&self, alert: &Alert) -> NimonResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO alerts (id, device_id, edge_id, rule_name, severity, message, title,
                                metric_name, metric_value, threshold, status, notification_sent,
                                fired_count, triggered_at, created_at, last_fired_at,
                                resolved_at, acknowledged_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(&now)
        .bind(alert.triggered_at.to_rfc3339())
        .bind(resolved_at_for(alert))
        .bind(acknowledged_at_for(alert))
        .execute(self.pool)
        .await?;

        Ok(())
    }

    /// Insert the alert, or update the existing row with the same id
    /// (re-fire / state change). `triggered_at`, `created_at` and
    /// `acknowledged_at` of an existing row are kept; `notification_sent`
    /// and `fired_count` never decrease; `last_fired_at` is set to now;
    /// an `acknowledged` row is not reverted to `firing`/`pending`.
    pub async fn upsert(&self, alert: &Alert) -> NimonResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO alerts (id, device_id, edge_id, rule_name, severity, message, title,
                                metric_name, metric_value, threshold, status, notification_sent,
                                fired_count, triggered_at, created_at, last_fired_at,
                                resolved_at, acknowledged_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                severity = excluded.severity,
                message = excluded.message,
                title = excluded.title,
                metric_name = excluded.metric_name,
                metric_value = excluded.metric_value,
                threshold = excluded.threshold,
                status = CASE
                    WHEN alerts.status = 'acknowledged' AND excluded.status IN ('firing', 'pending')
                        THEN alerts.status
                    ELSE excluded.status
                END,
                notification_sent = MAX(alerts.notification_sent, excluded.notification_sent),
                fired_count = MAX(alerts.fired_count, excluded.fired_count),
                last_fired_at = excluded.last_fired_at,
                resolved_at = excluded.resolved_at,
                acknowledged_at = COALESCE(alerts.acknowledged_at, excluded.acknowledged_at)
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
        .bind(&now)
        .bind(&now)
        .bind(resolved_at_for(alert))
        .bind(acknowledged_at_for(alert))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Record a re-fire of an existing alert: `fired_count` (never
    /// decreases), `last_fired_at`, `metric_value` (kept when `None`) and
    /// `message`. Status is unchanged. Returns rows affected (0 when the
    /// id is unknown; the caller should then `upsert` the full alert).
    pub async fn record_refire(
        &self,
        alert_id: &str,
        fired_count: i32,
        last_fired: DateTime<Utc>,
        metric_value: Option<f64>,
        message: &str,
    ) -> NimonResult<u64> {
        let result = sqlx::query(
            "UPDATE alerts SET fired_count = MAX(fired_count, ?), last_fired_at = ?,
                    metric_value = COALESCE(?, metric_value), message = ?
             WHERE id = ?",
        )
        .bind(fired_count as i64)
        .bind(last_fired.to_rfc3339())
        .bind(metric_value.filter(|v| v.is_finite()))
        .bind(message)
        .bind(alert_id)
        .execute(self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Set an alert's status at time `at`:
    /// `Resolved` sets `resolved_at`; `Acknowledged` sets `acknowledged_at`
    /// (ignored for an already-resolved alert); `Firing`/`Pending` reopen
    /// (clear `resolved_at`); `Suppressed` only changes the status.
    /// Returns rows affected (0 when the id is unknown or the change was refused).
    pub async fn set_status(
        &self,
        alert_id: &str,
        status: AlertStatus,
        at: DateTime<Utc>,
    ) -> NimonResult<u64> {
        let result = sqlx::query(
            "UPDATE alerts SET
                status = ?1,
                resolved_at = CASE
                    WHEN ?1 = 'resolved' THEN ?2
                    WHEN ?1 IN ('firing', 'pending') THEN NULL
                    ELSE resolved_at
                END,
                acknowledged_at = CASE WHEN ?1 = 'acknowledged' THEN ?2 ELSE acknowledged_at END
             WHERE id = ?3 AND NOT (?1 = 'acknowledged' AND status = 'resolved')",
        )
        .bind(status.as_str())
        .bind(at.to_rfc3339())
        .bind(alert_id)
        .execute(self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Resolve an alert by setting status='resolved' and resolved_at to now.
    /// Returns the number of rows affected (0 when the id is unknown).
    pub async fn resolve(&self, alert_id: &str) -> NimonResult<u64> {
        self.set_status(alert_id, AlertStatus::Resolved, Utc::now())
            .await
    }

    /// Fetch one alert by id.
    pub async fn get(&self, alert_id: &str) -> NimonResult<AlertRecord> {
        sqlx::query_as::<_, AlertRecord>(&format!(
            "SELECT {ALERT_COLUMNS} FROM alerts WHERE id = ?"
        ))
        .bind(alert_id)
        .fetch_one(self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => NimonError::AlertNotFound(alert_id.to_string()),
            other => NimonError::Database(other),
        })
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

    /// List all alerts that are not yet resolved (includes acknowledged
    /// and suppressed).
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

    /// Filtered alert history, most recently triggered first. All filters
    /// are applied in SQL before the limit.
    pub async fn list_history(&self, filter: &AlertHistoryFilter) -> NimonResult<Vec<AlertRecord>> {
        let mut qb: QueryBuilder<Sqlite> =
            QueryBuilder::new(format!("SELECT {ALERT_COLUMNS} FROM alerts WHERE 1 = 1"));
        if let Some(severity) = filter.severity {
            qb.push(" AND severity = ").push_bind(severity.to_string());
        }
        if let Some(edge_id) = &filter.edge_id {
            qb.push(" AND edge_id = ").push_bind(edge_id.clone());
        }
        if let Some(device_id) = &filter.device_id {
            qb.push(" AND device_id = ").push_bind(device_id.clone());
        }
        if let Some(status) = filter.status {
            qb.push(" AND status = ").push_bind(status.as_str());
        }
        if let Some(since) = filter.since {
            qb.push(" AND triggered_at >= ")
                .push_bind(since.to_rfc3339());
        }
        if let Some(until) = filter.until {
            qb.push(" AND triggered_at <= ")
                .push_bind(until.to_rfc3339());
        }
        qb.push(" ORDER BY triggered_at DESC, created_at DESC, id DESC LIMIT ")
            .push_bind(filter.limit.unwrap_or(-1));
        let records = qb
            .build_query_as::<AlertRecord>()
            .fetch_all(self.pool)
            .await?;
        Ok(records)
    }

    /// Prune resolved alerts older than the given number of days.
    pub async fn prune_resolved(&self, older_than_days: i64) -> NimonResult<u64> {
        self.prune_resolved_before(Utc::now() - chrono::Duration::days(older_than_days))
            .await
    }

    /// Prune alerts resolved before `cutoff`. Action-history references to
    /// the pruned alerts are nulled in the same transaction (the schema
    /// also declares ON DELETE SET NULL), so pruning never fails on FKs.
    pub async fn prune_resolved_before(&self, cutoff: DateTime<Utc>) -> NimonResult<u64> {
        let cutoff = cutoff.to_rfc3339();
        let mut tx = self.pool.begin().await?;
        sqlx::query(&format!(
            "UPDATE action_history SET alert_id = NULL
             WHERE alert_id IN (SELECT id FROM alerts WHERE {PRUNABLE})"
        ))
        .bind(&cutoff)
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(&format!("DELETE FROM alerts WHERE {PRUNABLE}"))
            .bind(&cutoff)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}

/// `resolved_at` to store: the alert's own, or now for a resolved alert without one.
fn resolved_at_for(alert: &Alert) -> Option<String> {
    match (alert.resolved_at, alert.status) {
        (Some(t), _) => Some(t.to_rfc3339()),
        (None, AlertStatus::Resolved) => Some(Utc::now().to_rfc3339()),
        _ => None,
    }
}

/// `acknowledged_at` to store for a (new) acknowledged alert.
fn acknowledged_at_for(alert: &Alert) -> Option<String> {
    (alert.status == AlertStatus::Acknowledged).then(|| Utc::now().to_rfc3339())
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

    #[tokio::test]
    async fn test_prune_resolved_with_action_history_reference() {
        let pool = create_test_db().await;
        let alert = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);
        repo.insert(&alert).await.unwrap();
        crate::db::ActionRepository::new(&pool)
            .insert(
                Some(alert.id.clone()),
                "device-1",
                "act-1",
                "restart",
                None,
                Some(0),
                None,
                Some(10),
                true,
                0,
            )
            .await
            .unwrap();

        let old = Utc::now() - chrono::Duration::days(60);
        repo.set_status(&alert.id, AlertStatus::Resolved, old)
            .await
            .unwrap();
        assert_eq!(repo.prune_resolved(30).await.unwrap(), 1);

        // The action row survives with its alert reference nulled
        let (n, alert_id): (i64, Option<String>) =
            sqlx::query_as("SELECT COUNT(*), MAX(alert_id) FROM action_history")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 1);
        assert_eq!(alert_id, None);
    }

    #[tokio::test]
    async fn test_upsert_refire_same_id() {
        let pool = create_test_db().await;
        let mut alert = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);

        repo.upsert(&alert).await.unwrap();
        repo.mark_notified(&alert.id).await.unwrap();

        // Re-fire with the same id: no PK violation, fields updated
        alert.fired_count = 3;
        alert.metric_value = Some(90.0);
        alert.message = "Temperature exceeded 80C (3x)".to_string();
        repo.upsert(&alert).await.unwrap();

        let rec = repo.get(&alert.id).await.unwrap();
        assert_eq!(rec.fired_count, 3);
        assert_eq!(rec.metric_value, Some(90.0));
        assert_eq!(rec.message, "Temperature exceeded 80C (3x)");
        // notification flag never goes back to 0
        assert_eq!(rec.notification_sent, 1);
        assert!(rec.last_fired_at.is_some());
        assert_eq!(repo.list_recent(10).await.unwrap().len(), 1);

        // Plain insert of the same id still fails (strict)
        assert!(repo.insert(&alert).await.is_err());
    }

    #[tokio::test]
    async fn test_record_refire() {
        let pool = create_test_db().await;
        let alert = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);
        repo.insert(&alert).await.unwrap();

        let at = Utc::now();
        let n = repo
            .record_refire(&alert.id, 5, at, Some(95.5), "still hot")
            .await
            .unwrap();
        assert_eq!(n, 1);
        let rec = repo.get(&alert.id).await.unwrap();
        assert_eq!(rec.fired_count, 5);
        assert_eq!(rec.metric_value, Some(95.5));
        assert_eq!(rec.message, "still hot");
        assert_eq!(rec.last_fired_at.as_deref(), Some(at.to_rfc3339().as_str()));
        assert_eq!(rec.status, "firing");

        // None keeps the stored value; count never decreases
        repo.record_refire(&alert.id, 2, at, None, "m")
            .await
            .unwrap();
        let rec = repo.get(&alert.id).await.unwrap();
        assert_eq!(rec.metric_value, Some(95.5));
        assert_eq!(rec.fired_count, 5);

        assert_eq!(
            repo.record_refire("unknown", 1, at, None, "m")
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn test_set_status_lifecycle() {
        let pool = create_test_db().await;
        let alert = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);
        repo.insert(&alert).await.unwrap();
        let at = Utc::now();

        // Acknowledge: still active
        assert_eq!(
            repo.set_status(&alert.id, AlertStatus::Acknowledged, at)
                .await
                .unwrap(),
            1
        );
        let rec = repo.get(&alert.id).await.unwrap();
        assert_eq!(rec.status(), AlertStatus::Acknowledged);
        assert_eq!(
            rec.acknowledged_at.as_deref(),
            Some(at.to_rfc3339().as_str())
        );
        assert!(rec.resolved_at.is_none());
        assert_eq!(repo.list_active().await.unwrap().len(), 1);
        assert_eq!(rec.to_alert().status, AlertStatus::Acknowledged);

        // A re-fire upsert does not revert the acknowledgement
        repo.upsert(&alert).await.unwrap();
        assert_eq!(
            repo.get(&alert.id).await.unwrap().status(),
            AlertStatus::Acknowledged
        );

        // Resolve
        repo.set_status(&alert.id, AlertStatus::Resolved, at)
            .await
            .unwrap();
        let rec = repo.get(&alert.id).await.unwrap();
        assert_eq!(rec.status(), AlertStatus::Resolved);
        assert_eq!(rec.resolved_at.as_deref(), Some(at.to_rfc3339().as_str()));
        assert!(repo.list_active().await.unwrap().is_empty());

        // Acknowledging a resolved alert is refused
        assert_eq!(
            repo.set_status(&alert.id, AlertStatus::Acknowledged, at)
                .await
                .unwrap(),
            0
        );

        // Reopen clears resolved_at
        repo.set_status(&alert.id, AlertStatus::Firing, at)
            .await
            .unwrap();
        let rec = repo.get(&alert.id).await.unwrap();
        assert_eq!(rec.status(), AlertStatus::Firing);
        assert!(rec.resolved_at.is_none());

        assert_eq!(
            repo.set_status("nope", AlertStatus::Resolved, at)
                .await
                .unwrap(),
            0
        );
        assert!(matches!(
            repo.get("nope").await.unwrap_err(),
            crate::NimonError::AlertNotFound(_)
        ));
    }

    #[tokio::test]
    async fn test_list_history_filters_before_limit() {
        let pool = create_test_db().await;
        let base = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);
        let t0 = Utc::now() - chrono::Duration::hours(10);

        // 10 info alerts (newest), then 2 critical alerts (older)
        for i in 0..10 {
            repo.insert(&Alert {
                id: ulid::Ulid::new().to_string(),
                severity: Severity::Info,
                triggered_at: t0 + chrono::Duration::hours(5) + chrono::Duration::minutes(i),
                ..base.clone()
            })
            .await
            .unwrap();
        }
        for i in 0..2 {
            repo.insert(&Alert {
                id: ulid::Ulid::new().to_string(),
                severity: Severity::Critical,
                edge_id: "edge-2".to_string(),
                device_id: String::new(),
                triggered_at: t0 + chrono::Duration::minutes(i),
                ..base.clone()
            })
            .await
            .unwrap();
        }

        // Severity filter applied before LIMIT: both criticals are found
        let crit = repo
            .list_history(&AlertHistoryFilter {
                severity: Some(Severity::Critical),
                limit: Some(5),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(crit.len(), 2);
        assert!(crit[0].triggered_at > crit[1].triggered_at, "newest first");

        let by_edge = repo
            .list_history(&AlertHistoryFilter {
                edge_id: Some("edge-2".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_edge.len(), 2);

        let by_device = repo
            .list_history(&AlertHistoryFilter {
                device_id: Some("device-1".to_string()),
                limit: Some(3),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_device.len(), 3);

        // Time window (inclusive)
        let window = repo
            .list_history(&AlertHistoryFilter {
                since: Some(t0 + chrono::Duration::hours(5)),
                until: Some(t0 + chrono::Duration::hours(5) + chrono::Duration::minutes(4)),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(window.len(), 5);

        // Status filter
        let first = &by_device[0].id;
        repo.resolve(first).await.unwrap();
        let resolved = repo
            .list_history(&AlertHistoryFilter {
                status: Some(AlertStatus::Resolved),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(&resolved[0].id, first);

        // Combined filters + no limit
        let all = repo
            .list_history(&AlertHistoryFilter::default())
            .await
            .unwrap();
        assert_eq!(all.len(), 12);
        let none = repo
            .list_history(&AlertHistoryFilter {
                severity: Some(Severity::Critical),
                device_id: Some("device-1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn test_insert_writes_rfc3339_created_at() {
        let pool = create_test_db().await;
        let alert = setup_test_data(&pool).await;
        let repo = AlertRepository::new(&pool);
        repo.insert(&alert).await.unwrap();
        let rec = repo.get(&alert.id).await.unwrap();
        assert!(DateTime::parse_from_rfc3339(&rec.created_at).is_ok());
        assert!(DateTime::parse_from_rfc3339(rec.triggered_at.as_deref().unwrap()).is_ok());
    }
}
