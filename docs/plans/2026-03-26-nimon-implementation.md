# NIMon Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a Rust-based hardware monitoring platform for National Instruments devices with predictive maintenance and self-healing capabilities.

**Architecture:** Actor-based system using Actix for concurrent device monitoring, WebSocket for real-time edge-to-hub communication, Leptos for web dashboard, SQLite for persistence, and FFI bindings for NI API integration.

**Tech Stack:** Rust, Tokio, Actix, Axum, Leptos, SQLx, SQLite, libloading (FFI), chrono, serde, tracing

---

## Phase 1: Foundation & Core Monitoring

### Task 1: Project Setup

**Files:**
- Create: `Cargo.toml`
- Create: `.gitignore`
- Create: `src/lib.rs`
- Create: `src/main.rs`

**Step 1: Initialize Cargo project**

Run: `cargo init --name nimon`

Expected: Creates Cargo.toml, src/main.rs

**Step 2: Update Cargo.toml with workspace structure**

```toml
[workspace]
members = [
    "crates/nimon-core",
    "crates/nimon-edge",
    "crates/nimon-hub",
    "crates/nimon-cli",
    "crates/nimon-ni",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
authors = ["NIMon Team"]
license = "MIT"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
actix = "0.13"
actix-rt = "2"
actix-web = "4"
actix-ws = "0.6"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.7", features = ["runtime-tokio", "sqlite"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
libloading = "0.8"
async-trait = "0.1"
```

**Step 3: Create workspace crates**

Run:
```bash
mkdir -p crates/nimon-core/src
mkdir -p crates/nimon-edge/src
mkdir -p crates/nimon-hub/src
mkdir -p crates/nimon-cli/src
mkdir -p crates/nimon-ni/src
```

**Step 4: Create .gitignore**

```
/target
/Cargo.lock
*.db
*.db-journal
*.log
/data
.env
```

**Step 5: Commit**

```bash
git add .
git commit -m "chore: initialize workspace structure"
```

---

### Task 2: Core Crate - Types & Error Handling

**Files:**
- Create: `crates/nimon-core/Cargo.toml`
- Create: `crates/nimon-core/src/lib.rs`
- Create: `crates/nimon-core/src/error.rs`
- Create: `crates/nimon-core/src/types.rs`
- Test: `crates/nimon-core/src/error.rs` (inline tests)

**Step 1: Write Cargo.toml for nimon-core**

```toml
[package]
name = "nimon-core"
version.workspace = true
edition.workspace = true

[dependencies]
serde.workspace = true
chrono.workspace = true
thiserror.workspace = true
```

**Step 2: Write failing test for error types**

Create `crates/nimon-core/src/error.rs`:

```rust
//! Error types for NIMon

use thiserror::Error;

#[derive(Error, Debug)]
pub enum NimonError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("NI API error in {api}: status {code}")]
    NiApi { api: &'static str, code: i32 },

    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    #[error("Edge node not found: {0}")]
    EdgeNotFound(String),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Timeout waiting for {0}")]
    Timeout(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type NimonResult<T> = Result<T, NimonError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = NimonError::DeviceNotFound("PXI1Slot2".to_string());
        assert!(err.to_string().contains("PXI1Slot2"));
    }

    #[test]
    fn test_ni_api_error() {
        let err = NimonError::NiApi {
            api: "NiSysCfg_Initialize",
            code: -1,
        };
        let msg = err.to_string();
        assert!(msg.contains("NiSysCfg_Initialize"));
        assert!(msg.contains("-1"));
    }
}
```

**Step 3: Run tests to verify they pass**

Run: `cargo test -p nimon-core`

Expected: 2 tests pass

**Step 4: Write core types**

Create `crates/nimon-core/src/types.rs`:

