//! Database layer for NIMon

pub mod device_repo;
pub mod edge_repo;
pub mod schema;

use sqlx::SqlitePool;
use crate::NimonResult;

/// Initialize the database with schema
pub async fn init_database(pool: &SqlitePool) -> NimonResult<()> {
    sqlx::raw_sql(schema::SCHEMA)
        .execute(pool)
        .await?;
    Ok(())
}

/// Create an in-memory database for testing
#[cfg(test)]
pub async fn create_test_db() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    init_database(&pool).await.unwrap();
    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_database() {
        let pool = create_test_db().await;
        // Verify tables exist
        let result: Result<(i64,), sqlx::Error> = sqlx::query_as(
            "SELECT COUNT(*) FROM edge_nodes"
        )
        .fetch_one(&pool)
        .await;
        assert!(result.is_ok());
    }
}
