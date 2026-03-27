//! Repository for prediction operations

use sqlx::SqlitePool;
use crate::NimonResult;

/// Database record for a prediction
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PredictionRecord {
    pub id: i64,
    pub device_id: String,
    pub edge_id: String,
    pub prediction_type: String,
    pub probability: f64,
    pub eta_minutes: Option<i64>,
    pub features: Option<String>,
    pub model_version: Option<String>,
    pub status: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

pub struct PredictionRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> PredictionRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a new prediction with status='active'. Returns the new prediction ID.
    pub async fn insert(
        &self,
        device_id: &str,
        edge_id: &str,
        prediction_type: &str,
        probability: f64,
        eta_minutes: Option<i64>,
        model_version: Option<&str>,
    ) -> NimonResult<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO predictions (device_id, edge_id, prediction_type, probability, eta_minutes, model_version, status)
            VALUES (?, ?, ?, ?, ?, ?, 'active')
            "#
        )
        .bind(device_id)
        .bind(edge_id)
        .bind(prediction_type)
        .bind(probability)
        .bind(eta_minutes)
        .bind(model_version)
        .execute(self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// List all active predictions.
    pub async fn list_active(&self) -> NimonResult<Vec<PredictionRecord>> {
        let records = sqlx::query_as::<_, PredictionRecord>(
            "SELECT id, device_id, edge_id, prediction_type, probability, eta_minutes, features, model_version, status, created_at, resolved_at FROM predictions WHERE status = 'active' ORDER BY created_at DESC"
        )
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }

    /// Dismiss a prediction by setting its status to 'dismissed'.
    pub async fn dismiss(&self, prediction_id: i64) -> NimonResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE predictions SET status = 'dismissed', resolved_at = ? WHERE id = ?"
        )
        .bind(&now)
        .bind(prediction_id)
        .execute(self.pool)
        .await?;

        Ok(())
    }

    /// List predictions for a specific device, most recent first, up to `limit` rows.
    pub async fn list_by_device(&self, device_id: &str, limit: i64) -> NimonResult<Vec<PredictionRecord>> {
        let records = sqlx::query_as::<_, PredictionRecord>(
            "SELECT id, device_id, edge_id, prediction_type, probability, eta_minutes, features, model_version, status, created_at, resolved_at FROM predictions WHERE device_id = ? ORDER BY created_at DESC LIMIT ?"
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
        let repo = PredictionRepository::new(&pool);

        let id = repo.insert(
            "device-1",
            "edge-1",
            "failure_prediction",
            0.85,
            Some(120),
            Some("v1.0.0"),
        ).await.unwrap();

        assert!(id > 0);

        let active = repo.list_active().await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].prediction_type, "failure_prediction");
        assert_eq!(active[0].status, "active");
        assert_eq!(active[0].probability, 0.85);
        assert_eq!(active[0].eta_minutes, Some(120));
        assert_eq!(active[0].model_version.as_deref(), Some("v1.0.0"));
    }

    #[tokio::test]
    async fn test_dismiss_prediction() {
        let pool = create_test_db().await;
        setup_test_data(&pool).await;
        let repo = PredictionRepository::new(&pool);

        let id = repo.insert(
            "device-1",
            "edge-1",
            "maintenance_needed",
            0.6,
            None,
            None,
        ).await.unwrap();

        repo.dismiss(id).await.unwrap();

        let active = repo.list_active().await.unwrap();
        assert_eq!(active.len(), 0);
    }

    #[tokio::test]
    async fn test_list_by_device() {
        let pool = create_test_db().await;
        setup_test_data(&pool).await;
        let repo = PredictionRepository::new(&pool);

        repo.insert("device-1", "edge-1", "type_a", 0.5, Some(30), None).await.unwrap();
        repo.insert("device-1", "edge-1", "type_b", 0.9, Some(60), None).await.unwrap();

        let device_preds = repo.list_by_device("device-1", 10).await.unwrap();
        assert_eq!(device_preds.len(), 2);
    }

    #[tokio::test]
    async fn test_list_by_device_with_limit() {
        let pool = create_test_db().await;
        setup_test_data(&pool).await;
        let repo = PredictionRepository::new(&pool);

        for i in 0..5 {
            repo.insert(
                "device-1", "edge-1",
                &format!("pred_{}", i),
                0.1 * (i as f64),
                Some(i * 10),
                None,
            ).await.unwrap();
        }

        let limited = repo.list_by_device("device-1", 2).await.unwrap();
        assert_eq!(limited.len(), 2);
    }
}
