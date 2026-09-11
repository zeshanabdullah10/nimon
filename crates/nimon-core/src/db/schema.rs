//! Database schema definitions and versioned migrations
//!
//! Fresh databases get the current schema directly. Existing databases
//! are migrated forward using `PRAGMA user_version`.

/// Current schema version. Bump when adding a migration below.
pub const SCHEMA_VERSION: i64 = 1;

/// Schema for fresh databases (version = SCHEMA_VERSION)
pub const SCHEMA: &str = r#"
-- Edge nodes
CREATE TABLE IF NOT EXISTS edge_nodes (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    hostname        TEXT,
    ip_address      TEXT,
    last_seen       TEXT,
    status          TEXT DEFAULT 'offline',
    created_at      TEXT DEFAULT CURRENT_TIMESTAMP
);

-- All discovered devices
CREATE TABLE IF NOT EXISTS devices (
    id              TEXT PRIMARY KEY,
    edge_id         TEXT NOT NULL REFERENCES edge_nodes(id),
    device_name     TEXT NOT NULL,
    device_type     TEXT NOT NULL,
    model           TEXT,
    serial_number   TEXT,
    firmware_version TEXT,
    driver_version  TEXT,
    ip_address      TEXT,
    slot            INTEGER,
    chassis         TEXT,
    is_simulated    INTEGER DEFAULT 0,
    created_at      TEXT DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(edge_id, device_name)
);

-- Current device status
CREATE TABLE IF NOT EXISTS device_status (
    device_id       TEXT PRIMARY KEY REFERENCES devices(id),
    status          TEXT NOT NULL,
    last_poll       TEXT,
    metrics         TEXT,
    error_message   TEXT,
    error_count     INTEGER DEFAULT 0,
    uptime_seconds  INTEGER DEFAULT 0,
    updated_at      TEXT DEFAULT CURRENT_TIMESTAMP
);

-- Time-series metrics
CREATE TABLE IF NOT EXISTS device_metrics_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id       TEXT NOT NULL REFERENCES devices(id),
    timestamp       TEXT NOT NULL,
    metric_name     TEXT NOT NULL,
    metric_value    REAL NOT NULL,
    created_at      TEXT DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_metrics_device_time
    ON device_metrics_history(device_id, timestamp DESC);

-- Predictions
CREATE TABLE IF NOT EXISTS predictions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id       TEXT NOT NULL,
    edge_id         TEXT NOT NULL,
    prediction_type TEXT NOT NULL,
    probability     REAL NOT NULL,
    eta_minutes     INTEGER,
    features        TEXT,
    model_version   TEXT,
    status          TEXT DEFAULT 'active',
    created_at      TEXT DEFAULT CURRENT_TIMESTAMP,
    resolved_at     TEXT
);

-- Alerts (ULID string primary keys)
CREATE TABLE IF NOT EXISTS alerts (
    id                TEXT PRIMARY KEY,
    device_id         TEXT REFERENCES devices(id),
    edge_id           TEXT,
    rule_name         TEXT NOT NULL,
    severity          TEXT NOT NULL,
    message           TEXT NOT NULL,
    title             TEXT,
    metric_name       TEXT,
    metric_value      REAL,
    threshold         REAL,
    channels          TEXT,
    status            TEXT DEFAULT 'firing',
    action_taken      TEXT,
    action_result     TEXT,
    notification_sent INTEGER DEFAULT 0,
    fired_count       INTEGER DEFAULT 1,
    triggered_at      TEXT,
    created_at        TEXT DEFAULT CURRENT_TIMESTAMP,
    resolved_at       TEXT
);
CREATE INDEX IF NOT EXISTS idx_alerts_status
    ON alerts(status, created_at DESC);

-- Action history
CREATE TABLE IF NOT EXISTS action_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    alert_id        TEXT REFERENCES alerts(id),
    device_id       TEXT NOT NULL,
    action_id       TEXT NOT NULL,
    action_type     TEXT NOT NULL,
    command         TEXT,
    exit_code       INTEGER,
    output          TEXT,
    duration_ms     INTEGER,
    success         INTEGER,
    retry_count     INTEGER DEFAULT 0,
    executed_at     TEXT DEFAULT CURRENT_TIMESTAMP
);
"#;

/// New-shape alerts table used as the migration target.
const CREATE_ALERTS_NEW: &str = r#"
CREATE TABLE alerts_new (
    id                TEXT PRIMARY KEY,
    device_id         TEXT REFERENCES devices(id),
    edge_id           TEXT,
    rule_name         TEXT NOT NULL,
    severity          TEXT NOT NULL,
    message           TEXT NOT NULL,
    title             TEXT,
    metric_name       TEXT,
    metric_value      REAL,
    threshold         REAL,
    channels          TEXT,
    status            TEXT DEFAULT 'firing',
    action_taken      TEXT,
    action_result     TEXT,
    notification_sent INTEGER DEFAULT 0,
    fired_count       INTEGER DEFAULT 1,
    triggered_at      TEXT,
    created_at        TEXT DEFAULT CURRENT_TIMESTAMP,
    resolved_at       TEXT
)"#;

