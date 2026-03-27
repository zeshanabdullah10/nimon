# NIMon Phase 6: Persistence, Wiring & Observability

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire the database persistence layer into the hub server, fix stub REST endpoints, and make the system observable. The hub currently has complete DB schemas and repositories in `nimon-core` but never calls them.

**Architecture:** The hub receives `DeviceStatusUpdate`, `PredictionResult`, and `EdgeRegister` messages via WebSocket but discards them instead of persisting. The REST API returns empty stubs for predictions and historical data.

---

## Task 1: Wire Device Persistence

AlertManager receives `DeviceStatusUpdate` and `PredictionResult` but never writes to the database. Wire the existing `DeviceRepository` and `PredictionRepository` calls.

**Files:**
- Modify: `crates/nimon-hub/src/alert/manager.rs`

**Step 1: Add database pool to AlertManager**

AlertManager already has `db_pool: Option<SqlitePool>`. Ensure the `with_db_pool()` method is called in `server/mod.rs:run()` (it already is).

**Step 2: Persist DeviceStatusUpdate on receipt**

In `AlertManager::receive()`, after the `DeviceStatusUpdate` is processed and before the return, persist to the database:

```rust
// After handling DeviceStatusUpdate (around line 276 in manager.rs)
if let Some(ref pool) = self.db_pool {
    if let Err(e) = pool.execute(
        sqlx::query(
            "INSERT INTO device_status (device_id, edge_id, status, temperature, voltage_5v, voltage_3v3, timestamp)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(device_id) DO UPDATE SET
               status = excluded.status, temperature = excluded.temperature,
               voltage_5v = excluded.voltage_5v, voltage_3v3 = excluded.voltage_3v3,
               timestamp = excluded.timestamp"
        )
        .bind(&status.device_id)
        .bind(&status.edge_id)
        .bind(format!("{:?}", status.status))
        .bind(status.temperature)
        .bind(status.voltage_5v)
        .bind(status.voltage_3v3)
        .bind(chrono::Utc::now())
    ).await {
        error!("Failed to persist device status: {}", e);
    }
}
```

Alternatively, use the existing `DeviceRepository::upsert_status()` method.

**Step 3: Persist PredictionResult on receipt**

In `AlertManager::receive()`, after handling `PredictionResult`:

```rust
if let Some(ref pool) = self.db_pool {
    if let Err(e) = pool.execute(
        sqlx::query(
            "INSERT INTO predictions (device_id, edge_id, model_name, prediction_type, probability, severity, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&pred.device_id)
        .bind(&pred.edge_id)
        .bind(&pred.model_name)
        .bind(format!("{:?}", pred.prediction_type))
        .bind(pred.probability)
        .bind(format!("{:?}", pred.severity))
        .bind(chrono::Utc::now())
    ).await {
        error!("Failed to persist prediction: {}", e);
    }
}
```

**Step 4: Run tests**
```
cargo test --workspace
```
Expected: All tests pass.

---

## Task 2: Wire Edge Persistence

When an edge registers via WebSocket, upsert to the `edge_nodes` table. When it heartbeats, update `last_seen`.

**Files:**
- Modify: `crates/nimon-hub/src/server/mod.rs` (ws_socket_handler)
- Modify: `crates/nimon-hub/src/server/routes.rs` (optional cleanup)

**Step 1: Persist edge registration**

In `ws_socket_handler` (around the `EdgeRegister` case), after creating the `EdgeSession`, also upsert to the database:

```rust
// After state.sessions().add(session);
if let Some(ref pool) = *self.db_pool.lock().await {
    if let Err(e) = edge_nodes::upsert(pool, &reg.edge_id, &reg.name, &reg.hostname).await {
        error!("Failed to upsert edge node: {}", e);
    }
}
```

The `EdgeRepository::upsert()` method already exists in `nimon-core/src/db/edge_repo.rs`.

**Step 2: Update last_seen on heartbeat**

In the `Heartbeat` case, also update the `edge_nodes.last_seen` timestamp:

```rust
nimon_core::db::edge_repo::update_last_seen(pool, edge_id).await;
```

**Step 3: Remove EdgeSession from DB on disconnect**

In `ws_socket_handler`, on disconnect (at the end of the handler), optionally mark the edge as offline rather than just removing the in-memory session:

```rust
// Instead of just removing from sessions:
// state.sessions().remove(eid);
// Mark as offline in DB instead:
if let Some(ref pool) = *self.db_pool.lock().await {
    let _ = edge_nodes::update_status(pool, edge_id, "offline").await;
}
```

**Step 4: Run tests**
```
cargo test --workspace
```
Expected: All tests pass.

---

## Task 3: Fix Stub REST Endpoints

The `get_predictions_handler` returns an empty array. `get_edge_devices` returns an empty array for all edges.

**Files:**
- Modify: `crates/nimon-hub/src/server/mod.rs`
- Modify: `crates/nimon-hub/src/server/routes.rs`

**Step 1: Fix get_predictions_handler to query DB**

Replace the stub implementation in `mod.rs`:

```rust
async fn get_predictions_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
) -> impl IntoResponse {
    match state.alert_manager().await {
        Some(addr) => {
            match addr.send(GetRecentPredictions { limit: 100 }).await {
                Ok(Ok(predictions)) => {
                    let total = predictions.len();
                    Json(PredictionsResponse { predictions, total }).into_response()
                }
                Ok(Err(e)) => {
                    error!("Failed to get predictions: {}", e);
                    Json(PredictionsResponse { predictions: vec![], total: 0 }).into_response()
                }
                Err(e) => {
                    error!("AlertManager unavailable: {}", e);
                    Json(PredictionsResponse { predictions: vec![], total: 0 }).into_response()
                }
            }
        }
        None => Json(PredictionsResponse { predictions: vec![], total: 0 }).into_response(),
    }
}
```