```rust
//! Core type definitions for NIMon

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Device types supported by NIMon
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    Daq,
    Pxi,
    CDaq,
    Visa,
    Xnet,
    Gpib,
    PowerSupply,
}

impl std::fmt::Display for DeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceType::Daq => write!(f, "daq"),
            DeviceType::Pxi => write!(f, "pxi"),
            DeviceType::CDaq => write!(f, "cdaq"),
            DeviceType::Visa => write!(f, "visa"),
            DeviceType::Xnet => write!(f, "xnet"),
            DeviceType::Gpib => write!(f, "gpib"),
            DeviceType::PowerSupply => write!(f, "power_supply"),
        }
    }
}

/// Health status of a device
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Warning,
    Error,
    Offline,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Warning => write!(f, "warning"),
            HealthStatus::Error => write!(f, "error"),
            HealthStatus::Offline => write!(f, "offline"),
        }
    }
}

/// Edge node status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeStatus {
    Online,
    Offline,
    Degraded,
}

/// Alert severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

/// Metric value types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetricValue {
    Float(f64),
    Integer(i64),
    String(String),
    Boolean(bool),
}

/// A device discovered or managed by NIMon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub edge_id: String,
    pub device_name: String,
    pub device_type: DeviceType,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub firmware_version: Option<String>,
    pub driver_version: Option<String>,
    pub ip_address: Option<String>,
    pub slot: Option<i32>,
    pub chassis: Option<String>,
}

/// Current status of a device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub device_id: String,
    pub status: HealthStatus,
    pub last_poll: DateTime<Utc>,
    pub metrics: HashMap<String, MetricValue>,
    pub error_message: Option<String>,
    pub error_count: i32,
    pub uptime_seconds: i64,
}

/// An edge node in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeNode {
    pub id: String,
    pub name: String,
    pub hostname: Option<String>,
    pub ip_address: Option<String>,
    pub last_seen: Option<DateTime<Utc>>,
    pub status: EdgeStatus,
}

/// A metric data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub device_id: String,
    pub timestamp: DateTime<Utc>,
    pub metric_name: String,
    pub metric_value: f64,
}

/// Prediction types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredictionType {
    Overheating,
    ConnectionFailure,
    PowerSupplyFailure,
    BusDegradation,
    FirmwareIssue,
    Custom(String),
}

/// A prediction generated by the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prediction {
    pub id: i64,
    pub device_id: String,
    pub edge_id: String,
    pub prediction_type: PredictionType,
    pub probability: f64,
    pub eta_minutes: Option<i32>,
    pub status: PredictionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PredictionStatus {
    Active,
    Confirmed,
    Dismissed,
}

/// An alert in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: i64,
    pub device_id: Option<String>,
    pub edge_id: Option<String>,
    pub rule_name: String,
    pub severity: Severity,
    pub message: String,
    pub status: AlertStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertStatus {
    Pending,
    Sent,
    Acknowledged,
    Resolved,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_type_serde() {
        let dt = DeviceType::Daq;
        let json = serde_json::to_string(&dt).unwrap();
        assert_eq!(json, "\"daq\"");
        let parsed: DeviceType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, dt);
    }

    #[test]
    fn test_health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(HealthStatus::Warning.to_string(), "warning");
    }
}
```

**Step 5: Create lib.rs to export modules**

Create `crates/nimon-core/src/lib.rs`:

```rust
//! NIMon Core - Shared types and utilities

pub mod error;
pub mod types;

pub use error::{NimonError, NimonResult};
pub use types::*;
```

**Step 6: Run all tests**

Run: `cargo test -p nimon-core`

Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/nimon-core/
git commit -m "feat(core): add core types and error handling"
```

---

### Task 3: Core Crate - Database Layer

**Files:**
- Create: `crates/nimon-core/src/db/mod.rs`
- Create: `crates/nimon-core/src/db/schema.rs`
- Create: `crates/nimon-core/src/db/edge_repo.rs`
- Create: `crates/nimon-core/src/db/device_repo.rs`
- Test: Inline tests in repo files

**Step 1: Add sqlx dependency to nimon-core**

Update `crates/nimon-core/Cargo.toml`:

```toml
[package]
name = "nimon-core"
version.workspace = true
edition.workspace = true

[dependencies]
serde.workspace = true
chrono.workspace = true
thiserror.workspace = true
sqlx.workspace = true
async-trait.workspace = true

[dev-dependencies]
tokio-test = "0.4"
```

**Step 2: Write database schema**

Create `crates/nimon-core/src/db/schema.rs`:

```rust
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
```

**Step 3: Write database module**

Create `crates/nimon-core/src/db/mod.rs`:

```rust
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
```

**Step 4: Write edge repository**

Create `crates/nimon-core/src/db/edge_repo.rs`:

```rust
//! Repository for edge node operations