const CREATE_ALERTS_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_alerts_status ON alerts(status, created_at DESC)";

/// Migrate the pre-`user_version` (legacy) alerts table to the current shape:
/// TEXT ids, lifecycle columns. Old integer ids are preserved as strings
/// (they cannot collide with ULIDs).
///
/// Handles every reachable partial state (e.g. a previously interrupted
/// run left `alerts_new` behind with or without `alerts` still present).
/// Statements run with foreign keys OFF: dropping the legacy `alerts`
/// table performs an implicit DELETE that `action_history` references
/// could otherwise reject.
async fn migrate_legacy_alerts(
    pool: &sqlx::SqlitePool,
    has_alerts: bool,
    has_alerts_new: bool,
) -> crate::NimonResult<()> {
    sqlx::raw_sql("PRAGMA foreign_keys = OFF")
        .execute(pool)
        .await?;
    let result = migrate_legacy_alerts_inner(pool, has_alerts, has_alerts_new).await;
    sqlx::raw_sql("PRAGMA foreign_keys = ON")
        .execute(pool)
        .await?;
    result
}

async fn migrate_legacy_alerts_inner(
    pool: &sqlx::SqlitePool,
    has_alerts: bool,
    has_alerts_new: bool,
) -> crate::NimonResult<()> {
    if has_alerts {
        // Full path: rebuild via the new-shape table. Clear any stale
        // partial target first.
        sqlx::raw_sql("DROP TABLE IF EXISTS alerts_new")
            .execute(pool)
            .await?;
        sqlx::raw_sql(CREATE_ALERTS_NEW).execute(pool).await?;
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO alerts_new
                (id, device_id, edge_id, rule_name, severity, message,
                 channels, status, action_taken, action_result, created_at, resolved_at)
            SELECT CAST(id AS TEXT), device_id, edge_id, rule_name, severity, message,
                   channels, status, action_taken, action_result, created_at, resolved_at
            FROM alerts
            "#,
        )
        .execute(pool)
        .await?;
        sqlx::raw_sql("DROP TABLE alerts").execute(pool).await?;
        sqlx::raw_sql("ALTER TABLE alerts_new RENAME TO alerts")
            .execute(pool)
            .await?;
    } else if has_alerts_new {
        // Partial migration recovery: the legacy table is already gone,
        // only the rename/index steps are missing.
        sqlx::raw_sql("ALTER TABLE alerts_new RENAME TO alerts")
            .execute(pool)
            .await?;
    }
    sqlx::raw_sql(CREATE_ALERTS_INDEX).execute(pool).await?;
    Ok(())
}

/// Add `is_simulated` to devices if missing (idempotent).
const MIGRATE_DEVICES_V0_V1: &str = "ALTER TABLE devices ADD COLUMN is_simulated INTEGER DEFAULT 0";

/// Read the current schema version (`PRAGMA user_version`).
async fn schema_version(pool: &sqlx::SqlitePool) -> sqlx::Result<i64> {
    let (version,): (i64,) = sqlx::query_as("PRAGMA user_version")
        .fetch_one(pool)
        .await?;
    Ok(version)
}

/// True when a named table already exists (i.e. the DB predates migrations).
async fn table_exists(pool: &sqlx::SqlitePool, name: &str) -> sqlx::Result<bool> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(name)
            .fetch_one(pool)
            .await?;
    Ok(count > 0)
}

