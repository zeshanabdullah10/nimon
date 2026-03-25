//! Database schema definitions

pub const SCHEMA: &str = r#"
-- Edge nodes
CREATE TABLE IF NOT EXISTS edge_nodes (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    hostname        TEXT,
    ip_address      TEXT,
    last_seen       TEXT,
    status          TEXT DEFAULT 'offline',
    config          TEXT,
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
    config          TEXT,
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
    device_id       TEXT NOT NULL REFERENCES devices(id),
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

-- Alerts
CREATE TABLE IF NOT EXISTS alerts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id       TEXT REFERENCES devices(id),
    edge_id         TEXT,
    rule_name       TEXT NOT NULL,
    severity        TEXT NOT NULL,
    message         TEXT NOT NULL,
    channels        TEXT,
    status          TEXT DEFAULT 'pending',
    action_taken    TEXT,
    action_result   TEXT,
    created_at      TEXT DEFAULT CURRENT_TIMESTAMP,
    resolved_at     TEXT
);

-- Action history
CREATE TABLE IF NOT EXISTS action_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    alert_id        INTEGER REFERENCES alerts(id),
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