use sqlx::SqlitePool;
use chrono::{DateTime, Utc};
use crate::{EdgeNode, EdgeStatus, NimonResult, NimonError};

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
            "#
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
        let row: (String, String, Option<String>, Option<String>, Option<String>, String) =
            sqlx::query_as(
                "SELECT id, name, hostname, ip_address, last_seen, status FROM edge_nodes WHERE id = ?"
            )
            .bind(id)
            .fetch_one(self.pool)
            .await
            .map_err(|_| NimonError::EdgeNotFound(id.to_string()))?;

        Ok(EdgeNode {
            id: row.0,
            name: row.1,
            hostname: row.2,
            ip_address: row.3,
            last_seen: row.4.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc))),
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

        Ok(rows.into_iter().map(|row| EdgeNode {
            id: row.0,
            name: row.1,
            hostname: row.2,
            ip_address: row.3,
            last_seen: row.4.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc))),
            status: parse_edge_status(&row.5),
        }).collect())
    }

    pub async fn update_status(&self, id: &str, status: EdgeStatus) -> NimonResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE edge_nodes SET status = ?, last_seen = ? WHERE id = ?"
        )
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
            }).await.unwrap();
        }

        let edges = repo.list().await.unwrap();
        assert_eq!(edges.len(), 3);
    }
}
```

**Step 5: Update lib.rs to export db module**

Update `crates/nimon-core/src/lib.rs`:

```rust
//! NIMon Core - Shared types and utilities

pub mod db;
pub mod error;
pub mod types;

pub use error::{NimonError, NimonResult};
pub use types::*;
```

**Step 6: Run tests**

Run: `cargo test -p nimon-core`

Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/nimon-core/
git commit -m "feat(core): add database layer with repositories"
```

---

### Task 4: NI Crate - Common FFI Infrastructure

**Files:**
- Create: `crates/nimon-ni/Cargo.toml`
- Create: `crates/nimon-ni/src/lib.rs`
- Create: `crates/nimon-ni/src/common.rs`
- Test: Inline tests

**Step 1: Write Cargo.toml**

Create `crates/nimon-ni/Cargo.toml`:

```toml
[package]
name = "nimon-ni"
version.workspace = true
edition.workspace = true

[dependencies]
nimon-core = { path = "../nimon-core" }
libloading.workspace = true
thiserror.workspace = true
tracing.workspace = true
```

**Step 2: Write common FFI utilities**

Create `crates/nimon-ni/src/common.rs`:

```rust
//! Common utilities for NI API FFI bindings

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

/// Convert a C string pointer to a Rust String
///
/// # Safety
/// The pointer must be valid and point to a null-terminated string
pub unsafe fn c_str_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    CStr::from_ptr(ptr)
        .to_str()
        .ok()
        .map(|s| s.to_owned())
}

/// Convert a Rust string to a C string
pub fn string_to_c_string(s: &str) -> Option<CString> {
    CString::new(s).ok()
}

/// Check NI API status code and convert to Result
pub fn check_status(api_name: &'static str, status: i32) -> crate::NimonResult<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(crate::NimonError::NiApi {
            api: api_name,
            code: status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_string_to_c_string() {
        let cstr = string_to_c_string("hello");
        assert!(cstr.is_some());
        assert_eq!(cstr.unwrap().as_bytes(), b"hello");
    }

    #[test]
    fn test_check_status_success() {
        let result = check_status("Test", 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_status_error() {
        let result = check_status("Test", -1);
        assert!(result.is_err());
    }
}
```

**Step 3: Write lib.rs**

Create `crates/nimon-ni/src/lib.rs`:

```rust
//! NI API bindings for Rust
//!
//! This crate provides safe Rust wrappers around NI's C APIs for hardware monitoring.

pub mod common;

// Re-export core types
pub use nimon_core::{NimonError, NimonResult};
```

**Step 4: Run tests**

Run: `cargo test -p nimon-ni`

Expected: 3 tests pass

**Step 5: Commit**

```bash
git add crates/nimon-ni/
git commit -m "feat(ni): add common FFI utilities"
```

---

### Task 5: NI Crate - NI-SysCfg Bindings

**Files:**
- Create: `crates/nimon-ni/src/syscfg/mod.rs`
- Create: `crates/nimon-ni/src/syscfg/ffi.rs`
- Create: `crates/nimon-ni/src/syscfg/safe.rs`
- Create: `crates/nimon-ni/src/syscfg/types.rs`
- Test: Inline tests