/// Initialize the database schema, running any pending migrations.
pub async fn init_database(pool: &sqlx::SqlitePool) -> crate::NimonResult<()> {
    let version = schema_version(pool).await?;
    let has_alerts = table_exists(pool, "alerts").await?;
    let has_alerts_new = table_exists(pool, "alerts_new").await?;
    let legacy = version == 0 && (has_alerts || has_alerts_new);

    if legacy {
        // Existing database from before the migration system: bring the
        // alerts table forward first (old column layout, possibly a
        // half-finished earlier migration), then apply the full
        // (idempotent) schema so any missing tables appear.
        migrate_legacy_alerts(pool, has_alerts, has_alerts_new).await?;
        if !column_exists(pool, "devices", "is_simulated").await? {
            sqlx::raw_sql(MIGRATE_DEVICES_V0_V1).execute(pool).await?;
        }
    }

    sqlx::raw_sql(SCHEMA).execute(pool).await?;

    if version < SCHEMA_VERSION {
        sqlx::raw_sql(&format!("PRAGMA user_version = {}", SCHEMA_VERSION))
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn column_exists(pool: &sqlx::SqlitePool, table: &str, column: &str) -> sqlx::Result<bool> {
    let (count,): (i64,) = sqlx::query_as(&format!(
        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = ?",
        table
    ))
    .bind(column)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

/// Create an in-memory database for testing
#[cfg(test)]
pub async fn create_test_db() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
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
        let result: Result<(i64,), sqlx::Error> = sqlx::query_as("SELECT COUNT(*) FROM edge_nodes")
            .fetch_one(&pool)
            .await;
        assert!(result.is_ok());
        // Schema version stamped
        assert_eq!(schema_version(&pool).await.unwrap(), SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn test_migrates_legacy_alerts_table() {
        // Build a legacy (pre-user_version) database with the OLD layout
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        sqlx::raw_sql(
            r#"
            CREATE TABLE edge_nodes (id TEXT PRIMARY KEY, name TEXT NOT NULL, hostname TEXT,
                ip_address TEXT, last_seen TEXT, status TEXT DEFAULT 'offline',
                config TEXT, created_at TEXT DEFAULT CURRENT_TIMESTAMP);
            CREATE TABLE devices (id TEXT PRIMARY KEY, edge_id TEXT NOT NULL, device_name TEXT NOT NULL,
                device_type TEXT NOT NULL, model TEXT, serial_number TEXT, firmware_version TEXT,
                driver_version TEXT, ip_address TEXT, slot INTEGER, chassis TEXT, config TEXT,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP, UNIQUE(edge_id, device_name));
            CREATE TABLE alerts (id INTEGER PRIMARY KEY AUTOINCREMENT, device_id TEXT, edge_id TEXT,
                rule_name TEXT NOT NULL, severity TEXT NOT NULL, message TEXT NOT NULL, channels TEXT,
                status TEXT DEFAULT 'pending', action_taken TEXT, action_result TEXT,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP, resolved_at TEXT);
            INSERT INTO edge_nodes (id, name) VALUES ('edge-1', 'Edge 1');
            INSERT INTO devices (id, edge_id, device_name, device_type) VALUES ('device-1', 'edge-1', 'DAQ-1', 'daq');
            INSERT INTO alerts (device_id, edge_id, rule_name, severity, message, status)
                VALUES ('device-1', 'edge-1', 'high_temp', 'warning', 'old alert', 'pending');
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        // Migrate
        init_database(&pool).await.unwrap();

        // Old row preserved with string id and new columns present
        let row: (String, Option<String>, Option<i64>, i64) = sqlx::query_as(
            "SELECT id, title, metric_name, notification_sent FROM alerts WHERE rule_name = 'high_temp'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "1");
        assert!(row.1.is_none());
        assert_eq!(row.3, 0);

        // user_version stamped
        assert_eq!(schema_version(&pool).await.unwrap(), SCHEMA_VERSION);

        // New inserts use TEXT ids and lifecycle columns
        sqlx::query("INSERT INTO alerts (id, rule_name, severity, message, status, triggered_at) VALUES ('01HTEST', 'r2', 'info', 'm', 'firing', '2024-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_recovers_from_partial_migration() {
        // Reproduce a half-finished migration: `alerts` is gone, the
        // `alerts_new` target table is left behind, user_version is 0.
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        sqlx::raw_sql(
            r#"
            CREATE TABLE edge_nodes (id TEXT PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE devices (id TEXT PRIMARY KEY, edge_id TEXT NOT NULL, device_name TEXT NOT NULL,
                device_type TEXT NOT NULL, is_simulated INTEGER DEFAULT 0);
            CREATE TABLE alerts_new (id TEXT PRIMARY KEY, device_id TEXT, edge_id TEXT,
                rule_name TEXT NOT NULL, severity TEXT NOT NULL, message TEXT NOT NULL,
                title TEXT, metric_name TEXT, metric_value REAL, threshold REAL, channels TEXT,
                status TEXT DEFAULT 'firing', action_taken TEXT, action_result TEXT,
                notification_sent INTEGER DEFAULT 0, fired_count INTEGER DEFAULT 1,
                triggered_at TEXT, created_at TEXT DEFAULT CURRENT_TIMESTAMP, resolved_at TEXT);
            INSERT INTO alerts_new (id, rule_name, severity, message)
                VALUES ('01SAVED', 'kept_rule', 'info', 'survived partial migration');
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        init_database(&pool).await.unwrap();

        // The saved row survives under the final `alerts` name; alerts_new is gone
        let (rule,): (String,) =
            sqlx::query_as("SELECT rule_name FROM alerts WHERE id = '01SAVED'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(rule, "kept_rule");
        assert!(!table_exists(&pool, "alerts_new").await.unwrap());
        assert_eq!(schema_version(&pool).await.unwrap(), SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn test_reinit_is_idempotent() {
        let pool = create_test_db().await;
        // Running init again must not fail or duplicate
        init_database(&pool).await.unwrap();
        assert_eq!(schema_version(&pool).await.unwrap(), SCHEMA_VERSION);
    }
}
