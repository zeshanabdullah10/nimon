//! Database schema definitions and versioned migrations
//!
//! The schema version lives in `PRAGMA user_version`. Migrations run
//! stepwise (`0 -> 1 -> 2 ...`) on ONE connection, with foreign keys OFF,
//! inside a single `BEGIN IMMEDIATE` transaction that also bumps
//! `user_version`, so a failure leaves the database untouched. After
//! COMMIT, `PRAGMA foreign_key_check` is run and foreign keys are turned
//! back ON for that connection.
//!
//! Versions:
//! - 0: fresh database, or a pre-migration ("legacy") database.
//! - 1: baseline schema (TEXT alert ids, lifecycle columns,
//!   `devices.is_simulated`).
//! - 2: RFC3339 timestamps everywhere that is range-queried,
//!   `alerts.last_fired_at` / `alerts.acknowledged_at`,
//!   `action_history.alert_id ... ON DELETE SET NULL`, query indexes,
//!   `predictions` without the legacy FK to `devices`.
//!
//! Fresh databases go through the same steps as existing ones, so there
//! is a single definition of the final shape.

use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::NimonResult;

/// Current schema version. Bump when adding a migration step below.
pub const SCHEMA_VERSION: i64 = 2;

/// Baseline (version 1) schema. Frozen: later changes go into steps.
const SCHEMA_V1: &str = r#"
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

/// Version-1 alerts table used as the legacy migration target.
const CREATE_ALERTS_NEW_V1: &str = r#"
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

/// SQL expression for "now" in the RFC3339 UTC storage format.
const NOW_RFC3339: &str = "strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now')";

/// GLOB matching SQLite's `CURRENT_TIMESTAMP` format (`YYYY-MM-DD HH:MM:SS`).
const SQLITE_TS_GLOB: &str =
    "'[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9][0-9]:[0-9][0-9]:[0-9][0-9]*'";

/// SQL expression converting `col` from SQLite `CURRENT_TIMESTAMP` format
/// to RFC3339 UTC; other values (already RFC3339, NULL) pass through.
fn to_rfc3339_sql(col: &str) -> String {
    format!(
        "(CASE WHEN {col} GLOB {SQLITE_TS_GLOB} \
         THEN strftime('%Y-%m-%dT%H:%M:%S+00:00', {col}) ELSE {col} END)"
    )
}

/// `UPDATE` normalizing one timestamp column to RFC3339 in place.
fn normalize_ts_sql(table: &str, col: &str) -> String {
    format!(
        "UPDATE {table} SET {col} = {} WHERE {col} GLOB {SQLITE_TS_GLOB}",
        to_rfc3339_sql(col)
    )
}

/// Outcome of a migration run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// `user_version` before the run
    pub from_version: i64,
    /// `user_version` after the run (unchanged when already current, or
    /// when the database is newer than this build)
    pub to_version: i64,
    /// Pre-existing rows violating foreign keys (from `PRAGMA
    /// foreign_key_check`). They are left in place; new writes are
    /// checked because foreign keys are ON.
    pub foreign_key_violations: Vec<ForeignKeyViolation>,
}

/// One row of `PRAGMA foreign_key_check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKeyViolation {
    pub table: String,
    pub rowid: Option<i64>,
    pub parent: String,
}

/// Initialize the database schema, running any pending migrations.
pub async fn init_database(pool: &SqlitePool) -> NimonResult<()> {
    migrate(pool).await.map(|_| ())
}

/// Run pending migrations and report what happened.
pub async fn migrate(pool: &SqlitePool) -> NimonResult<MigrationReport> {
    let mut conn = pool.acquire().await?;
    sqlx::raw_sql("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await?;
    let result = migrate_on(&mut conn).await;
    // Always restore FK enforcement before the connection returns to the
    // pool. The pragma is a silent no-op inside an open transaction, so
    // read it back; a connection left in a bad state is closed, not pooled.
    let restore = sqlx::raw_sql("PRAGMA foreign_keys = ON")
        .execute(&mut *conn)
        .await;
    let fk_on = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
        .fetch_one(&mut *conn)
        .await
        .map(|v| v == 1)
        .unwrap_or(false);
    if restore.is_err() || !fk_on {
        drop(conn.detach());
    }
    let report = result?;
    restore?;
    if !fk_on {
        return Err(crate::NimonError::InvalidState(
            "foreign key enforcement could not be restored after migration".into(),
        ));
    }
    Ok(report)
}

async fn migrate_on(conn: &mut SqliteConnection) -> NimonResult<MigrationReport> {
    sqlx::raw_sql("BEGIN IMMEDIATE").execute(&mut *conn).await?;
    let steps = run_steps(conn).await;
    let (from_version, to_version) = match steps {
        Ok(v) => {
            if let Err(e) = sqlx::raw_sql("COMMIT").execute(&mut *conn).await {
                let _ = sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await;
                return Err(e.into());
            }
            v
        }
        Err(e) => {
            let _ = sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await;
            return Err(e);
        }
    };

    let rows = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *conn)
        .await?;
    let foreign_key_violations = rows
        .iter()
        .map(|r| ForeignKeyViolation {
            table: r.try_get::<String, _>(0).unwrap_or_default(),
            rowid: r.try_get::<Option<i64>, _>(1).unwrap_or(None),
            parent: r.try_get::<String, _>(2).unwrap_or_default(),
        })
        .collect();

    Ok(MigrationReport {
        from_version,
        to_version,
        foreign_key_violations,
    })
}