**Step 1: Write FFI type definitions**

Create `crates/nimon-ni/src/syscfg/ffi.rs`:

```rust
//! Raw FFI bindings to NI System Configuration API

use std::os::raw::{c_char, c_int, c_void};

/// Search mode for FindHardware
pub const NISYSCFG_SIMPLE_SEARCH: c_int = 0;

/// Property IDs for GetResourceProperty
pub mod properties {
    use std::os::raw::c_int;

    pub const PRODUCT_NAME: c_int = 0;
    pub const SERIAL_NUMBER: c_int = 1;
    pub const IPADDRESS: c_int = 3;
    pub const IS_REACHABLE: c_int = 7;
    pub const TEMPERATURE: c_int = 100;
    pub const FIRMWARE_REVISION: c_int = 200;
    pub const DRIVER_VERSION: c_int = 201;
}

/// Opaque handle to NI-SysCfg session
#[repr(C)]
pub struct NiSysCfgSession {
    _private: [u8; 0],
}

/// Opaque handle to hardware enumeration
#[repr(C)]
pub struct NiSysCfgEnum {
    _private: [u8; 0],
}

/// Opaque handle to a hardware resource
#[repr(C)]
pub struct NiSysCfgResource {
    _private: [u8; 0],
}

/// Function signature for NiSysCfg_Initialize
pub type NiSysCfgInitialize = unsafe extern "C" fn(
    hostname: *const c_char,
    username: *const c_char,
    password: *const c_char,
    session: *mut *mut NiSysCfgSession,
) -> c_int;

/// Function signature for NiSysCfg_CloseHandle
pub type NiSysCfgCloseHandle = unsafe extern "C" fn(
    handle: *mut c_void,
) -> c_int;

/// Function signature for NiSysCfg_FindHardware
pub type NiSysCfgFindHardware = unsafe extern "C" fn(
    session: *mut NiSysCfgSession,
    mode: c_int,
    filter: *const c_char,
    enum_handle: *mut *mut NiSysCfgEnum,
) -> c_int;

/// Function signature for NiSysCfg_NextResource
pub type NiSysCfgNextResource = unsafe extern "C" fn(
    session: *mut NiSysCfgSession,
    enum_handle: *mut NiSysCfgEnum,
    resource: *mut *mut NiSysCfgResource,
) -> c_int;

/// Function signature for NiSysCfg_GetResourceProperty
pub type NiSysCfgGetResourceProperty = unsafe extern "C" fn(
    resource: *mut NiSysCfgResource,
    property_id: c_int,
    property_value: *mut c_void,
) -> c_int;
```

**Step 2: Write types**

Create `crates/nimon-ni/src/syscfg/types.rs`:

```rust
//! Types for NI-SysCfg API

use serde::{Deserialize, Serialize};

/// A device discovered by NI-SysCfg
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredDevice {
    pub product_name: String,
    pub serial_number: String,
    pub ip_address: Option<String>,
    pub is_reachable: bool,
    pub temperature: Option<f64>,
    pub firmware_version: Option<String>,
    pub driver_version: Option<String>,
}

/// Health information for a device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceHealth {
    pub is_reachable: bool,
    pub temperature: Option<f64>,
    pub self_test_passed: Option<bool>,
    pub error_message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovered_device_serde() {
        let device = DiscoveredDevice {
            product_name: "PXIe-8880".to_string(),
            serial_number: "12345678".to_string(),
            ip_address: Some("192.168.1.100".to_string()),
            is_reachable: true,
            temperature: Some(45.5),
            firmware_version: Some("1.2.3".to_string()),
            driver_version: Some("23.0.0".to_string()),
        };

        let json = serde_json::to_string(&device).unwrap();
        let parsed: DiscoveredDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.product_name, device.product_name);
    }
}
```

**Step 3: Write safe wrapper**

Create `crates/nimon-ni/src/syscfg/safe.rs`:

