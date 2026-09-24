//! Database layer for NIMon
//!
//! Timestamp convention: every timestamp column that is range-queried or
//! ordered on is stored as RFC3339 UTC text as produced by chrono's
//! `DateTime<Utc>::to_rfc3339()` (`YYYY-MM-DDTHH:MM:SS[.frac]+00:00`), and
//! every cutoff bound into a comparison uses the same format, so string
//! comparison equals time comparison.

pub mod action_repo;
pub mod alert_repo;
pub mod device_repo;
pub mod edge_repo;
pub mod prediction_repo;
pub mod schema;

use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::NimonResult;

pub use action_repo::{ActionHistoryFilter, ActionRecord, ActionRepository};
pub use alert_repo::{AlertHistoryFilter, AlertRecord, AlertRepository};
pub use device_repo::{DeviceMetadata, DeviceRepository};
pub use prediction_repo::{PredictionRecord, PredictionRepository};

pub use schema::{init_database, migrate, ForeignKeyViolation, MigrationReport, SCHEMA_VERSION};

#[cfg(test)]
pub use schema::create_test_db;

/// Maximum pool size for file-backed databases.
const FILE_POOL_MAX_CONNECTIONS: u32 = 4;

/// True for the in-memory spellings: `:memory:`, `sqlite::memory:`,
/// `sqlite://:memory:` or any URL with `mode=memory`.
fn is_memory(path_or_url: &str) -> bool {
    matches!(
        path_or_url,
        ":memory:" | "sqlite::memory:" | "sqlite://:memory:" | "sqlite:memory:"
    ) || path_or_url.contains("mode=memory")
}

/// Open (creating if missing) a SQLite database and run migrations.
///
/// Accepts a plain file path (`data/nimon.db`, `D:\data\nimon.db`), a
/// `sqlite:` URL (`sqlite://data/nimon.db?mode=rwc`), or `:memory:`.
///
/// File databases use WAL, `synchronous=NORMAL`, a 5 s busy timeout,
/// foreign keys ON and a pool of at most 4 connections. In-memory
/// databases use a single long-lived connection so every query sees the
/// same database.
pub async fn connect(path_or_url: &str) -> NimonResult<SqlitePool> {
    let target = path_or_url.trim();
    let memory = is_memory(target);

    let base = if matches!(
        target,
        ":memory:" | "sqlite::memory:" | "sqlite://:memory:" | "sqlite:memory:"
    ) {
        SqliteConnectOptions::from_str("sqlite::memory:")?
    } else if target.starts_with("sqlite:") {
        SqliteConnectOptions::from_str(target)?
    } else {
        SqliteConnectOptions::new().filename(target)
    };

    let mut options = base
        .create_if_missing(true)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    if !memory {
        options = options.journal_mode(SqliteJournalMode::Wal);
    }

    let pool_options = if memory {
        SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
    } else {
        SqlitePoolOptions::new().max_connections(FILE_POOL_MAX_CONNECTIONS)
    };

    let pool = pool_options.connect_with(options).await?;
    init_database(&pool).await?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connect_memory_shares_one_database() {
        let pool = connect(":memory:").await.unwrap();
        sqlx::query("INSERT INTO edge_nodes (id, name) VALUES ('e', 'E')")
            .execute(&pool)
            .await
            .unwrap();
        // Several sequential/parallel queries must all see the same DB
        for _ in 0..5 {
            let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM edge_nodes")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(n, 1);
        }
        let (fk,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[tokio::test]
    async fn test_connect_file_path_and_url() {
        let dir = std::env::temp_dir().join(format!("nimon-core-test-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("plain.db");
        let pool = connect(path.to_str().unwrap()).await.unwrap();
        let (mode,): (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let (fk,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);
        let (v,): (i64,) = sqlx::query_as("PRAGMA user_version")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        pool.close().await;

        // Re-open the same file: migrations are a no-op
        let pool = connect(path.to_str().unwrap()).await.unwrap();
        pool.close().await;

        let url = format!("sqlite://{}?mode=rwc", dir.join("url.db").display());
        let pool = connect(&url).await.unwrap();
        pool.close().await;
        assert!(dir.join("url.db").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
