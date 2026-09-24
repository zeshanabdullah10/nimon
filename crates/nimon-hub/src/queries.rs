//! Hub-side read queries that the core repositories do not provide
//! (single-query joins used by the REST API and startup restore).

use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use std::collections::HashMap;

/// A device row joined with its last status (one query, no N+1).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeviceStatusRow {
    pub id: String,
    pub edge_id: String,
    pub device_name: String,
    pub device_type: String,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub slot: Option<i64>,
    pub is_simulated: Option<i64>,
    pub status: Option<String>,
    pub last_poll: Option<String>,
    pub metrics: Option<String>,
}

impl DeviceStatusRow {
    pub fn last_poll(&self) -> Option<DateTime<Utc>> {
        self.last_poll.as_deref().and_then(parse_ts)
    }

    pub fn metrics(&self) -> HashMap<String, nimon_core::MetricValue> {
        self.metrics
            .as_deref()
            .map(nimon_core::types::parse_metrics_lenient)
            .unwrap_or_default()
    }
}

/// Parse an RFC3339 timestamp.
pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Every device (optionally of one edge) with its last status.
pub async fn list_devices(
    pool: &SqlitePool,
    edge_id: Option<&str>,
) -> Result<Vec<DeviceStatusRow>, sqlx::Error> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT d.id, d.edge_id, d.device_name, d.device_type, d.model, d.serial_number, \
         d.slot, d.is_simulated, s.status, s.last_poll, s.metrics \
         FROM devices d LEFT JOIN device_status s ON s.device_id = d.id WHERE 1 = 1",
    );
    if let Some(edge_id) = edge_id {
        qb.push(" AND d.edge_id = ").push_bind(edge_id.to_string());
    }
    qb.push(" ORDER BY d.edge_id, d.id");
    qb.build_query_as::<DeviceStatusRow>().fetch_all(pool).await
}

/// One device with its last status.
pub async fn get_device(
    pool: &SqlitePool,
    device_id: &str,
) -> Result<Option<DeviceStatusRow>, sqlx::Error> {
    sqlx::query_as::<_, DeviceStatusRow>(
        "SELECT d.id, d.edge_id, d.device_name, d.device_type, d.model, d.serial_number, \
         d.slot, d.is_simulated, s.status, s.last_poll, s.metrics \
         FROM devices d LEFT JOIN device_status s ON s.device_id = d.id WHERE d.id = ?",
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await
}

/// Number of known devices per edge.
pub async fn device_counts_by_edge(pool: &SqlitePool) -> Result<HashMap<String, i64>, sqlx::Error> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT edge_id, COUNT(*) FROM devices GROUP BY edge_id")
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Metric history points `(timestamp, value)`, newest `limit` points in
/// the window, returned oldest first.
pub async fn metric_points(
    pool: &SqlitePool,
    device_id: &str,
    metric: &str,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    limit: i64,
) -> Result<Vec<(String, f64)>, sqlx::Error> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT timestamp, metric_value FROM device_metrics_history WHERE device_id = ",
    );
    qb.push_bind(device_id.to_string());
    qb.push(" AND metric_name = ").push_bind(metric.to_string());
    if let Some(since) = since {
        qb.push(" AND timestamp >= ").push_bind(since.to_rfc3339());
    }
    if let Some(until) = until {
        qb.push(" AND timestamp <= ").push_bind(until.to_rfc3339());
    }
    qb.push(" ORDER BY timestamp DESC LIMIT ").push_bind(limit);
    let mut rows: Vec<(String, f64)> = qb.build_query_as().fetch_all(pool).await?;
    rows.reverse();
    Ok(rows)
}