/// Apply every pending step inside the open transaction.
async fn run_steps(conn: &mut SqliteConnection) -> NimonResult<(i64, i64)> {
    let from = schema_version(&mut *conn).await?;
    let mut version = from;
    // A database newer than this build is left alone (additive schema).
    while version < SCHEMA_VERSION {
        match version {
            0 => migrate_0_to_1(conn).await?,
            1 => migrate_1_to_2(conn).await?,
            other => {
                return Err(crate::NimonError::InvalidState(format!(
                    "no migration from schema version {other}"
                )))
            }
        }
        version += 1;
        sqlx::raw_sql(&format!("PRAGMA user_version = {version}"))
            .execute(&mut *conn)
            .await?;
    }
    Ok((from, version))
}

// ---------------------------------------------------------------------------
// Step 0 -> 1: baseline (handles fresh, legacy and half-migrated databases)
// ---------------------------------------------------------------------------

async fn migrate_0_to_1(conn: &mut SqliteConnection) -> NimonResult<()> {
    let has_alerts = table_exists(&mut *conn, "alerts").await?;
    let has_alerts_new = table_exists(&mut *conn, "alerts_new").await?;

    // Leftovers of an interrupted pre-transactional migration
    if !has_alerts && has_alerts_new {
        // Legacy table already dropped: only the rename was missing
        sqlx::raw_sql("ALTER TABLE alerts_new RENAME TO alerts")
            .execute(&mut *conn)
            .await?;
    } else if has_alerts_new {
        // Legacy table still present: it is the source of truth
        sqlx::raw_sql("DROP TABLE alerts_new")
            .execute(&mut *conn)
            .await?;
    }

    // Legacy detection by shape, not by version: only an alerts table
    // without the lifecycle columns is rebuilt.
    if has_alerts && !column_exists(&mut *conn, "alerts", "triggered_at").await? {
        migrate_legacy_alerts(conn).await?;
    }

    if table_exists(&mut *conn, "devices").await?
        && !column_exists(&mut *conn, "devices", "is_simulated").await?
    {
        sqlx::raw_sql("ALTER TABLE devices ADD COLUMN is_simulated INTEGER DEFAULT 0")
            .execute(&mut *conn)
            .await?;
    }

    // Create whatever is still missing (idempotent)
    sqlx::raw_sql(SCHEMA_V1).execute(&mut *conn).await?;
    Ok(())
}

