//! Repository for device operations

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::{Device, DeviceStatus, DeviceType, HealthStatus, MetricPoint, NimonError, NimonResult};

/// Row shape of the `devices` columns selected by this repository.
#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: String,
    edge_id: String,
    device_name: String,
    device_type: String,
    model: Option<String>,
    serial_number: Option<String>,
    firmware_version: Option<String>,
    driver_version: Option<String>,
    ip_address: Option<String>,
    slot: Option<i32>,
    chassis: Option<String>,
    is_simulated: Option<i64>,
}

impl From<DeviceRow> for Device {
    fn from(row: DeviceRow) -> Self {
        Device {
            id: row.id,
            edge_id: row.edge_id,
            device_name: row.device_name,
            device_type: DeviceType::parse(&row.device_type),
            model: row.model,
            serial_number: row.serial_number,
            firmware_version: row.firmware_version,
            driver_version: row.driver_version,
            ip_address: row.ip_address,
            slot: row.slot,
            chassis: row.chassis,
            is_simulated: row.is_simulated.unwrap_or(0) != 0,
        }
    }
}

#[derive(sqlx::FromRow)]
struct StatusRow {
    status: String,
    last_poll: Option<String>,
    metrics: Option<String>,
    error_message: Option<String>,
    error_count: Option<i32>,
    uptime_seconds: Option<i64>,
}

const DEVICE_COLUMNS: &str = "id, edge_id, device_name, device_type, model, serial_number, \
     firmware_version, driver_version, ip_address, slot, chassis, is_simulated";

/// SQL for a `device_name` that is unique on the edge: `?3` (preferred)
/// unless another device on edge `?2` already uses it, then `?N`
/// (fallback), then the globally unique device id `?1`.
fn unique_name_sql(fallback_param: usize) -> String {
    format!(
        "CASE \
            WHEN NOT EXISTS (SELECT 1 FROM devices WHERE edge_id = ?2 AND device_name = ?3 AND id <> ?1) THEN ?3 \
            WHEN NOT EXISTS (SELECT 1 FROM devices WHERE edge_id = ?2 AND device_name = ?{fallback_param} AND id <> ?1) THEN ?{fallback_param} \
            ELSE ?1 \
         END"
    )
}

/// The edge-local part of a composite device id (`edge:PXIe-6368#2` ->
/// `PXIe-6368#2`), or the whole id when it has no `edge:` prefix. Splits at
/// the FIRST colon: VISA resources (`edge:TCPIP0::10.0.0.5::INSTR`) contain
/// `::` themselves.
fn local_part(device_id: &str) -> &str {
    match device_id.split_once(':') {
        Some((_, rest)) if !rest.is_empty() => rest,
        _ => device_id,
    }
}

/// Descriptive device fields learned after first sighting (discovery,
/// SysCfg, ...). `None` means "unknown, keep what is stored".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceMetadata {
    pub device_name: Option<String>,
    pub device_type: Option<DeviceType>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub firmware_version: Option<String>,
    pub driver_version: Option<String>,
    pub ip_address: Option<String>,
    pub slot: Option<i32>,
    pub chassis: Option<String>,
    pub is_simulated: Option<bool>,
}