Add a `GetRecentPredictions` message to `AlertManager`:

```rust
pub struct GetRecentPredictions {
    pub limit: usize,
}

impl Handler<GetRecentPredictions> for AlertManager {
    type Result = ResponseActFuture<Self, Result<Vec<serde_json::Value>, String>>;

    fn handle(&mut self, msg: GetRecentPredictions, _ctx: &mut Self::Context) -> Self::Result {
        let pool = self.db_pool.clone();
        let fut = async move {
            let pool = pool.ok_or("No database pool")?;
            let predictions = nimon_core::db::prediction_repo::list_recent(&pool, msg.limit)
                .await
                .map_err(|e| e.to_string())?;
            Ok(predictions)
        }.into_actor(self);
        Box::pin(fut)
    }
}
```

Add `GetRecentPredictions` to the `Setup` message enum.

**Step 2: Fix get_edge_devices to query devices table**

Update `routes.rs:get_edge_devices` to actually query the `devices` table for the edge's devices. First check if the edge session exists (return 404 if not), then query `devices WHERE edge_id = ?`.

**Step 3: Add historical alerts endpoint**

Add `GET /api/v1/alerts/history` that queries the `alerts` table with optional filters:
- `?severity=critical|warning|info`
- `?edge_id=...`
- `?device_id=...`
- `?since=ISO8601`
- `?limit=100`

**Step 4: Run tests**
```
cargo test --workspace
```
Expected: All tests pass.

---

## Task 4: Notification Channel Wiring

The `AlertNotifier` and `ChannelType` (Email, Slack, Teams) exist in `crates/nimon-hub/src/alert/notifier.rs` but are never started. Wire them into AlertManager.

**Files:**
- Modify: `crates/nimon-hub/src/alert/manager.rs`
- Create: `crates/nimon-hub/src/alert/notifier.rs` (implement if missing)
- Modify: `crates/nimon-hub/src/config.rs` (add notification config)

**Step 1: Add notification config to HubConfig**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    #[serde(default)]
    pub slack_webhook: Option<String>,
    #[serde(default)]
    pub email: Option<EmailConfig>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub from: String,
    pub to: Vec<String>,
}
```

**Step 2: Implement AlertNotifier**

Create or complete `crates/nimon-hub/src/alert/notifier.rs`:

```rust
pub struct AlertNotifier {
    channels: Vec<ChannelType>,
}

impl AlertNotifier {
    pub fn new(config: NotificationConfig) -> Self {
        let mut channels = Vec::new();
        if let Some(webhook) = config.slack_webhook {
            channels.push(ChannelType::Slack(webhook));
        }
        if let Some(email_cfg) = config.email {
            channels.push(ChannelType::Email(email_cfg));
        }
        Self { channels }
    }

    pub async fn notify(&self, alert: &serde_json::Value) {
        for channel in &self.channels {
            if let Err(e) = channel.send(alert).await {
                error!("Failed to send notification via {:?}: {}", channel, e);
            }
        }
    }
}
```

**Step 3: Start notifier in AlertManager**

In `AlertManager::started()`, if notifications are configured:

```rust
fn started(&mut self, _ctx: &mut Self::Context) {
    // ... existing code ...
    if let Some(ref config) = self.notification_config {
        if config.enabled {
            self.notifier = Some(AlertNotifier::new(config.clone()));
            info!("AlertNotifier started");
        }
    }
}
```

**Step 4: Send notification when alert fires**

In `AlertManager::evaluate_rules()`, after creating a new alert:

```rust
if let Some(ref mut notifier) = self.notifier {
    let _ = notifier.notify(&alert_json);
}
```

**Step 5: Run tests**
```
cargo test --workspace
```
Expected: All tests pass.

---

## Task 5: Config-Driven Alert Rules

AlertManager has hardcoded rules in `default_rules()`. Move these to the YAML config file.

**Files:**
- Modify: `crates/nimon-hub/src/config.rs`
- Modify: `crates/nimon-hub/src/alert/manager.rs`

**Step 1: Add rules to AlertConfig**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    #[serde(default = "default_cooldown")]
    pub default_cooldown_minutes: i64,
    #[serde(default = "default_max_firing")]
    pub max_firing_count: i32,
    #[serde(default)]
    pub rules: Vec<RuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleConfig {
    pub name: String,
    pub condition: ConditionConfig,
    pub action: ActionConfig,
    #[serde(default = "default_severity")]
    pub severity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionConfig {
    pub metric: String,
    pub threshold: Option<f64>,
    pub comparison: Option<String>, // "gt", "lt", "eq"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionConfig {
    pub action_type: String,
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub service_name: Option<String>,
}
```

**Step 2: Use config rules in AlertManager**

In `AlertManager::new(config: AlertManagerConfig)`, replace the hardcoded `default_rules()` with rules from config:

```rust
let rules = if config.rules.is_empty() {
    Self::default_rules() // fallback to hardcoded defaults
} else {
    config.rules.iter().map(|r| r.into()).collect()
};
```

**Step 3: Update example config**

Update `config/hub.example.yaml` with the rules format.

**Step 4: Run tests**
```
cargo test --workspace
```
Expected: All tests pass.

---

## Task 6: Run Full Integration Test

After all tasks, run the full test suite and verify the hub starts correctly.

**Step 1: Start hub with test config and verify all endpoints**
```bash
cargo run -p nimon-hub --release -- config/hub.yaml
# Test all endpoints with curl
```

**Step 2: Run tests**
```bash
cargo test --workspace
```

Expected: All tests pass, 0 warnings.

**Step 3: Commit**
```bash
git add -A
git commit -m "feat(hub): wire database persistence, fix stub endpoints, add notification channels and config-driven rules"
```