/// Rebuild the legacy alerts table (INTEGER ids, no lifecycle columns)
/// into the version-1 shape. Old ids are kept as strings (they cannot
/// collide with ULIDs); statuses are mapped to the lifecycle
/// (`pending`/unknown and unresolved -> `firing`, resolved or with a
/// `resolved_at` -> `resolved`); timestamps become RFC3339.
async fn migrate_legacy_alerts(conn: &mut SqliteConnection) -> NimonResult<()> {
    let resolved_cond =
        "(LOWER(COALESCE(status, '')) IN ('resolved', 'closed', 'cleared', 'dismissed') \
                         OR resolved_at IS NOT NULL)";
    let created = to_rfc3339_sql("created_at");
    let resolved = to_rfc3339_sql("resolved_at");
    let copy = format!(
        r#"
        INSERT OR IGNORE INTO alerts_new
            (id, device_id, edge_id, rule_name, severity, message, channels, status,
             action_taken, action_result, notification_sent, fired_count,
             triggered_at, created_at, resolved_at)
        SELECT CAST(id AS TEXT), device_id, edge_id, rule_name, severity, message, channels,
               CASE
                   WHEN {resolved_cond} THEN 'resolved'
                   WHEN LOWER(status) IN ('acknowledged', 'acked', 'ack') THEN 'acknowledged'
                   WHEN LOWER(status) = 'suppressed' THEN 'suppressed'
                   ELSE 'firing'
               END,
               action_taken, action_result, 0, 1,
               COALESCE({created}, {NOW_RFC3339}),
               COALESCE({created}, {NOW_RFC3339}),
               CASE WHEN {resolved_cond}
                    THEN COALESCE({resolved}, {created}, {NOW_RFC3339})
                    ELSE NULL END
        FROM alerts
        "#
    );

    sqlx::raw_sql(CREATE_ALERTS_NEW_V1)
        .execute(&mut *conn)
        .await?;
    sqlx::raw_sql(&copy).execute(&mut *conn).await?;
    sqlx::raw_sql("DROP TABLE alerts")
        .execute(&mut *conn)
        .await?;
    sqlx::raw_sql("ALTER TABLE alerts_new RENAME TO alerts")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 1 -> 2
// ---------------------------------------------------------------------------

/// Version-2 action_history: alert references are nulled when the alert
/// is deleted (retention), timestamps default to RFC3339.
const CREATE_ACTION_HISTORY_NEW: &str = r#"
CREATE TABLE action_history_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    alert_id        TEXT REFERENCES alerts(id) ON DELETE SET NULL,
    device_id       TEXT NOT NULL,
    action_id       TEXT NOT NULL,
    action_type     TEXT NOT NULL,
    command         TEXT,
    exit_code       INTEGER,
    output          TEXT,
    duration_ms     INTEGER,
    success         INTEGER,
    retry_count     INTEGER DEFAULT 0,
    executed_at     TEXT DEFAULT (strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now'))
)"#;

/// Carry `old`'s AUTOINCREMENT high-water mark over to `new` (before `old`
/// is dropped) so rebuilt tables never reuse ids already shown elsewhere.
fn carry_sequence_sql(old: &str, new: &str) -> String {
    format!(
        r#"
        UPDATE sqlite_sequence
           SET seq = MAX(seq, COALESCE((SELECT seq FROM sqlite_sequence WHERE name = '{old}'), 0))
         WHERE name = '{new}';
        INSERT INTO sqlite_sequence (name, seq)
            SELECT '{new}', seq FROM sqlite_sequence
             WHERE name = '{old}'
               AND NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = '{new}');
        "#
    )
}

/// Version-2 predictions: no FK to devices (predictions may arrive for
/// devices not yet registered), RFC3339 default timestamp.
const CREATE_PREDICTIONS_NEW: &str = r#"
CREATE TABLE predictions_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id       TEXT NOT NULL,
    edge_id         TEXT NOT NULL,
    prediction_type TEXT NOT NULL,
    probability     REAL NOT NULL,
    eta_minutes     INTEGER,
    features        TEXT,
    model_version   TEXT,
    status          TEXT DEFAULT 'active',
    created_at      TEXT DEFAULT (strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now')),
    resolved_at     TEXT
)"#;

/// Indexes for the queries the repositories actually issue.
const INDEXES_V2: &str = r#"
-- DeviceRepository::get_metrics (device_id, metric_name ORDER BY timestamp)
CREATE INDEX IF NOT EXISTS idx_metrics_device_name_time
    ON device_metrics_history(device_id, metric_name, timestamp);
-- DeviceRepository::prune_metrics (timestamp < cutoff)
CREATE INDEX IF NOT EXISTS idx_metrics_time
    ON device_metrics_history(timestamp);
-- superseded by idx_metrics_device_name_time
DROP INDEX IF EXISTS idx_metrics_device_time;

-- AlertRepository::list_active / list_history(status) ORDER BY created_at
CREATE INDEX IF NOT EXISTS idx_alerts_status
    ON alerts(status, created_at DESC);
-- AlertRepository::list_by_device / list_history(device_id)
CREATE INDEX IF NOT EXISTS idx_alerts_device
    ON alerts(device_id, created_at);
-- AlertRepository::list_history(since/until) ORDER BY triggered_at
CREATE INDEX IF NOT EXISTS idx_alerts_triggered
    ON alerts(triggered_at);
-- AlertRepository::prune_resolved (status = 'resolved' AND resolved_at < cutoff)
CREATE INDEX IF NOT EXISTS idx_alerts_resolved
    ON alerts(status, resolved_at);

-- ActionRepository::list_by_device / list(device_id)
CREATE INDEX IF NOT EXISTS idx_actions_device_time
    ON action_history(device_id, executed_at);
-- ActionRepository::list_recent / prune_older_than
CREATE INDEX IF NOT EXISTS idx_actions_time
    ON action_history(executed_at);
-- ON DELETE SET NULL lookups when alerts are pruned
CREATE INDEX IF NOT EXISTS idx_actions_alert
    ON action_history(alert_id);

-- PredictionRepository::list_active / expire_stale
CREATE INDEX IF NOT EXISTS idx_predictions_status_time
    ON predictions(status, created_at);
-- PredictionRepository::prune
CREATE INDEX IF NOT EXISTS idx_predictions_time
    ON predictions(created_at);
-- PredictionRepository::list_by_device
CREATE INDEX IF NOT EXISTS idx_predictions_device_time
    ON predictions(device_id, created_at);