pub struct DeviceRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> DeviceRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert or fully update a device. If another device on the same edge
    /// already uses `device.device_name` (e.g. two identical products),
    /// the stored name falls back to the id's local part, then to the id,
    /// so the `UNIQUE(edge_id, device_name)` constraint never rejects it.
    pub async fn upsert(&self, device: &Device) -> NimonResult<()> {
        let sql = format!(
            r#"
            INSERT INTO devices (id, edge_id, device_name, device_type, model, serial_number,
                                 firmware_version, driver_version, ip_address, slot, chassis, is_simulated)
            VALUES (?1, ?2, {name}, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(id) DO UPDATE SET
                edge_id = excluded.edge_id,
                device_name = excluded.device_name,
                device_type = excluded.device_type,
                model = excluded.model,
                serial_number = excluded.serial_number,
                firmware_version = excluded.firmware_version,
                driver_version = excluded.driver_version,
                ip_address = excluded.ip_address,
                slot = excluded.slot,
                chassis = excluded.chassis,
                is_simulated = excluded.is_simulated
            "#,
            name = unique_name_sql(13)
        );
        sqlx::query(&sql)
            .bind(&device.id)
            .bind(&device.edge_id)
            .bind(&device.device_name)
            .bind(device.device_type.to_string())
            .bind(&device.model)
            .bind(&device.serial_number)
            .bind(&device.firmware_version)
            .bind(&device.driver_version)
            .bind(&device.ip_address)
            .bind(device.slot)
            .bind(&device.chassis)
            .bind(device.is_simulated as i64)
            .bind(local_part(&device.id))
            .execute(self.pool)
            .await?;

        Ok(())
    }

    pub async fn get(&self, id: &str) -> NimonResult<Device> {
        let row: DeviceRow = sqlx::query_as(&format!(
            "SELECT {DEVICE_COLUMNS} FROM devices WHERE id = ?"
        ))
        .bind(id)
        .fetch_one(self.pool)
        .await
        .map_err(|e| map_not_found(e, NimonError::DeviceNotFound(id.to_string())))?;
        Ok(row.into())
    }

    pub async fn list_by_edge(&self, edge_id: &str) -> NimonResult<Vec<Device>> {
        let rows: Vec<DeviceRow> = sqlx::query_as(&format!(
            "SELECT {DEVICE_COLUMNS} FROM devices WHERE edge_id = ? ORDER BY device_name"
        ))
        .bind(edge_id)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(Device::from).collect())
    }

    /// Best-effort insert of a device first seen in a status update, so
    /// `alerts`/`device_status` FK references resolve. Existing rows
    /// (from real discovery) are left untouched. The name is the id's
    /// local part INCLUDING any `#n` suffix (`edge:PXIe-6368#2` ->
    /// `PXIe-6368#2`) so identical products do not collide; if the name
    /// is still taken on the edge, the full id is used.
    pub async fn upsert_snapshot(&self, device_id: &str, edge_id: &str) -> NimonResult<()> {
        let device_name = local_part(device_id);
        let device_type = classify_device_name(device_name);
        sqlx::query(
            "INSERT INTO devices (id, edge_id, device_name, device_type)
             VALUES (?1, ?2,
                     CASE WHEN NOT EXISTS (SELECT 1 FROM devices
                                           WHERE edge_id = ?2 AND device_name = ?3 AND id <> ?1)
                          THEN ?3 ELSE ?1 END,
                     ?4)
             ON CONFLICT DO NOTHING",
        )
        .bind(device_id)
        .bind(edge_id)
        .bind(device_name)
        .bind(device_type)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Create the device row if missing (like [`Self::upsert_snapshot`])
    /// and store every metadata field that is `Some`, keeping stored
    /// values for fields that are `None`.
    pub async fn upsert_metadata(
        &self,
        device_id: &str,
        edge_id: &str,
        meta: &DeviceMetadata,
    ) -> NimonResult<()> {
        let preferred = meta
            .device_name
            .clone()
            .unwrap_or_else(|| local_part(device_id).to_string());
        let insert_type = meta
            .device_type
            .map(|t| t.to_string())
            .unwrap_or_else(|| classify_device_name(&preferred).to_string());
        let sql = format!(
            r#"
            INSERT INTO devices (id, edge_id, device_name, device_type, model, serial_number,
                                 firmware_version, driver_version, ip_address, slot, chassis, is_simulated)
            VALUES (?1, ?2, {name}, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, COALESCE(?12, 0))
            ON CONFLICT(id) DO UPDATE SET
                device_name = CASE WHEN ?14 IS NULL THEN devices.device_name ELSE excluded.device_name END,
                device_type = COALESCE(?15, devices.device_type),
                model = COALESCE(excluded.model, devices.model),
                serial_number = COALESCE(excluded.serial_number, devices.serial_number),
                firmware_version = COALESCE(excluded.firmware_version, devices.firmware_version),
                driver_version = COALESCE(excluded.driver_version, devices.driver_version),
                ip_address = COALESCE(excluded.ip_address, devices.ip_address),
                slot = COALESCE(excluded.slot, devices.slot),
                chassis = COALESCE(excluded.chassis, devices.chassis),
                is_simulated = COALESCE(?12, devices.is_simulated)
            "#,
            name = unique_name_sql(13)
        );
        sqlx::query(&sql)
            .bind(device_id) // ?1
            .bind(edge_id) // ?2
            .bind(&preferred) // ?3
            .bind(&insert_type) // ?4
            .bind(&meta.model) // ?5
            .bind(&meta.serial_number) // ?6
            .bind(&meta.firmware_version) // ?7
            .bind(&meta.driver_version) // ?8
            .bind(&meta.ip_address) // ?9
            .bind(meta.slot) // ?10
            .bind(&meta.chassis) // ?11
            .bind(meta.is_simulated.map(|b| b as i64)) // ?12
            .bind(local_part(device_id)) // ?13
            .bind(&meta.device_name) // ?14
            .bind(meta.device_type.map(|t| t.to_string())) // ?15
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn upsert_status(&self, status: &DeviceStatus) -> NimonResult<()> {
        let metrics_json = serde_json::to_string(&status.metrics)?;
        sqlx::query(
            r#"
            INSERT INTO device_status (device_id, status, last_poll, metrics, error_message, error_count, uptime_seconds)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(device_id) DO UPDATE SET
                status = excluded.status,
                last_poll = excluded.last_poll,
                metrics = excluded.metrics,
                error_message = excluded.error_message,
                error_count = excluded.error_count,
                uptime_seconds = excluded.uptime_seconds,
                updated_at = CURRENT_TIMESTAMP
            "#
        )
        .bind(&status.device_id)
        .bind(status.status.to_string())
        .bind(status.last_poll.to_rfc3339())
        .bind(metrics_json)
        .bind(&status.error_message)
        .bind(status.error_count)
        .bind(status.uptime_seconds)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    /// Current status of a device. Stored metric entries that cannot be
    /// parsed (e.g. `null` from a NaN) are skipped individually.
    pub async fn get_status(&self, device_id: &str) -> NimonResult<DeviceStatus> {
        let row: StatusRow = sqlx::query_as(
            "SELECT status, last_poll, metrics, error_message, error_count, uptime_seconds
                 FROM device_status WHERE device_id = ?",
        )
        .bind(device_id)
        .fetch_one(self.pool)
        .await
        .map_err(|e| map_not_found(e, NimonError::DeviceNotFound(device_id.to_string())))?;

        let last_poll = row.last_poll.as_deref().and_then(parse_ts).ok_or_else(|| {
            NimonError::InvalidState(format!(
                "unparsable last_poll stored for device {}",
                device_id
            ))
        })?;

        let metrics = row
            .metrics
            .as_deref()
            .map(crate::types::parse_metrics_lenient)
            .unwrap_or_default();

        Ok(DeviceStatus {
            device_id: device_id.to_string(),
            status: parse_health_status(&row.status),
            last_poll,
            metrics,
            error_message: row.error_message,
            error_count: row.error_count.unwrap_or(0),
            uptime_seconds: row.uptime_seconds.unwrap_or(0),
        })
    }

    /// Insert one metric point. Non-finite values (NaN/Inf) cannot be
    /// stored in the `REAL NOT NULL` column and are silently skipped.
    pub async fn insert_metric(&self, metric: &MetricPoint) -> NimonResult<()> {
        if !metric.metric_value.is_finite() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO device_metrics_history (device_id, timestamp, metric_name, metric_value) VALUES (?, ?, ?, ?)"
        )
        .bind(&metric.device_id)
        .bind(metric.timestamp.to_rfc3339())
        .bind(&metric.metric_name)
        .bind(metric.metric_value)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    /// Insert many `(device_id, metric_name, value, timestamp)` rows in ONE
    /// transaction (all or nothing). Non-finite values are skipped.
    /// Returns the number of rows written.
    pub async fn insert_metrics_batch<D, M>(
        &self,
        rows: &[(D, M, f64, DateTime<Utc>)],
    ) -> NimonResult<u64>
    where
        D: AsRef<str>,
        M: AsRef<str>,
    {
        if rows.iter().all(|r| !r.2.is_finite()) {
            return Ok(0);
        }
        let mut tx = self.pool.begin().await?;
        let mut written = 0u64;
        for (device_id, metric_name, value, timestamp) in rows {
            if !value.is_finite() {
                continue;
            }
            sqlx::query(
                "INSERT INTO device_metrics_history (device_id, timestamp, metric_name, metric_value) VALUES (?, ?, ?, ?)",
            )
            .bind(device_id.as_ref())
            .bind(timestamp.to_rfc3339())
            .bind(metric_name.as_ref())
            .bind(*value)
            .execute(&mut *tx)
            .await?;
            written += 1;
        }
        tx.commit().await?;
        Ok(written)
    }

    pub async fn get_metrics(
        &self,
        device_id: &str,
        metric_name: &str,
        limit: i64,
    ) -> NimonResult<Vec<MetricPoint>> {
        let rows: Vec<(String, f64)> = sqlx::query_as(
            "SELECT timestamp, metric_value FROM device_metrics_history
             WHERE device_id = ? AND metric_name = ?
             ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(device_id)
        .bind(metric_name)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                parse_ts(&row.0).map(|timestamp| MetricPoint {
                    device_id: device_id.to_string(),
                    timestamp,
                    metric_name: metric_name.to_string(),
                    metric_value: row.1,
                })
            })
            .collect())
    }

    /// Prune metric history rows older than the given number of days.
    pub async fn prune_metrics(&self, older_than_days: i64) -> NimonResult<u64> {
        let cutoff = (Utc::now() - chrono::Duration::days(older_than_days)).to_rfc3339();
        let result = sqlx::query("DELETE FROM device_metrics_history WHERE timestamp < ?")
            .bind(&cutoff)
            .execute(self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

/// Map a sqlx error to NotFound only when it actually is a missing row;
/// everything else stays a database error instead of being swallowed.
fn map_not_found(e: sqlx::Error, not_found: NimonError) -> NimonError {
    match e {
        sqlx::Error::RowNotFound => not_found,
        other => NimonError::Database(other),
    }
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Classify a device from its name (best effort; defaults to Daq).
fn classify_device_name(name: &str) -> &'static str {
    let upper = name.to_uppercase();
    if upper.contains("CDAQ") || upper.contains("COMPACTDAQ") {
        "cdaq"
    } else if upper.contains("PXI") {
        "pxi"
    } else if upper.contains("GPIB") {
        "gpib"
    } else if upper.contains("XNET") {
        "xnet"
    } else if upper.contains("VISA") || upper.contains("::") {
        "visa"
    } else {
        "daq"
    }
}

fn parse_health_status(s: &str) -> HealthStatus {
    HealthStatus::parse(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::create_test_db;
    use crate::db::edge_repo::EdgeRepository;

    async fn setup(pool: &SqlitePool) {
        let edge_repo = EdgeRepository::new(pool);
        edge_repo
            .upsert(&crate::EdgeNode {
                id: "edge-1".to_string(),
                name: "Edge 1".to_string(),
                hostname: None,
                ip_address: None,
                last_seen: None,
                status: crate::EdgeStatus::Online,
            })
            .await
            .unwrap();
    }

    fn sample_device() -> Device {
        Device {
            id: "device-1".to_string(),
            edge_id: "edge-1".to_string(),
            device_name: "PXI1Slot2".to_string(),
            device_type: DeviceType::Pxi,
            model: Some("PXIe-6368".to_string()),
            serial_number: Some("12345678".to_string()),
            firmware_version: None,
            driver_version: None,
            ip_address: None,
            slot: Some(2),
            chassis: Some("PXI1".to_string()),
            is_simulated: true,
        }
    }

    #[tokio::test]
    async fn test_upsert_and_get_device() {
        let pool = create_test_db().await;
        setup(&pool).await;

        let device_repo = DeviceRepository::new(&pool);
        let device = sample_device();

        device_repo.upsert(&device).await.unwrap();
        let fetched = device_repo.get("device-1").await.unwrap();

        assert_eq!(fetched.id, device.id);
        assert_eq!(fetched.device_name, device.device_name);
        assert_eq!(fetched.device_type, DeviceType::Pxi);
        assert!(fetched.is_simulated);
    }

    #[tokio::test]
    async fn test_get_missing_device_is_not_found() {
        let pool = create_test_db().await;
        let device_repo = DeviceRepository::new(&pool);
        let err = device_repo.get("nope").await.unwrap_err();
        assert!(matches!(err, NimonError::DeviceNotFound(_)));
    }

    #[tokio::test]
    async fn test_status_roundtrip_with_metrics() {
        let pool = create_test_db().await;
        setup(&pool).await;
        let device_repo = DeviceRepository::new(&pool);
        device_repo.upsert(&sample_device()).await.unwrap();

        let mut metrics = std::collections::HashMap::new();
        metrics.insert("temperature".to_string(), crate::MetricValue::Float(45.5));
        metrics.insert("error_count".to_string(), crate::MetricValue::Integer(3));

        let status = DeviceStatus {
            device_id: "device-1".to_string(),
            status: crate::HealthStatus::Warning,
            last_poll: Utc::now(),
            metrics,
            error_message: None,
            error_count: 0,
            uptime_seconds: 0,
        };
        device_repo.upsert_status(&status).await.unwrap();

        let fetched = device_repo.get_status("device-1").await.unwrap();
        assert_eq!(fetched.status, crate::HealthStatus::Warning);
        // Metrics JSON is now actually deserialized
        assert_eq!(
            fetched.metrics.get("temperature"),
            Some(&crate::MetricValue::Float(45.5))
        );
        assert_eq!(
            fetched.metrics.get("error_count"),
            // untagged MetricValue: numeric JSON round-trips as Float
            Some(&crate::MetricValue::Float(3.0))
        );
    }

    #[tokio::test]
    async fn test_insert_and_get_metrics() {
        let pool = create_test_db().await;
        setup(&pool).await;
        let device_repo = DeviceRepository::new(&pool);
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

        let metric = MetricPoint {
            device_id: "device-1".to_string(),
            timestamp: Utc::now(),
            metric_name: "temperature".to_string(),
            metric_value: 45.5,
        };

        device_repo.insert_metric(&metric).await.unwrap();
        let metrics = device_repo
            .get_metrics("device-1", "temperature", 10)
            .await
            .unwrap();

        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].metric_value, 45.5);
    }

    #[tokio::test]
    async fn test_prune_metrics() {
        let pool = create_test_db().await;
        setup(&pool).await;
        let device_repo = DeviceRepository::new(&pool);
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

        let old = MetricPoint {
            device_id: "device-1".to_string(),
            timestamp: Utc::now() - chrono::Duration::days(90),
            metric_name: "temperature".to_string(),
            metric_value: 40.0,
        };
        let recent = MetricPoint {
            device_id: "device-1".to_string(),
            timestamp: Utc::now(),
            metric_name: "temperature".to_string(),
            metric_value: 41.0,
        };
        device_repo.insert_metric(&old).await.unwrap();
        device_repo.insert_metric(&recent).await.unwrap();

        assert_eq!(device_repo.prune_metrics(30).await.unwrap(), 1);
        let remaining = device_repo
            .get_metrics("device-1", "temperature", 10)
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].metric_value, 41.0);
    }

    #[tokio::test]
    async fn test_snapshot_identical_products_both_get_rows() {
        let pool = create_test_db().await;
        setup(&pool).await;
        let repo = DeviceRepository::new(&pool);

        repo.upsert_snapshot("edge-1:PXIe-6368#1", "edge-1")
            .await
            .unwrap();
        repo.upsert_snapshot("edge-1:PXIe-6368#2", "edge-1")
            .await
            .unwrap();
        // Idempotent
        repo.upsert_snapshot("edge-1:PXIe-6368#2", "edge-1")
            .await
            .unwrap();

        let devices = repo.list_by_edge("edge-1").await.unwrap();
        assert_eq!(devices.len(), 2);
        let d1 = repo.get("edge-1:PXIe-6368#1").await.unwrap();
        let d2 = repo.get("edge-1:PXIe-6368#2").await.unwrap();
        assert_eq!(d1.device_name, "PXIe-6368#1");
        assert_eq!(d2.device_name, "PXIe-6368#2");
        assert_eq!(d2.device_type, DeviceType::Pxi);

        // FK-dependent inserts now succeed for both
        for id in ["edge-1:PXIe-6368#1", "edge-1:PXIe-6368#2"] {
            repo.upsert_status(&DeviceStatus {
                device_id: id.to_string(),
                status: HealthStatus::Healthy,
                last_poll: Utc::now(),
                metrics: Default::default(),
                error_message: None,
                error_count: 0,
                uptime_seconds: 0,
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn test_upsert_disambiguates_duplicate_names() {
        let pool = create_test_db().await;
        setup(&pool).await;
        let repo = DeviceRepository::new(&pool);

        // Two discovered devices with the same product name
        let mut a = sample_device();
        a.id = "edge-1:PXIe-6368#1".to_string();
        a.device_name = "PXIe-6368".to_string();
        let mut b = a.clone();
        b.id = "edge-1:PXIe-6368#2".to_string();
        repo.upsert(&a).await.unwrap();
        repo.upsert(&b).await.unwrap();
        // Re-upserting keeps both working
        repo.upsert(&a).await.unwrap();
        repo.upsert(&b).await.unwrap();

        assert_eq!(repo.get(&a.id).await.unwrap().device_name, "PXIe-6368");
        assert_eq!(repo.get(&b.id).await.unwrap().device_name, "PXIe-6368#2");

        // Snapshot row first, discovery second (same id): upsert updates it
        repo.upsert_snapshot("edge-1:Dev3", "edge-1").await.unwrap();
        let mut c = sample_device();
        c.id = "edge-1:Dev3".to_string();
        c.device_name = "Dev3".to_string();
        c.model = Some("USB-6009".to_string());
        repo.upsert(&c).await.unwrap();
        assert_eq!(
            repo.get("edge-1:Dev3").await.unwrap().model.as_deref(),
            Some("USB-6009")
        );
    }

    #[tokio::test]
    async fn test_upsert_metadata_merges_known_fields() {
        let pool = create_test_db().await;
        setup(&pool).await;
        let repo = DeviceRepository::new(&pool);

        // Creates the row when missing
        repo.upsert_metadata(
            "edge-1:PXI1Slot2",
            "edge-1",
            &DeviceMetadata {
                model: Some("PXIe-6368".to_string()),
                serial_number: Some("0x1A2B".to_string()),
                slot: Some(2),
                is_simulated: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let d = repo.get("edge-1:PXI1Slot2").await.unwrap();
        assert_eq!(d.device_name, "PXI1Slot2");
        assert_eq!(d.device_type, DeviceType::Pxi);
        assert_eq!(d.model.as_deref(), Some("PXIe-6368"));
        assert_eq!(d.slot, Some(2));
        assert!(d.is_simulated);

        // Partial update: only firmware known; the rest is kept
        repo.upsert_metadata(
            "edge-1:PXI1Slot2",
            "edge-1",
            &DeviceMetadata {
                firmware_version: Some("1.2.3".to_string()),
                device_type: Some(DeviceType::PowerSupply),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let d = repo.get("edge-1:PXI1Slot2").await.unwrap();
        assert_eq!(d.firmware_version.as_deref(), Some("1.2.3"));
        assert_eq!(d.model.as_deref(), Some("PXIe-6368"));
        assert_eq!(d.serial_number.as_deref(), Some("0x1A2B"));
        assert_eq!(d.device_type, DeviceType::PowerSupply);
        assert!(d.is_simulated);
        assert_eq!(d.device_name, "PXI1Slot2");

        // Metadata on an existing snapshot row
        repo.upsert_snapshot("edge-1:Dev1", "edge-1").await.unwrap();
        repo.upsert_metadata(
            "edge-1:Dev1",
            "edge-1",
            &DeviceMetadata {
                is_simulated: Some(false),
                chassis: Some("PXI1".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let d = repo.get("edge-1:Dev1").await.unwrap();
        assert_eq!(d.chassis.as_deref(), Some("PXI1"));
        assert!(!d.is_simulated);
    }

    #[tokio::test]
    async fn test_insert_metrics_batch() {
        let pool = create_test_db().await;
        setup(&pool).await;
        let repo = DeviceRepository::new(&pool);
        repo.upsert(&sample_device()).await.unwrap();

        let now = Utc::now();
        let rows = vec![
            ("device-1".to_string(), "temperature".to_string(), 40.0, now),
            (
                "device-1".to_string(),
                "temperature".to_string(),
                f64::NAN,
                now,
            ),
            ("device-1".to_string(), "voltage".to_string(), 5.0, now),
        ];
        assert_eq!(repo.insert_metrics_batch(&rows).await.unwrap(), 2);
        assert_eq!(
            repo.get_metrics("device-1", "temperature", 10)
                .await
                .unwrap()
                .len(),
            1
        );
        // Borrowed rows work too; empty batch is a no-op
        let borrowed = [("device-1", "voltage", 5.5, now)];
        assert_eq!(repo.insert_metrics_batch(&borrowed).await.unwrap(), 1);
        let empty: [(&str, &str, f64, DateTime<Utc>); 0] = [];
        assert_eq!(repo.insert_metrics_batch(&empty).await.unwrap(), 0);

        // One failing row (FK: unknown device) rolls back the whole batch
        let bad = [
            ("device-1", "current", 1.0, now),
            ("ghost", "current", 1.0, now),
        ];
        assert!(repo.insert_metrics_batch(&bad).await.is_err());
        assert!(repo
            .get_metrics("device-1", "current", 10)
            .await
            .unwrap()
            .is_empty());

        // Single-row insert skips NaN instead of failing NOT NULL
        repo.insert_metric(&MetricPoint {
            device_id: "device-1".to_string(),
            timestamp: now,
            metric_name: "voltage".to_string(),
            metric_value: f64::INFINITY,
        })
        .await
        .unwrap();
        assert_eq!(
            repo.get_metrics("device-1", "voltage", 10)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn test_get_status_skips_unparseable_metrics() {
        let pool = create_test_db().await;
        setup(&pool).await;
        let repo = DeviceRepository::new(&pool);
        repo.upsert(&sample_device()).await.unwrap();

        let mut metrics = std::collections::HashMap::new();
        metrics.insert(
            "temperature".to_string(),
            crate::MetricValue::Float(f64::NAN),
        );
        metrics.insert("voltage".to_string(), crate::MetricValue::Float(5.0));
        repo.upsert_status(&DeviceStatus {
            device_id: "device-1".to_string(),
            status: HealthStatus::Healthy,
            last_poll: Utc::now(),
            metrics,
            error_message: None,
            error_count: 0,
            uptime_seconds: 0,
        })
        .await
        .unwrap();

        let fetched = repo.get_status("device-1").await.unwrap();
        assert_eq!(fetched.metrics.len(), 1);
        assert_eq!(
            fetched.metrics.get("voltage"),
            Some(&crate::MetricValue::Float(5.0))
        );
    }

    #[test]
    fn test_local_part_keeps_visa_resource_intact() {
        assert_eq!(
            local_part("edge-01:TCPIP0::10.0.0.5::inst0::INSTR"),
            "TCPIP0::10.0.0.5::inst0::INSTR"
        );
        assert_eq!(local_part("edge-01:PXIe-6368#2"), "PXIe-6368#2");
        assert_eq!(local_part("standalone"), "standalone");
        assert_eq!(classify_device_name("TCPIP0::10.0.0.5::inst0::INSTR"), "visa");
        assert_eq!(classify_device_name("ASRL3::INSTR"), "visa");
        assert_eq!(classify_device_name("GPIB0::5::INSTR"), "gpib");
        assert_eq!(classify_device_name("PXIe-6368#2"), "pxi");
    }
}