```rust
//! Safe Rust wrapper for NI-SysCfg API

use libloading::{Library, Symbol};
use std::ffi::CString;
use std::path::Path;

use super::ffi::*;
use super::types::*;
use crate::common::{c_str_to_string, check_status};
use crate::{NimonError, NimonResult};

/// NI System Configuration API wrapper
pub struct NiSysCfg {
    #[allow(dead_code)]
    library: Library,
    initialize: Symbol<'static, NiSysCfgInitialize>,
    close_handle: Symbol<'static, NiSysCfgCloseHandle>,
    find_hardware: Symbol<'static, NiSysCfgFindHardware>,
    next_resource: Symbol<'static, NiSysCfgNextResource>,
    get_property: Symbol<'static, NiSysCfgGetResourceProperty>,
}

impl NiSysCfg {
    /// Load the NI-SysCfg DLL and initialize function pointers
    pub fn load() -> NimonResult<Self> {
        let dll_path = Self::find_dll()?;

        unsafe {
            let library = Library::new(&dll_path).map_err(|e| {
                NimonError::Connection(format!(
                    "Failed to load {:?}: {}",
                    dll_path, e
                ))
            })?;

            let initialize = Self::get_symbol(&library, b"NiSysCfg_Initialize")?;
            let close_handle = Self::get_symbol(&library, b"NiSysCfg_CloseHandle")?;
            let find_hardware = Self::get_symbol(&library, b"NiSysCfg_FindHardware")?;
            let next_resource = Self::get_symbol(&library, b"NiSysCfg_NextResource")?;
            let get_property = Self::get_symbol(&library, b"NiSysCfg_GetResourceProperty")?;

            Ok(Self {
                library,
                initialize,
                close_handle,
                find_hardware,
                next_resource,
                get_property,
            })
        }
    }

    unsafe fn get_symbol<T>(library: &Library, name: &[u8]) -> NimonResult<Symbol<'static, T>> {
        library
            .get(name)
            .map_err(|e| NimonError::Connection(format!("Symbol not found: {}", e)))
    }

    /// Find the NI-SysCfg DLL location
    fn find_dll() -> NimonResult<std::path::PathBuf> {
        let candidates = [
            "C:\\Windows\\System32\\niSysCfg.dll",
            "C:\\Program Files\\National Instruments\\Shared\\niSysCfg.dll",
            "C:\\Program Files (x86)\\National Instruments\\Shared\\niSysCfg.dll",
        ];

        for path in &candidates {
            if Path::new(path).exists() {
                return Ok(path.into());
            }
        }

        Err(NimonError::Connection(
            "niSysCfg.dll not found. Please install NI System Configuration.".into()
        ))
    }

    /// Check if NI-SysCfg is available
    pub fn is_available() -> bool {
        Self::find_dll().is_ok()
    }

    /// Create a new session
    pub fn create_session(&self) -> NimonResult<SysCfgSession> {
        let mut handle: *mut NiSysCfgSession = std::ptr::null_mut();

        unsafe {
            let status = (self.initialize)(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                &mut handle,
            );
            check_status("NiSysCfg_Initialize", status)?;
        }

        Ok(SysCfgSession {
            handle,
            api: self,
        })
    }

    fn close_handle_ptr(&self, handle: *mut c_void) {
        unsafe {
            (self.close_handle)(handle);
        }
    }
}

/// RAII wrapper for SysCfg session
pub struct SysCfgSession<'a> {
    handle: *mut NiSysCfgSession,
    api: &'a NiSysCfg,
}

impl<'a> SysCfgSession<'a> {
    /// Discover all NI devices on the system
    pub fn discover_devices(&self) -> NimonResult<Vec<DiscoveredDevice>> {
        let mut enum_handle: *mut NiSysCfgEnum = std::ptr::null_mut();
        let mut devices = Vec::new();

        unsafe {
            let status = (self.api.find_hardware)(
                self.handle,
                NISYSCFG_SIMPLE_SEARCH,
                std::ptr::null(),
                &mut enum_handle,
            );
            check_status("NiSysCfg_FindHardware", status)?;

            loop {
                let mut resource: *mut NiSysCfgResource = std::ptr::null_mut();
                let status = (self.api.next_resource)(
                    self.handle,
                    enum_handle,
                    &mut resource,
                );

                if status != 0 {
                    break;
                }

                let device = self.extract_device_info(resource)?;
                devices.push(device);
            }
        }

        Ok(devices)
    }

    unsafe fn extract_device_info(
        &self,
        resource: *mut NiSysCfgResource,
    ) -> NimonResult<DiscoveredDevice> {
        let mut buffer = [0i8; 512];
        let mut int_val: i32 = 0;
        let mut float_val: f64 = 0.0;

        // Product name
        (self.api.get_property)(
            resource,
            properties::PRODUCT_NAME,
            buffer.as_mut_ptr() as *mut _,
        );
        let product_name = c_str_to_string(buffer.as_ptr())
            .unwrap_or_default();

        // Serial number
        buffer.fill(0);
        (self.api.get_property)(
            resource,
            properties::SERIAL_NUMBER,
            buffer.as_mut_ptr() as *mut _,
        );
        let serial_number = c_str_to_string(buffer.as_ptr())
            .unwrap_or_default();

        // IP address
        buffer.fill(0);
        (self.api.get_property)(
            resource,
            properties::IPADDRESS,
            buffer.as_mut_ptr() as *mut _,
        );
        let ip_address = c_str_to_string(buffer.as_ptr());

        // Is reachable
        (self.api.get_property)(
            resource,
            properties::IS_REACHABLE,
            &mut int_val as *mut _ as *mut _,
        );
        let is_reachable = int_val != 0;

        // Temperature
        let temp_status = (self.api.get_property)(
            resource,
            properties::TEMPERATURE,
            &mut float_val as *mut _ as *mut _,
        );
        let temperature = if temp_status == 0 { Some(float_val) } else { None };

        // Firmware version
        buffer.fill(0);
        (self.api.get_property)(
            resource,
            properties::FIRMWARE_REVISION,
            buffer.as_mut_ptr() as *mut _,
        );
        let firmware_version = c_str_to_string(buffer.as_ptr());

        // Driver version
        buffer.fill(0);
        (self.api.get_property)(
            resource,
            properties::DRIVER_VERSION,
            buffer.as_mut_ptr() as *mut _,
        );
        let driver_version = c_str_to_string(buffer.as_ptr());

        Ok(DiscoveredDevice {
            product_name,
            serial_number,
            ip_address,
            is_reachable,
            temperature,
            firmware_version,
            driver_version,
        })
    }
}

impl<'a> Drop for SysCfgSession<'a> {
    fn drop(&mut self) {
        self.api.close_handle_ptr(self.handle as *mut _);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available() {
        // This will return false on systems without NI software installed
        let _ = NiSysCfg::is_available();
    }
}
```