"#;

async fn migrate_1_to_2(conn: &mut SqliteConnection) -> NimonResult<()> {
    // (a) RFC3339 timestamps for range-queried/ordered columns
    for (table, col) in [
        ("alerts", "created_at"),
        ("alerts", "triggered_at"),
        ("alerts", "resolved_at"),
        ("predictions", "created_at"),
        ("predictions", "resolved_at"),
        ("action_history", "executed_at"),
        ("device_metrics_history", "timestamp"),
    ] {
        sqlx::raw_sql(&normalize_ts_sql(table, col))
            .execute(&mut *conn)
            .await?;
    }
    // Rows from older migrations without a trigger time; resolved rows
    // without resolved_at could never be pruned by retention.
    sqlx::raw_sql(&format!(
        "UPDATE alerts SET triggered_at = COALESCE(created_at, {NOW_RFC3339}) WHERE triggered_at IS NULL;
         UPDATE alerts SET resolved_at = COALESCE(triggered_at, created_at, {NOW_RFC3339})
             WHERE status = 'resolved' AND resolved_at IS NULL;"
    ))
    .execute(&mut *conn)
    .await?;

    // (b) alert lifecycle columns
    for col in ["last_fired_at", "acknowledged_at"] {
        if !column_exists(&mut *conn, "alerts", col).await? {
            sqlx::raw_sql(&format!("ALTER TABLE alerts ADD COLUMN {col} TEXT"))
                .execute(&mut *conn)
                .await?;
        }
    }
    sqlx::raw_sql("UPDATE alerts SET last_fired_at = triggered_at WHERE last_fired_at IS NULL")
        .execute(&mut *conn)
        .await?;

    // (c) action_history: alert_id ON DELETE SET NULL. Dangling alert
    // references (e.g. legacy integer ids) are nulled during the copy.
    sqlx::raw_sql("DROP TABLE IF EXISTS action_history_new")
        .execute(&mut *conn)
        .await?;
    sqlx::raw_sql(CREATE_ACTION_HISTORY_NEW)
        .execute(&mut *conn)
        .await?;
    sqlx::raw_sql(&format!(
        r#"
        INSERT INTO action_history_new
            (id, alert_id, device_id, action_id, action_type, command, exit_code,
             output, duration_ms, success, retry_count, executed_at)
        SELECT id,
               CASE WHEN CAST(alert_id AS TEXT) IN (SELECT id FROM alerts)
                    THEN CAST(alert_id AS TEXT) ELSE NULL END,
               device_id, action_id, action_type, command, exit_code,
               output, duration_ms, success, COALESCE(retry_count, 0),
               COALESCE(executed_at, {NOW_RFC3339})
        FROM action_history;
        {carry}
        DROP TABLE action_history;
        ALTER TABLE action_history_new RENAME TO action_history;
        "#,
        carry = carry_sequence_sql("action_history", "action_history_new")
    ))
    .execute(&mut *conn)
    .await?;

    // (d) legacy predictions carried an FK to devices: drop it
    if has_foreign_keys(&mut *conn, "predictions").await? {
        sqlx::raw_sql("DROP TABLE IF EXISTS predictions_new")
            .execute(&mut *conn)
            .await?;
        sqlx::raw_sql(CREATE_PREDICTIONS_NEW)
            .execute(&mut *conn)
            .await?;
        sqlx::raw_sql(&format!(
            r#"
            INSERT INTO predictions_new
                (id, device_id, edge_id, prediction_type, probability, eta_minutes,
                 features, model_version, status, created_at, resolved_at)
            SELECT id, device_id, edge_id, prediction_type, probability, eta_minutes,
                   features, model_version, COALESCE(status, 'active'),
                   COALESCE(created_at, {NOW_RFC3339}), resolved_at
            FROM predictions;
            {carry}
            DROP TABLE predictions;
            ALTER TABLE predictions_new RENAME TO predictions;
            "#,
            carry = carry_sequence_sql("predictions", "predictions_new")
        ))
        .execute(&mut *conn)
        .await?;
    }

    // (e) indexes
    sqlx::raw_sql(INDEXES_V2).execute(&mut *conn).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Introspection helpers
// ---------------------------------------------------------------------------

/// Read the current schema version (`PRAGMA user_version`).
async fn schema_version(conn: &mut SqliteConnection) -> sqlx::Result<i64> {
    let (version,): (i64,) = sqlx::query_as("PRAGMA user_version")
        .fetch_one(&mut *conn)
        .await?;
    Ok(version)
}

/// True when a named table exists.
async fn table_exists(conn: &mut SqliteConnection, name: &str) -> sqlx::Result<bool> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(name)
            .fetch_one(&mut *conn)
            .await?;
    Ok(count > 0)
}

