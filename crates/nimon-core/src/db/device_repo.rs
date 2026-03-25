//! Repository for device operations

use sqlx::SqlitePool;
use chrono::{DateTime, Utc};
use serde_json;
use crate::{Device, DeviceType, DeviceStatus, HealthStatus, MetricPoint, NimonResult, NimonError};

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
                                 firmware_version, driver_version, ip_address, slot, chassis)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                chassis = excluded.chassis
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
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn get(&self, id: &str) -> NimonResult<Device> {
        let row: (
            String, String, String, String,
            Option<String>, Option<String>, Option<String>, Option<String>,
            Option<String>, Option<i32>, Option<String>
        ) = sqlx::query_as(
            "SELECT id, edge_id, device_name, device_type, model, serial_number,
                    firmware_version, driver_version, ip_address, slot, chassis
             FROM devices WHERE id = ?"
        )
        .bind(id)
        .fetch_one(self.pool)
        .await
        .map_err(|_| NimonError::DeviceNotFound(id.to_string()))?;

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
        })
    }

    pub async fn list_by_edge(&self, edge_id: &str) -> NimonResult<Vec<Device>> {
        let rows: Vec<(
            String, String, String, String,
            Option<String>, Option<String>, Option<String>, Option<String>,
            Option<String>, Option<i32>, Option<String>
        )> = sqlx::query_as(
            "SELECT id, edge_id, device_name, device_type, model, serial_number,
                    firmware_version, driver_version, ip_address, slot, chassis
             FROM devices WHERE edge_id = ? ORDER BY device_name"
        )
        .bind(edge_id)
        .fetch_all(self.pool)
        .await?;

        Ok(rows.into_iter().map(|row| Device {
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
        }).collect())
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
        let row: (String, Option<String>, Option<String>, Option<i32>, Option<i64>) = sqlx::query_as(
            "SELECT status, last_poll, error_message, error_count, uptime_seconds
             FROM device_status WHERE device_id = ?"
        )
        .bind(device_id)
        .fetch_one(self.pool)
        .await
        .map_err(|_| NimonError::DeviceNotFound(device_id.to_string()))?;

        Ok(DeviceStatus {
            device_id: device_id.to_string(),
            status: parse_health_status(&row.0),
            last_poll: row.1.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc)))
                .unwrap_or_else(Utc::now),
            metrics: std::collections::HashMap::new(), // Would need to deserialize
            error_message: row.2,
            error_count: row.3.unwrap_or(0),
            uptime_seconds: row.4.unwrap_or(0),
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

    pub async fn get_metrics(&self, device_id: &str, metric_name: &str, limit: i64) -> NimonResult<Vec<MetricPoint>> {
        let rows: Vec<(String, f64)> = sqlx::query_as(
            "SELECT timestamp, metric_value FROM device_metrics_history
             WHERE device_id = ? AND metric_name = ?
             ORDER BY timestamp DESC LIMIT ?"
        )
        .bind(device_id)
        .bind(metric_name)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(rows.into_iter().map(|row| MetricPoint {
            device_id: device_id.to_string(),
            timestamp: DateTime::parse_from_rfc3339(&row.0)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            metric_name: metric_name.to_string(),
            metric_value: row.1,
        }).collect())
    }
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

fn parse_health_status(s: &str) -> HealthStatus {
    match s {
        "healthy" => HealthStatus::Healthy,
        "warning" => HealthStatus::Warning,
        "error" => HealthStatus::Error,
        _ => HealthStatus::Offline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::create_test_db;
    use crate::db::edge_repo::EdgeRepository;

    #[tokio::test]
    async fn test_upsert_and_get_device() {
        let pool = create_test_db().await;

        // First create an edge node (foreign key constraint)
        let edge_repo = EdgeRepository::new(&pool);
        edge_repo.upsert(&crate::EdgeNode {
            id: "edge-1".to_string(),
            name: "Edge 1".to_string(),
            hostname: None,
            ip_address: None,
            last_seen: None,
            status: crate::EdgeStatus::Online,
        }).await.unwrap();

        let device_repo = DeviceRepository::new(&pool);
        let device = Device {
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
        };

        device_repo.upsert(&device).await.unwrap();
        let fetched = device_repo.get("device-1").await.unwrap();

        assert_eq!(fetched.id, device.id);
        assert_eq!(fetched.device_name, device.device_name);
        assert_eq!(fetched.device_type, DeviceType::Pxi);
    }

    #[tokio::test]
    async fn test_insert_and_get_metrics() {
        let pool = create_test_db().await;
        let edge_repo = EdgeRepository::new(&pool);
        edge_repo.upsert(&crate::EdgeNode {
            id: "edge-1".to_string(),
            name: "Edge 1".to_string(),
            hostname: None,
            ip_address: None,
            last_seen: None,
            status: crate::EdgeStatus::Online,
        }).await.unwrap();

        let device_repo = DeviceRepository::new(&pool);
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

        let metric = MetricPoint {
            device_id: "device-1".to_string(),
            timestamp: Utc::now(),
            metric_name: "temperature".to_string(),
            metric_value: 45.5,
        };

        device_repo.insert_metric(&metric).await.unwrap();
        let metrics = device_repo.get_metrics("device-1", "temperature", 10).await.unwrap();

        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].metric_value, 45.5);
    }
}