**Step 4: Write module**

Create `crates/nimon-ni/src/syscfg/mod.rs`:

```rust
//! NI System Configuration API bindings

mod ffi;
mod safe;
mod types;

pub use safe::{NiSysCfg, SysCfgSession};
pub use types::{DiscoveredDevice, DeviceHealth};
```

**Step 5: Update lib.rs**

Update `crates/nimon-ni/src/lib.rs`:

```rust
//! NI API bindings for Rust
//!
//! This crate provides safe Rust wrappers around NI's C APIs for hardware monitoring.

pub mod common;
pub mod syscfg;

// Re-export core types
pub use nimon_core::{NimonError, NimonResult};
```

**Step 6: Run tests**

Run: `cargo test -p nimon-ni`

Expected: Tests pass

**Step 7: Commit**

```bash
git add crates/nimon-ni/
git commit -m "feat(ni): add NI-SysCfg FFI bindings"
```

---

### Task 6: Edge Crate - Configuration

**Files:**
- Create: `crates/nimon-edge/Cargo.toml`
- Create: `crates/nimon-edge/src/lib.rs`
- Create: `crates/nimon-edge/src/config.rs`
- Create: `config/edge.example.yaml`
- Test: Inline tests

**Step 1: Write Cargo.toml**

Create `crates/nimon-edge/Cargo.toml`:

```toml
[package]
name = "nimon-edge"
version.workspace = true
edition.workspace = true

[dependencies]
nimon-core = { path = "../nimon-core" }
nimon-ni = { path = "../nimon-ni" }
tokio.workspace = true
actix.workspace = true
actix-rt.workspace = true
serde.workspace = true
serde_yaml = "0.9"
chrono.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

**Step 2: Write configuration types**

Create `crates/nimon-edge/src/config.rs`:

```rust
//! Configuration for the edge node

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeConfig {
    pub node: NodeConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub prediction: PredictionConfig,
    #[serde(default)]
    pub buffer: BufferConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub id: String,
    pub name: String,
    pub hub_address: String,
    #[serde(default = "default_reconnect_interval")]
    pub reconnect_interval_secs: u64,
}