/// True when `table` has a column named `column`.
async fn column_exists(
    conn: &mut SqliteConnection,
    table: &str,
    column: &str,
) -> sqlx::Result<bool> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM pragma_table_info(?) WHERE name = ?")
            .bind(table)
            .bind(column)
            .fetch_one(&mut *conn)
            .await?;
    Ok(count > 0)
}

/// True when `table` declares any foreign key.
async fn has_foreign_keys(conn: &mut SqliteConnection, table: &str) -> sqlx::Result<bool> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pragma_foreign_key_list(?)")
        .bind(table)
        .fetch_one(&mut *conn)
        .await?;
    Ok(count > 0)
}

/// Create an in-memory database for testing
#[cfg(test)]
pub async fn create_test_db() -> SqlitePool {
    super::connect(":memory:").await.unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single-connection in-memory pool WITHOUT migrations, for
    /// building old-shape databases.
    async fn raw_memory_pool() -> SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    async fn version_of(pool: &SqlitePool) -> i64 {
        let mut c = pool.acquire().await.unwrap();
        schema_version(&mut c).await.unwrap()
    }

    async fn table_sql(pool: &SqlitePool, name: &str) -> String {
        let (sql,): (String,) =
            sqlx::query_as("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?")
                .bind(name)
                .fetch_one(pool)
                .await
                .unwrap();
        sql
    }

    async fn index_names(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_as::<_, (String,)>(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%'",
        )
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.0)
        .collect()
    }

    #[tokio::test]
    async fn test_init_database() {
        let pool = create_test_db().await;
        let result: Result<(i64,), sqlx::Error> = sqlx::query_as("SELECT COUNT(*) FROM edge_nodes")
            .fetch_one(&pool)
            .await;
        assert!(result.is_ok());
        assert_eq!(version_of(&pool).await, SCHEMA_VERSION);
        assert!(table_sql(&pool, "action_history")
            .await
            .contains("ON DELETE SET NULL"));
        let idx = index_names(&pool).await;
        for expected in [
            "idx_metrics_device_name_time",
            "idx_metrics_time",
            "idx_alerts_status",
            "idx_alerts_device",
            "idx_actions_device_time",
            "idx_actions_time",
            "idx_predictions_status_time",
        ] {
            assert!(idx.iter().any(|i| i == expected), "missing {expected}");
        }
        assert!(!idx.iter().any(|i| i == "idx_metrics_device_time"));
    }

    #[tokio::test]
    async fn test_fresh_migration_report() {
        let pool = raw_memory_pool().await;
        let report = migrate(&pool).await.unwrap();
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, SCHEMA_VERSION);
        assert!(report.foreign_key_violations.is_empty());
        // Second run is a no-op
        let report = migrate(&pool).await.unwrap();
        assert_eq!(report.from_version, SCHEMA_VERSION);
        assert_eq!(report.to_version, SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn test_migrates_legacy_alerts_table() {
        // Build a legacy (pre-user_version) database with the OLD layout
        let pool = raw_memory_pool().await;
        sqlx::raw_sql(
            r#"
            PRAGMA foreign_keys = OFF;
            CREATE TABLE edge_nodes (id TEXT PRIMARY KEY, name TEXT NOT NULL, hostname TEXT,
                ip_address TEXT, last_seen TEXT, status TEXT DEFAULT 'offline',
                config TEXT, created_at TEXT DEFAULT CURRENT_TIMESTAMP);
            CREATE TABLE devices (id TEXT PRIMARY KEY, edge_id TEXT NOT NULL, device_name TEXT NOT NULL,
                device_type TEXT NOT NULL, model TEXT, serial_number TEXT, firmware_version TEXT,
                driver_version TEXT, ip_address TEXT, slot INTEGER, chassis TEXT, config TEXT,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP, UNIQUE(edge_id, device_name));
            CREATE TABLE predictions (id INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id TEXT NOT NULL REFERENCES devices(id), edge_id TEXT NOT NULL,
                prediction_type TEXT NOT NULL, probability REAL NOT NULL, eta_minutes INTEGER,
                features TEXT, model_version TEXT, status TEXT DEFAULT 'active',
                created_at TEXT DEFAULT CURRENT_TIMESTAMP, resolved_at TEXT);
            CREATE TABLE alerts (id INTEGER PRIMARY KEY AUTOINCREMENT, device_id TEXT, edge_id TEXT,
                rule_name TEXT NOT NULL, severity TEXT NOT NULL, message TEXT NOT NULL, channels TEXT,
                status TEXT DEFAULT 'pending', action_taken TEXT, action_result TEXT,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP, resolved_at TEXT);
            CREATE TABLE action_history (id INTEGER PRIMARY KEY AUTOINCREMENT,
                alert_id INTEGER REFERENCES alerts(id), device_id TEXT NOT NULL,
                action_id TEXT NOT NULL, action_type TEXT NOT NULL, command TEXT,
                exit_code INTEGER, output TEXT, duration_ms INTEGER, success INTEGER,
                retry_count INTEGER DEFAULT 0, executed_at TEXT DEFAULT CURRENT_TIMESTAMP);
            INSERT INTO edge_nodes (id, name) VALUES ('edge-1', 'Edge 1');
            INSERT INTO devices (id, edge_id, device_name, device_type) VALUES ('device-1', 'edge-1', 'DAQ-1', 'daq');
            INSERT INTO alerts (device_id, edge_id, rule_name, severity, message, status, created_at)
                VALUES ('device-1', 'edge-1', 'high_temp', 'warning', 'old alert', 'pending', '2024-03-01 12:34:56');
            INSERT INTO alerts (device_id, edge_id, rule_name, severity, message, status, created_at, resolved_at)
                VALUES ('device-1', 'edge-1', 'low_temp', 'info', 'done', 'resolved', '2024-03-01 10:00:00', '2024-03-01 11:00:00');
            INSERT INTO alerts (device_id, edge_id, rule_name, severity, message, status, created_at)
                VALUES ('device-1', 'edge-1', 'acked', 'info', 'seen', 'acknowledged', '2024-03-02 10:00:00');
            INSERT INTO action_history (alert_id, device_id, action_id, action_type, executed_at)
                VALUES (2, 'device-1', 'act-1', 'restart', '2024-03-01 10:30:00');
            INSERT INTO action_history (alert_id, device_id, action_id, action_type)
                VALUES (999, 'device-1', 'act-2', 'restart');
            INSERT INTO predictions (device_id, edge_id, prediction_type, probability, created_at)
                VALUES ('device-1', 'edge-1', 'overheating', 0.9, '2024-03-01 09:00:00');
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let report = migrate(&pool).await.unwrap();
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, SCHEMA_VERSION);

        // Old row preserved with string id and new columns present
        let row: (
            String,
            Option<String>,
            Option<String>,
            i64,
            String,
            String,
            String,
        ) = sqlx::query_as(
            "SELECT id, title, metric_name, notification_sent, status, triggered_at, created_at
                 FROM alerts WHERE rule_name = 'high_temp'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "1");
        assert!(row.1.is_none());
        assert_eq!(row.3, 0);
        assert_eq!(row.4, "firing", "unresolved pending -> firing");
        assert_eq!(row.5, "2024-03-01T12:34:56+00:00");
        assert_eq!(row.6, "2024-03-01T12:34:56+00:00");
        assert!(chrono::DateTime::parse_from_rfc3339(&row.5).is_ok());

        let (status, resolved_at): (String, Option<String>) =
            sqlx::query_as("SELECT status, resolved_at FROM alerts WHERE rule_name = 'low_temp'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "resolved");
        assert_eq!(resolved_at.as_deref(), Some("2024-03-01T11:00:00+00:00"));

        let (status,): (String,) =
            sqlx::query_as("SELECT status FROM alerts WHERE rule_name = 'acked'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "acknowledged");

        // Action history: integer alert id mapped to the string id,
        // dangling reference nulled, timestamps RFC3339
        let actions: Vec<(String, Option<String>, String)> = sqlx::query_as(
            "SELECT action_id, alert_id, executed_at FROM action_history ORDER BY action_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(actions[0].1.as_deref(), Some("2"));
        assert_eq!(actions[0].2, "2024-03-01T10:30:00+00:00");
        assert_eq!(actions[1].1, None);
        assert!(chrono::DateTime::parse_from_rfc3339(&actions[1].2).is_ok());

        // Legacy predictions FK dropped, timestamp normalized
        assert!(!table_sql(&pool, "predictions").await.contains("REFERENCES"));
        let (created,): (String,) = sqlx::query_as("SELECT created_at FROM predictions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(created, "2024-03-01T09:00:00+00:00");

        assert_eq!(version_of(&pool).await, SCHEMA_VERSION);
        assert!(report.foreign_key_violations.is_empty());

        // New inserts use TEXT ids and lifecycle columns
        sqlx::query("INSERT INTO alerts (id, rule_name, severity, message, status, triggered_at) VALUES ('01HTEST', 'r2', 'info', 'm', 'firing', '2024-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_unversioned_new_shape_db_is_not_destroyed() {
        // user_version 0 but alerts already in the new shape (e.g. created
        // by SCHEMA without stamping): data in new columns must survive.
        let pool = raw_memory_pool().await;
        sqlx::raw_sql(SCHEMA_V1).execute(&pool).await.unwrap();
        sqlx::raw_sql(
            r#"
            INSERT INTO edge_nodes (id, name) VALUES ('e', 'E');
            INSERT INTO devices (id, edge_id, device_name, device_type) VALUES ('d', 'e', 'D', 'daq');
            INSERT INTO alerts (id, device_id, rule_name, severity, message, title, metric_name,
                                metric_value, fired_count, notification_sent, triggered_at, status)
                VALUES ('01KEEP', 'd', 'r', 'warning', 'm', 'Kept title', 'temperature',
                        81.5, 7, 1, '2024-05-01T00:00:00+00:00', 'acknowledged');
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(version_of(&pool).await, 0);

        init_database(&pool).await.unwrap();

        let row: (String, String, f64, i64, i64, String) = sqlx::query_as(
            "SELECT title, metric_name, metric_value, fired_count, notification_sent, status
             FROM alerts WHERE id = '01KEEP'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "Kept title");
        assert_eq!(row.1, "temperature");
        assert_eq!(row.2, 81.5);
        assert_eq!(row.3, 7);
        assert_eq!(row.4, 1);
        assert_eq!(row.5, "acknowledged");
        assert_eq!(version_of(&pool).await, SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn test_migrates_v1_database_with_data() {
        // A database written by the previous release (user_version = 1)
        let pool = raw_memory_pool().await;
        sqlx::raw_sql(SCHEMA_V1).execute(&pool).await.unwrap();
        sqlx::raw_sql(
            r#"
            PRAGMA user_version = 1;
            INSERT INTO edge_nodes (id, name) VALUES ('e', 'E');
            INSERT INTO devices (id, edge_id, device_name, device_type) VALUES ('d', 'e', 'D', 'daq');
            INSERT INTO alerts (id, device_id, rule_name, severity, message, status, triggered_at, created_at, resolved_at)
                VALUES ('01OLD', 'd', 'r', 'warning', 'm', 'resolved', '2024-01-01T00:00:00+00:00',
                        '2024-01-01 00:00:00', '2024-01-02T00:00:00+00:00');
            INSERT INTO alerts (id, device_id, rule_name, severity, message, status, created_at)
                VALUES ('01NOTRIG', 'd', 'r', 'warning', 'm', 'firing', '2024-02-01 08:00:00');
            INSERT INTO action_history (alert_id, device_id, action_id, action_type, executed_at)
                VALUES ('01OLD', 'd', 'act-1', 'restart', '2024-01-01 00:05:00');
            INSERT INTO device_metrics_history (device_id, timestamp, metric_name, metric_value)
                VALUES ('d', '2024-01-01T00:00:00+00:00', 't', 40.0);
            INSERT INTO predictions (device_id, edge_id, prediction_type, probability, created_at)
                VALUES ('d', 'e', 'overheating', 0.9, '2024-01-01T00:00:00+00:00');
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let report = migrate(&pool).await.unwrap();
        assert_eq!(report.from_version, 1);
        assert_eq!(report.to_version, SCHEMA_VERSION);
        assert!(report.foreign_key_violations.is_empty());

        assert!(table_sql(&pool, "action_history")
            .await
            .contains("ON DELETE SET NULL"));
        let (alert_id, executed_at): (Option<String>, String) =
            sqlx::query_as("SELECT alert_id, executed_at FROM action_history")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(alert_id.as_deref(), Some("01OLD"));
        assert_eq!(executed_at, "2024-01-01T00:05:00+00:00");

        let (created, last_fired): (String, Option<String>) =
            sqlx::query_as("SELECT created_at, last_fired_at FROM alerts WHERE id = '01OLD'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(created, "2024-01-01T00:00:00+00:00");
        assert_eq!(last_fired.as_deref(), Some("2024-01-01T00:00:00+00:00"));

        let (trig,): (String,) =
            sqlx::query_as("SELECT triggered_at FROM alerts WHERE id = '01NOTRIG'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(trig, "2024-02-01T08:00:00+00:00");

        // Deleting the referenced alert now nulls the action reference
        // instead of failing with an FK violation
        sqlx::query("DELETE FROM alerts WHERE id = '01OLD'")
            .execute(&pool)
            .await
            .unwrap();
        let (alert_id,): (Option<String>,) = sqlx::query_as("SELECT alert_id FROM action_history")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(alert_id, None);

        // Row counts preserved
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM device_metrics_history")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM predictions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn test_step_1_to_2_is_idempotent() {
        // Re-running step 2 on an already-v2 database (e.g. user_version
        // reset by a tool) must succeed and keep data.
        let pool = create_test_db().await;
        sqlx::raw_sql(
            r#"
            INSERT INTO edge_nodes (id, name) VALUES ('e', 'E');
            INSERT INTO devices (id, edge_id, device_name, device_type) VALUES ('d', 'e', 'D', 'daq');
            INSERT INTO alerts (id, device_id, rule_name, severity, message, triggered_at)
                VALUES ('01A', 'd', 'r', 'info', 'm', '2024-01-01T00:00:00+00:00');
            INSERT INTO action_history (alert_id, device_id, action_id, action_type)
                VALUES ('01A', 'd', 'act', 'restart');
            PRAGMA user_version = 1;
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        init_database(&pool).await.unwrap();
        assert_eq!(version_of(&pool).await, SCHEMA_VERSION);
        let (n,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM action_history WHERE alert_id = '01A'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn test_rebuild_keeps_autoincrement_high_water_mark() {
        // Retention pruned every action row; a re-run of step 2 must not
        // restart ids at 1 and reuse ids already shown to users.
        let pool = create_test_db().await;
        sqlx::raw_sql(
            r#"
            INSERT INTO edge_nodes (id, name) VALUES ('e', 'E');
            INSERT INTO devices (id, edge_id, device_name, device_type) VALUES ('d', 'e', 'D', 'daq');
            INSERT INTO action_history (device_id, action_id, action_type) VALUES ('d', 'a1', 'x');
            INSERT INTO action_history (device_id, action_id, action_type) VALUES ('d', 'a2', 'x');
            INSERT INTO action_history (device_id, action_id, action_type) VALUES ('d', 'a3', 'x');
            DELETE FROM action_history;
            PRAGMA user_version = 1;
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        init_database(&pool).await.unwrap();
        sqlx::query("INSERT INTO action_history (device_id, action_id, action_type) VALUES ('d', 'a4', 'x')")
            .execute(&pool)
            .await
            .unwrap();
        let (id,): (i64,) = sqlx::query_as("SELECT id FROM action_history WHERE action_id = 'a4'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(id, 4);
    }

    #[tokio::test]
    async fn test_recovers_from_partial_migration() {
        // Reproduce a half-finished (pre-transactional) migration: `alerts`
        // is gone, the `alerts_new` target table is left behind, user_version is 0.
        let pool = raw_memory_pool().await;
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

        let (rule, trig): (String, Option<String>) =
            sqlx::query_as("SELECT rule_name, triggered_at FROM alerts WHERE id = '01SAVED'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(rule, "kept_rule");
        // triggered_at backfilled from created_at (RFC3339)
        assert!(chrono::DateTime::parse_from_rfc3339(&trig.unwrap()).is_ok());
        let mut c = pool.acquire().await.unwrap();
        assert!(!table_exists(&mut c, "alerts_new").await.unwrap());
        drop(c);
        assert_eq!(version_of(&pool).await, SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn test_failed_migration_rolls_back() {
        // A v1-stamped database whose action_history lacks a required
        // column makes step 2 fail: nothing may change.
        let pool = raw_memory_pool().await;
        sqlx::raw_sql(
            r#"
            CREATE TABLE alerts (id TEXT PRIMARY KEY, created_at TEXT, triggered_at TEXT,
                resolved_at TEXT, status TEXT);
            CREATE TABLE predictions (id INTEGER PRIMARY KEY, created_at TEXT, resolved_at TEXT);
            CREATE TABLE device_metrics_history (id INTEGER PRIMARY KEY, timestamp TEXT);
            CREATE TABLE action_history (id INTEGER PRIMARY KEY, executed_at TEXT);
            INSERT INTO alerts (id, created_at) VALUES ('a', '2024-01-01 00:00:00');
            PRAGMA user_version = 1;
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        assert!(init_database(&pool).await.is_err());
        assert_eq!(version_of(&pool).await, 1);
        let (created,): (String,) = sqlx::query_as("SELECT created_at FROM alerts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(created, "2024-01-01 00:00:00", "normalization rolled back");
        // FK enforcement restored on the connection
        let (fk,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[tokio::test]
    async fn test_reports_foreign_key_violations() {
        let pool = raw_memory_pool().await;
        sqlx::raw_sql(SCHEMA_V1).execute(&pool).await.unwrap();
        sqlx::raw_sql(
            r#"
            PRAGMA foreign_keys = OFF;
            INSERT INTO device_status (device_id, status) VALUES ('ghost', 'healthy');
            PRAGMA user_version = 1;
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let report = migrate(&pool).await.unwrap();
        assert_eq!(report.foreign_key_violations.len(), 1);
        assert_eq!(report.foreign_key_violations[0].table, "device_status");
        assert_eq!(report.foreign_key_violations[0].parent, "devices");
    }

    #[tokio::test]
    async fn test_reinit_is_idempotent() {
        let pool = create_test_db().await;
        init_database(&pool).await.unwrap();
        assert_eq!(version_of(&pool).await, SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn test_newer_database_is_left_alone() {
        let pool = raw_memory_pool().await;
        sqlx::raw_sql("PRAGMA user_version = 99")
            .execute(&pool)
            .await
            .unwrap();
        let report = migrate(&pool).await.unwrap();
        assert_eq!(report.from_version, 99);
        assert_eq!(report.to_version, 99);
    }
}
