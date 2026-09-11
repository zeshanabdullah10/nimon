//! Repository for device operations

use chrono::{DateTime, Utc};
use serde_json;
use sqlx::SqlitePool;

use crate::{Device, DeviceStatus, DeviceType, HealthStatus, MetricPoint, NimonError, NimonResult};

pub struct DeviceRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> DeviceRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert(&self, device: &Device) -> NimonResult<()> {
        sqlx::query(
            r#"
            INSERT INTO devices (id, edge_id, device_name, device_type, model, serial_number,
                                 firmware_version, driver_version, ip_address, slot, chassis, is_simulated)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            "#
        )
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
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn get(&self, id: &str) -> NimonResult<Device> {
        let row: (
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<i64>,
        ) = sqlx::query_as(
            "SELECT id, edge_id, device_name, device_type, model, serial_number,
                    firmware_version, driver_version, ip_address, slot, chassis, is_simulated
             FROM devices WHERE id = ?",
        )
        .bind(id)
        .fetch_one(self.pool)
        .await
        .map_err(|e| map_not_found(e, NimonError::DeviceNotFound(id.to_string())))?;

        Ok(Device {
            id: row.0,
            edge_id: row.1,
            device_name: row.2,
            device_type: parse_device_type(&row.3),
            model: row.4,
            serial_number: row.5,
            firmware_version: row.6,
            driver_version: row.7,
            ip_address: row.8,
            slot: row.9,
            chassis: row.10,
            is_simulated: row.11.unwrap_or(0) != 0,
        })
    }

    pub async fn list_by_edge(&self, edge_id: &str) -> NimonResult<Vec<Device>> {
        let rows: Vec<(
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<i64>,
        )> = sqlx::query_as(
            "SELECT id, edge_id, device_name, device_type, model, serial_number,
                    firmware_version, driver_version, ip_address, slot, chassis, is_simulated
             FROM devices WHERE edge_id = ? ORDER BY device_name",
        )
        .bind(edge_id)
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Device {
                id: row.0,
                edge_id: row.1,
                device_name: row.2,
                device_type: parse_device_type(&row.3),
                model: row.4,
                serial_number: row.5,
                firmware_version: row.6,
                driver_version: row.7,
                ip_address: row.8,
                slot: row.9,
                chassis: row.10,
                is_simulated: row.11.unwrap_or(0) != 0,
            })
            .collect())
    }

    /// Best-effort upsert of a device first seen in a status update, so
    /// `alerts`/`device_status` FK references resolve. Existing rows
    /// (from real discovery) are left untouched; the name/type are
    /// derived from the composite id.
    pub async fn upsert_snapshot(
        &self,
        device_id: &str,
        edge_id: &str,
    ) -> NimonResult<()> {
        let local = match device_id.rsplit_once(':') {
            Some((_, rest)) if !rest.is_empty() => rest,
            _ => device_id,
        };
        let device_name = match local.rsplit_once('#') {
            Some((name, _)) => name,
            None => local,
        };
        let device_type = classify_device_name(device_name);
        sqlx::query(
            "INSERT INTO devices (id, edge_id, device_name, device_type) VALUES (?, ?, ?, ?)
             ON CONFLICT(id) DO NOTHING",
        )
        .bind(device_id)
        .bind(edge_id)
        .bind(device_name)
        .bind(device_type)
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

    pub async fn get_status(&self, device_id: &str) -> NimonResult<DeviceStatus> {
        let row: (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<i64>,
        ) = sqlx::query_as(
            "SELECT status, last_poll, metrics, error_message, error_count, uptime_seconds
                 FROM device_status WHERE device_id = ?",
        )
        .bind(device_id)
        .fetch_one(self.pool)
        .await
        .map_err(|e| map_not_found(e, NimonError::DeviceNotFound(device_id.to_string())))?;

        let last_poll = row.1.as_deref().and_then(parse_ts).ok_or_else(|| {
            NimonError::InvalidState(format!(
                "unparsable last_poll stored for device {}",
                device_id
            ))
        })?;

        // Deserialize the metrics JSON that upsert_status stores
        let metrics: Option<std::collections::HashMap<String, crate::MetricValue>> = row
            .2
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok());

        Ok(DeviceStatus {
            device_id: device_id.to_string(),
            status: parse_health_status(&row.0),
            last_poll,
            metrics: metrics.unwrap_or_default(),
            error_message: row.3,
            error_count: row.4.unwrap_or(0),
            uptime_seconds: row.5.unwrap_or(0),
        })
    }

    pub async fn insert_metric(&self, metric: &MetricPoint) -> NimonResult<()> {
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

fn parse_device_type(s: &str) -> DeviceType {
    match s {
        "daq" => DeviceType::Daq,
        "pxi" => DeviceType::Pxi,
        "cdaq" => DeviceType::CDaq,
        "visa" => DeviceType::Visa,
        "xnet" => DeviceType::Xnet,
        "gpib" => DeviceType::Gpib,
        "power_supply" => DeviceType::PowerSupply,
        _ => DeviceType::Daq,
    }
}

/// Classify a device from its name (best effort; defaults to Daq).
fn classify_device_name(name: &str) -> &'static str {
    let upper = name.to_uppercase();
    if upper.contains("PXI") {
        "pxi"
    } else if upper.contains("CDAQ") || upper.contains("CDAQ") {
        "cdaq"
    } else if upper.contains("GPIB") {
        "gpib"
    } else if upper.contains("XNET") {
        "xnet"
    } else if upper.contains("VISA") {
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
}