fn default_reconnect_interval() -> u64 { 5 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiConfig {
    #[serde(default)]
    pub syscfg: ApiSettings,
    #[serde(default)]
    pub daqmx: ApiSettings,
    #[serde(default)]
    pub visa: ApiSettings,
    #[serde(default)]
    pub xnet: ApiSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSettings {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
}

fn default_enabled() -> bool { true }
fn default_poll_interval() -> u64 { 10 }

impl Default for ApiSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_models")]
    pub models: Vec<String>,
}

fn default_models() -> Vec<String> {
    vec!["threshold".to_string(), "ewma_anomaly".to_string()]
}

impl Default for PredictionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            models: default_models(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_buffer_size")]
    pub max_size_mb: u64,
    #[serde(default = "default_buffer_path")]
    pub persist_path: String,
}

fn default_buffer_size() -> u64 { 100 }
fn default_buffer_path() -> String { "./data/buffer.db".to_string() }

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size_mb: 100,
            persist_path: default_buffer_path(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_file")]
    pub file: String,
}

fn default_log_level() -> String { "info".to_string() }
fn default_log_file() -> String { "./logs/nimon-edge.log".to_string() }

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file: default_log_file(),
        }
    }
}

impl EdgeConfig {
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }

    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::from_yaml(&content)?)
    }

    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let yaml = r#"
node:
  id: test-edge-01
  name: Test Edge
  hub_address: "192.168.1.100:8080"
"#;
        let config = EdgeConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.node.id, "test-edge-01");
        assert_eq!(config.node.reconnect_interval_secs, 5);
    }

    #[test]
    fn test_default_config() {
        let config = EdgeConfig::from_yaml("node:\n  id: x\n  name: y\n  hub_address: z:8080").unwrap();
        assert!(config.prediction.enabled);
        assert!(config.buffer.enabled);
    }
}
```

**Step 3: Write lib.rs**

Create `crates/nimon-edge/src/lib.rs`:

```rust
//! NIMon Edge Node
//!
//! Edge collector that monitors NI hardware on a single system

pub mod config;

pub use config::EdgeConfig;
```

**Step 4: Create example config**

Create `config/edge.example.yaml`:

```yaml
# NIMon Edge Node Configuration

node:
  id: "test-cell-01"
  name: "Test Cell 1"
  hub_address: "192.168.1.100:8080"
  reconnect_interval_secs: 5

api:
  syscfg:
    enabled: true
    poll_interval_secs: 10
  daqmx:
    enabled: true
    poll_interval_secs: 5
  visa:
    enabled: true
    poll_interval_secs: 15
  xnet:
    enabled: true
    poll_interval_secs: 2

prediction:
  enabled: true
  models:
    - threshold
    - ewma_anomaly
    - trend_prediction

buffer:
  enabled: true
  max_size_mb: 100
  persist_path: "./data/buffer.db"

logging:
  level: info
  file: "./logs/nimon-edge.log"
```

**Step 5: Run tests**

Run: `cargo test -p nimon-edge`

Expected: 2 tests pass

**Step 6: Commit**

```bash
git add crates/nimon-edge/ config/
git commit -m "feat(edge): add configuration module"
```

---

## Phase 1 Summary

After completing Phase 1, you will have:

1. ✅ Workspace structure with 5 crates
2. ✅ Core types and error handling
3. ✅ Database layer with SQLite
4. ✅ NI-SysCfg FFI bindings
5. ✅ Edge node configuration

**Next phases would include:**
- Phase 2: Edge actors, WebSocket client, Hub server
- Phase 3: Prediction engine, Alerting, Self-healing
- Phase 4: Additional NI APIs (DAQmx, VISA, XNET)
- Phase 5: Web dashboard, CLI, deployment

---

## Running the Full Test Suite

After all Phase 1 tasks:

```bash
cargo test --workspace
```

Expected: All tests pass

---

## Building

```bash
cargo build --workspace
```

---

## Verification Checklist

- [ ] All workspace crates compile
- [ ] All tests pass
- [ ] Example config file exists
- [ ] NI-SysCfg integration ready for testing on Windows with NI software
