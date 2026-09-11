//! Repository for edge node operations

use crate::{EdgeNode, EdgeStatus, NimonError, NimonResult};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

pub struct EdgeRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> EdgeRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert(&self, edge: &EdgeNode) -> NimonResult<()> {
        sqlx::query(
            r#"
            INSERT INTO edge_nodes (id, name, hostname, ip_address, last_seen, status)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                hostname = excluded.hostname,
                ip_address = excluded.ip_address,
                last_seen = excluded.last_seen,
                status = excluded.status
            "#,
        )
        .bind(&edge.id)
        .bind(&edge.name)
        .bind(&edge.hostname)
        .bind(&edge.ip_address)
        .bind(edge.last_seen.map(|t| t.to_rfc3339()))
        .bind(edge.status.to_string())
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn get(&self, id: &str) -> NimonResult<EdgeNode> {
        let row: (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
        ) = sqlx::query_as(
            "SELECT id, name, hostname, ip_address, last_seen, status FROM edge_nodes WHERE id = ?",
        )
        .bind(id)
        .fetch_one(self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => NimonError::EdgeNotFound(id.to_string()),
            other => NimonError::Database(other),
        })?;

        Ok(EdgeNode {
            id: row.0,
            name: row.1,
            hostname: row.2,
            ip_address: row.3,
            last_seen: row.4.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            status: parse_edge_status(&row.5),
        })
    }

    pub async fn list(&self) -> NimonResult<Vec<EdgeNode>> {
        let rows: Vec<(String, String, Option<String>, Option<String>, Option<String>, String)> =
            sqlx::query_as(
                "SELECT id, name, hostname, ip_address, last_seen, status FROM edge_nodes ORDER BY name"
            )
            .fetch_all(self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| EdgeNode {
                id: row.0,
                name: row.1,
                hostname: row.2,
                ip_address: row.3,
                last_seen: row.4.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
                status: parse_edge_status(&row.5),
            })
            .collect())
    }

    pub async fn update_status(&self, id: &str, status: EdgeStatus) -> NimonResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE edge_nodes SET status = ?, last_seen = ? WHERE id = ?")
            .bind(status.to_string())
            .bind(now)
            .bind(id)
            .execute(self.pool)
            .await?;

        Ok(())
    }
}

fn parse_edge_status(s: &str) -> EdgeStatus {
    match s {
        "online" => EdgeStatus::Online,
        "degraded" => EdgeStatus::Degraded,
        _ => EdgeStatus::Offline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::create_test_db;

    #[tokio::test]
    async fn test_upsert_and_get() {
        let pool = create_test_db().await;
        let repo = EdgeRepository::new(&pool);

        let edge = EdgeNode {
            id: "test-edge-01".to_string(),
            name: "Test Edge".to_string(),
            hostname: Some("test.local".to_string()),
            ip_address: Some("192.168.1.100".to_string()),
            last_seen: None,
            status: EdgeStatus::Online,
        };

        repo.upsert(&edge).await.unwrap();
        let fetched = repo.get("test-edge-01").await.unwrap();

        assert_eq!(fetched.id, edge.id);
        assert_eq!(fetched.name, edge.name);
        assert_eq!(fetched.status, EdgeStatus::Online);
    }

    #[tokio::test]
    async fn test_list() {
        let pool = create_test_db().await;
        let repo = EdgeRepository::new(&pool);

        for i in 1..=3 {
            repo.upsert(&EdgeNode {
                id: format!("edge-{}", i),
                name: format!("Edge {}", i),
                hostname: None,
                ip_address: None,
                last_seen: None,
                status: EdgeStatus::Offline,
            })
            .await
            .unwrap();
        }

        let edges = repo.list().await.unwrap();
        assert_eq!(edges.len(), 3);
    }

    #[tokio::test]
    async fn test_update_status() {
        let pool = create_test_db().await;
        let repo = EdgeRepository::new(&pool);

        let edge = EdgeNode {
            id: "edge-1".to_string(),
            name: "Edge 1".to_string(),
            hostname: None,
            ip_address: None,
            last_seen: None,
            status: EdgeStatus::Offline,
        };
        repo.upsert(&edge).await.unwrap();

        repo.update_status("edge-1", EdgeStatus::Online)
            .await
            .unwrap();
        let fetched = repo.get("edge-1").await.unwrap();
        assert_eq!(fetched.status, EdgeStatus::Online);
        assert!(fetched.last_seen.is_some());
    }
}
