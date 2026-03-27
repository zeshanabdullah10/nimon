# NIMon Phase 5: End-to-End WebSocket Flow & API Completion

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire up the full edge-to-hub WebSocket message flow (the system's critical missing piece), complete the REST API, add hub configuration, and implement graceful shutdown.

**Architecture:** The edge WsClient's `send()` is currently a no-op. Fix it with an mpsc channel to the connection task. The hub's WebSocket handler currently echoes — replace with message parsing that dispatches to AlertManager. Wire edge actors together (DeviceActor → PredictionActor → HubConnectorActor). Add missing REST endpoints with /api/v1/ prefix.

**Tech Stack:** tokio-tungstenite (edge WS client), axum WS (hub WS server), actix (actors), serde_json (message protocol)

---

## Task 1: Fix Edge WsClient send() with mpsc Channel

The `WsClient::send()` at `crates/nimon-edge/src/comm/ws_client.rs:108-129` logs "Send not yet implemented" and drops messages. Fix this by adding an mpsc sender that bridges to the spawned connection task.

**Files:**
- Modify: `crates/nimon-edge/src/comm/ws_client.rs`
- Test: `crates/nimon-edge/src/comm/ws_client.rs` (existing test module)

**Step 1: Add send channel field to WsClient**

Add `send_tx: Option<mpsc::UnboundedSender<String>>` field to `WsClient`. In `new()`, initialize to `None`.

```rust
pub struct WsClient {
    config: WsClientConfig,
    state: Arc<tokio::sync::RwLock<ConnectionState>>,
    event_tx: mpsc::UnboundedSender<WsClientEvent>,
    message_buffer: Arc<tokio::sync::Mutex<Vec<WsMessage>>>,
    shutdown_tx: Option<mpsc::UnboundedSender<()>>,
    send_tx: Option<mpsc::UnboundedSender<String>>,
}
```

Update `new()`:
```rust
Self {
    config,
    state: Arc::new(tokio::sync::RwLock::new(ConnectionState::Disconnected)),
    event_tx,
    message_buffer: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    shutdown_tx: None,
    send_tx: None,
}
```

Update `Clone` impl to include `send_tx`:
```rust
impl Clone for WsClient {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            state: self.state.clone(),
            event_tx: self.event_tx.clone(),
            message_buffer: self.message_buffer.clone(),
            shutdown_tx: None,
            send_tx: self.send_tx.clone(),
        }
    }
}
```

**Step 2: Fix send() to use the channel**

Replace the body of `send()` (lines 108-129) with:
```rust
pub async fn send(&self, msg: WsMessage) -> Result<(), NimonError> {
    let state = self.state.read().await;
    if *state != ConnectionState::Connected {
        drop(state);
        let mut buffer = self.message_buffer.lock().await;
        if buffer.len() < self.config.buffer_size {
            buffer.push(msg);
            debug!("Message buffered (not connected)");
            return Ok(());
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Message buffer full",
            ).into());
        }
    }
    drop(state);

    let json = msg.to_json().map_err(|e| NimonError::Connection(format!("Serialization failed: {}", e)))?;
    if let Some(ref tx) = self.send_tx {
        tx.send(json).map_err(|_| NimonError::Connection("Send channel closed".to_string()))?;
        Ok(())
    } else {
        Err(NimonError::Connection("Not connected".to_string()))
    }
}
```

**Step 3: Fix connect() to create the send channel**

In `connect()`, before spawning the connection task, create an mpsc channel. Store the sender. Pass the receiver to `handle_connection`.

Replace the success branch of `connect()` (lines 140-152):
```rust
Ok((ws_stream, _)) => {
    *state.write().await = ConnectionState::Connected;
    info!("Connected to hub at {}", url);

    let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();
    self.shutdown_tx = Some(shutdown_tx);

    let (send_tx, send_rx) = mpsc::unbounded_channel::<String>();
    self.send_tx = Some(send_tx);

    tokio::spawn(async move {
        Self::handle_connection(ws_stream, &mut shutdown_rx, send_rx).await;
    });

    self.send_buffered_messages().await;
    Ok(())
}
```

**Step 4: Fix handle_connection() to accept and use the send channel**

Change the signature and add a select branch for outgoing messages:
```rust
async fn handle_connection(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    shutdown: &mut mpsc::UnboundedReceiver<()>,
    mut outgoing: mpsc::UnboundedReceiver<String>,
) {
    let (mut write, mut read) = ws_stream.split();
    let ping_interval = Duration::from_secs(30);
    let mut ping_interval_tokio = tokio::time::interval(ping_interval);

    loop {
        tokio::select! {
            _ = ping_interval_tokio.tick() => {
                let ping_msg = WsMessage::ping();
                if let Ok(json) = ping_msg.to_json() {
                    if let Err(e) = write.send(WsProtoMessage::Text(json)).await {
                        error!("Failed to send ping: {}", e);
                        break;
                    }
                }
            }
            Some(_) = shutdown.recv() => {
                info!("Connection shutdown requested");
                break;
            }
            Some(text) = outgoing.recv() => {
                if let Err(e) = write.send(WsProtoMessage::Text(text)).await {
                    error!("Failed to send message: {}", e);
                    break;
                }
            }
            Some(msg) = read.next() => {
                match msg {
                    Ok(WsProtoMessage::Text(text)) => {
                        if let Ok(ws_msg) = WsMessage::from_json(&text) {
                            debug!("Received message: {:?}", ws_msg.msg_type);
                        }
                    }
                    Ok(WsProtoMessage::Close(_)) => {
                        info!("WebSocket closed by server");
                        break;
                    }
                    Ok(WsProtoMessage::Ping(data)) => {
                        let _ = write.send(WsProtoMessage::Pong(data)).await;
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}
```

**Step 5: Fix send_buffered_messages() to actually send**

Replace the body of `send_buffered_messages()`:
```rust
async fn send_buffered_messages(&self) {
    let mut buffer = self.message_buffer.lock().await;
    if buffer.is_empty() {
        return;
    }

    info!("Sending {} buffered messages", buffer.len());
    if let Some(ref tx) = self.send_tx {
        for msg in buffer.drain(..) {
            match msg.to_json() {
                Ok(json) => {
                    if tx.send(json).is_err() {
                        warn!("Failed to send buffered message");
                        break;
                    }
                }
                Err(e) => {
                    warn!("Failed to serialize buffered message: {}", e);
                }
            }
        }
    }
}
```

**Step 6: Fix Connect handler to actually connect**

Replace the `Handler<Connect>` impl:
```rust
impl Handler<Connect> for WsClient {
    type Result = ResponseActFuture<Self, Result<(), NimonError>>;

    fn handle(&mut self, _msg: Connect, _ctx: &mut Self::Context) -> Self::Result {
        let url = self.config.hub_url.clone();
        let state = self.state.clone();
        let event_tx = self.event_tx.clone();

        let fut = async move {
            *state.write().await = ConnectionState::Connecting;

            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    *state.write().await = ConnectionState::Connected;
                    info!("Connected to hub at {}", url);
                    let _ = event_tx.send(WsClientEvent::Connected);
                    Ok(())
                }
                Err(e) => {
                    *state.write().await = ConnectionState::Disconnected;
                    let err: NimonError = std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        format!("Failed to connect: {}", e),
                    ).into();
                    let _ = event_tx.send(WsClientEvent::Error(format!("{}", err)));
                    Err(err)
                }
            }
        }
        .into_actor(self);

        Box::pin(fut)
    }
}
```

**Step 7: Run tests**

Run: `cargo test -p nimon-edge`
Expected: All 69+ tests pass.

**Step 8: Commit**

```bash
git add crates/nimon-edge/src/comm/ws_client.rs
git commit -m "fix(edge): implement WsClient send() with mpsc channel to connection task"
```

---

## Task 2: Wire Hub WebSocket Handler to Parse Messages

The hub's `ws_socket_handler` in `crates/nimon-hub/src/server/mod.rs:199-235` currently echoes messages. Replace it with actual message parsing and dispatch to AlertManager.

**Files:**
- Modify: `crates/nimon-hub/src/server/mod.rs`

**Step 1: Add WsMessage dependency to nimon-hub**

The hub needs to parse `WsMessage`. The type is defined in `nimon-edge/src/comm/message.rs` which depends on `nimon-core::actor::messages`. Since `nimon-hub` already depends on `nimon-core`, we need a shared protocol module.

Create `crates/nimon-core/src/protocol.rs` — move the `WsMessage`, `WsMessageType`, `AckMessage`, `ErrorMessage`, `PingMessage`, `PongMessage`, and `PROTOCOL_VERSION` types there. They only depend on `serde`, `chrono`, and `nimon_core::actor::messages`.

Read the existing types from `crates/nimon-edge/src/comm/message.rs:1-178` and create the same types in `crates/nimon-core/src/protocol.rs`.

**Step 2: Re-export from nimon-edge**

In `crates/nimon-edge/src/comm/message.rs`, replace the type definitions with re-exports:
```rust
pub use nimon_core::protocol::*;
```

Keep the factory methods and tests in `message.rs`.

**Step 3: Register module in nimon-core**

In `crates/nimon-core/src/lib.rs`, add `pub mod protocol;`.

**Step 4: Replace ws_socket_handler in hub server**

Replace the `ws_socket_handler` function in `crates/nimon-hub/src/server/mod.rs` (lines 199-235) with:

```rust
/// Handle WebSocket connection
async fn ws_socket_handler(socket: WebSocket, state: HubState) {
    info!("WebSocket connection established");
    let (mut sender, mut receiver) = socket.split();
    let mut edge_id: Option<String> = None;

    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(axum::extract::ws::Message::Text(text)) => {
                let ws_msg = match nimon_core::protocol::WsMessage::from_json(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("Failed to parse WS message: {}", e);
                        continue;
                    }
                };

                match ws_msg.msg_type {
                    nimon_core::protocol::WsMessageType::EdgeRegister => {
                        if let Ok(reg) = ws_msg.payload::<nimon_core::actor::messages::EdgeRegister>() {
                            edge_id = Some(reg.edge_id.clone());
                            let session = crate::session::EdgeSession::new(
                                reg.edge_id.clone(),
                                reg.name,
                                reg.hostname,
                                reg.ip_address,
                            );
                            state.sessions().add(session);
                            info!("Edge registered: {}", reg.edge_id);

                            // Send ack
                            let ack = nimon_core::protocol::WsMessage::ack(
                                ws_msg.msg_id, true, None,
                            );
                            if let Ok(json) = ack.to_json() {
                                let _ = sender.send(axum::extract::ws::Message::Text(json)).await;
                            }
                        }
                    }
                    nimon_core::protocol::WsMessageType::DeviceStatus => {
                        if let Ok(status) = ws_msg.payload::<nimon_core::actor::messages::DeviceStatusUpdate>() {
                            debug!("Device status: {} is {:?}", status.device_id, status.status);
                            if let Some(alert_manager) = state.alert_manager().await {
                                let _ = alert_manager.send(status).await;
                            }
                        }
                    }
                    nimon_core::protocol::WsMessageType::Prediction => {
                        if let Ok(pred) = ws_msg.payload::<nimon_core::actor::messages::PredictionResult>() {
                            debug!("Prediction: {:?} for {} ({:.0}%)", pred.prediction_type, pred.device_id, pred.probability * 100.0);
                            if let Some(alert_manager) = state.alert_manager().await {
                                let _ = alert_manager.send(pred).await;
                            }
                        }
                    }
                    nimon_core::protocol::WsMessageType::DeviceAlert => {
                        if let Ok(_alert) = ws_msg.payload::<nimon_core::actor::messages::DeviceAlert>() {
                            debug!("Device alert received");
                        }
                    }
                    nimon_core::protocol::WsMessageType::Heartbeat => {
                        if let Some(ref eid) = edge_id {
                            if let Some(session) = state.sessions().get(eid) {
                                session.update_heartbeat().await;
                            }
                        }
                    }
                    nimon_core::protocol::WsMessageType::Ping => {
                        let pong = nimon_core::protocol::WsMessage::pong(ws_msg.timestamp);
                        if let Ok(json) = pong.to_json() {
                            let _ = sender.send(axum::extract::ws::Message::Text(json)).await;
                        }
                    }
                    _ => {
                        debug!("Unhandled message type: {:?}", ws_msg.msg_type);
                    }
                }
            }
            Ok(axum::extract::ws::Message::Close(_)) => {
                info!("WebSocket close received");
                break;
            }
            Ok(axum::extract::ws::Message::Ping(data)) => {
                let _ = sender.send(axum::extract::ws::Message::Pong(data)).await;
            }
            Err(e) => {
                error!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    // Unregister edge on disconnect
    if let Some(ref eid) = edge_id {
        state.sessions().remove(eid);
        info!("Edge {} disconnected", eid);
    }
    info!("WebSocket connection closed");
}
```

**Step 5: Run tests**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 6: Commit**

```bash
git add crates/nimon-core/src/protocol.rs crates/nimon-core/src/lib.rs crates/nimon-edge/src/comm/message.rs crates/nimon-hub/src/server/mod.rs
git commit -m "feat(hub): parse WebSocket messages and dispatch to AlertManager"
```

---

## Task 3: Wire Edge Actors Together

The DeviceActor has a `status_recipient` field that is never set. The PredictionActor receives DeviceStatusUpdate but is never subscribed. Wire the actor graph: DeviceActor → PredictionActor → HubConnectorActor.

**Files:**
- Modify: `crates/nimon-edge/src/actor/device_manager.rs`

**Step 1: Add HubConnectorActor address to DeviceManagerActor**

Add `hub_connector: Option<Addr<HubConnectorActor>>` field and a builder method:

```rust
pub struct DeviceManagerActor {
    device_actors: HashMap<String, Addr<DeviceActor>>,
    prediction_actor: Option<Addr<PredictionActor>>,
    hub_connector: Option<Addr<HubConnectorActor>>,
    edge_id: String,
    default_poll_interval: u64,
}

impl DeviceManagerActor {
    pub fn new(edge_id: String) -> Self {
        Self {
            device_actors: HashMap::new(),
            prediction_actor: None,
            hub_connector: None,
            edge_id,
            default_poll_interval: 10,
        }
    }

    pub fn with_hub_connector(mut self, addr: Addr<HubConnectorActor>) -> Self {
        self.hub_connector = Some(addr);
        self
    }
}
```

**Step 2: Wire status_recipient when adding devices**

In `add_device()`, set the status_recipient to the PredictionActor and HubConnectorActor. Since actix `Recipient` can only target one handler, use a helper actor or send to both manually. The simplest approach: set `status_recipient` to the HubConnectorActor (so device status goes to hub), and have the PredictionActor subscribe to `DeviceStatusUpdate` by receiving forwarded messages from HubConnectorActor.

Actually, the simplest correct approach: DeviceActor sends to PredictionActor. PredictionActor forwards interesting results to HubConnectorActor. HubConnectorActor forwards everything to hub.

Change `add_device()`:
```rust
fn add_device(&mut self, device: Device) {
    let device_id = device.id.clone();

    let mut actor = DeviceActor::new(device, self.default_poll_interval);

    // Wire status updates to prediction actor
    if let Some(ref pred_addr) = self.prediction_actor {
        actor = actor.with_status_recipient(pred_addr.recipient());
    }

    let addr = actor.start();
    self.device_actors.insert(device_id.clone(), addr);
    tracing::info!("Added device: {}", device_id);
}
```

**Step 3: Wire PredictionActor output to HubConnectorActor**

The PredictionActor already emits `PredictionResult` messages. We need it to forward to the HubConnectorActor. Add a `hub_connector` field to `PredictionActor`:

In `crates/nimon-edge/src/actor/prediction_actor.rs`, add:
```rust
use super::hub_connector::HubConnectorActor;

pub struct PredictionActor {
    // ... existing fields ...
    hub_connector: Option<Recipient<nimon_core::actor::messages::PredictionResult>>,
}

impl PredictionActor {
    pub fn with_hub_connector(mut self, addr: Recipient<nimon_core::actor::messages::PredictionResult>) -> Self {
        self.hub_connector = Some(addr);
        self
    }
}
```

In the PredictionActor's handler that emits `PredictionResult`, also forward to hub_connector:
```rust
// After emitting prediction, forward to hub connector
if let Some(ref hub) = self.hub_connector {
    let _ = hub.send(prediction.clone());
}
```

**Step 4: Wire DeviceStatusUpdate to HubConnectorActor from DeviceManagerActor**

In `DeviceManagerActor::started()`, after creating the prediction actor, if a hub_connector is set, also subscribe to DeviceStatusUpdate messages by having the DeviceActor send to both prediction and hub. Since `status_recipient` is a single `Recipient`, the cleanest approach is:

Make DeviceManagerActor itself handle `DeviceStatusUpdate` and forward to HubConnectorActor:

```rust
impl Handler<DeviceStatusUpdate> for DeviceManagerActor {
    type Result = ();

    fn handle(&mut self, msg: DeviceStatusUpdate, _ctx: &mut Self::Context) -> Self::Result {
        if let Some(ref hub) = self.hub_connector {
            hub.do_send(msg);
        }
    }
}
```

Then set the DeviceActor's status_recipient to the DeviceManagerActor's recipient for `DeviceStatusUpdate`. But DeviceActor sends to PredictionActor, and PredictionActor sends PredictionResult. We also need DeviceStatusUpdate to go to hub.

Simplest solution: Use a broadcast approach. Have DeviceManagerActor subscribe to both PredictionActor's output and also forward DeviceStatusUpdate directly to HubConnectorActor.

In `started()`, after starting the prediction actor:
```rust
// Wire prediction output to hub connector
if let (Some(ref pred_addr), Some(ref hub_addr)) = (&self.prediction_actor, &self.hub_connector) {
    // PredictionActor will forward PredictionResult to HubConnectorActor
    // This is done by setting hub_connector on PredictionActor
}
```

Actually, the cleanest approach: Create the PredictionActor with the hub connector recipient BEFORE starting it:

```rust
fn start_prediction_actor(&mut self, _ctx: &mut Context<Self>) {
    let mut actor = PredictionActor::new(10).with_thresholds(65.0, 75.0);
    if let Some(ref hub) = self.hub_connector {
        actor = actor.with_hub_connector(hub.recipient());
    }
    self.prediction_actor = Some(actor.start());
    tracing::info!("Prediction actor started");
}
```

And in `add_device()`, also forward status updates to hub:
```rust
fn add_device(&mut self, device: Device) {
    let device_id = device.id.clone();
    let mut actor = DeviceActor::new(device, self.default_poll_interval);

    if let Some(ref pred_addr) = self.prediction_actor {
        actor = actor.with_status_recipient(pred_addr.recipient());
    }

    let addr = actor.start();

    // Also forward status updates directly to hub connector
    if let Some(ref hub) = self.hub_connector {
        // The hub connector needs DeviceStatusUpdate forwarded.
        // We use the DeviceManagerActor as intermediary.
    }

    self.device_actors.insert(device_id.clone(), addr);
    tracing::info!("Added device: {}", device_id);
}
```

Wait — there's a cleaner way. Make PredictionActor forward BOTH predictions AND the incoming status updates to HubConnectorActor:

In PredictionActor's `handle_status_update()`, after processing, forward the status update to hub:
```rust
fn handle_status_update(&mut self, msg: DeviceStatusUpdate) {
    // ... existing prediction logic ...

    // Forward status to hub connector
    if let Some(ref hub) = self.hub_connector {
        let _ = hub.send(msg);
    }
}
```

This way the flow is: DeviceActor → PredictionActor → HubConnectorActor → WebSocket → Hub.

**Step 5: Run tests**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 6: Commit**

```bash
git add crates/nimon-edge/src/actor/device_manager.rs crates/nimon-edge/src/actor/prediction_actor.rs
git commit -m "feat(edge): wire DeviceActor → PredictionActor → HubConnectorActor pipeline"
```

---

## Task 4: Complete REST API Endpoints

Wire up missing REST endpoints and switch to `/api/v1/` prefix. The `routes.rs` handlers exist but are dead code (not wired into the router).

**Files:**
- Modify: `crates/nimon-hub/src/server/mod.rs`
- Modify: `crates/nimon-hub/src/server/routes.rs`

**Step 1: Update route prefix to /api/v1**

In `crates/nimon-hub/src/server/mod.rs`, update the router in `run()`:
```rust
let app = Router::new()
    .route("/ws", get(ws_handler))
    .route("/health", get(health_handler))
    .route("/api/v1/status", get(status_handler))
    .route("/api/v1/alerts", get(get_alerts_handler))
    .route("/api/v1/predictions", get(get_predictions_handler))
    .route("/api/v1/edges", get(routes::list_edges))
    .route("/api/v1/edges/{edge_id}", get(routes::get_edge))
    .route("/api/v1/edges/{edge_id}/devices", get(routes::get_edge_devices))
    .with_state(state)
    .layer(CorsLayer::permissive())
    .layer(TraceLayer::new_for_http());
```

**Step 2: Add alert acknowledge endpoint**

Add a POST handler for acknowledging alerts:
```rust
/// Handler to acknowledge an alert
async fn acknowledge_alert_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
    axum::extract::Path(alert_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.alert_manager().await {
        Some(addr) => {
            match addr.send(crate::alert::manager::ResolveAlert { alert_id }).await {
                Ok(Ok(())) => Json(serde_json::json!({ "status": "resolved" })).into_response(),
                Ok(Err(e)) => (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": format!("{}", e) })),
                ).into_response(),
                Err(e) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "error": format!("{}", e) })),
                ).into_response(),
            }
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "AlertManager not available" })),
        ).into_response(),
    }
}
```

Add route: `.route("/api/v1/alerts/{alert_id}/acknowledge", post(acknowledge_alert_handler))`

**Step 3: Fix routes.rs handlers**

Update `get_edge()` in routes.rs to actually look up the session:
```rust
pub async fn get_edge(
    State(state): State<HubState>,
    axum::extract::Path(edge_id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.sessions().get(&edge_id) {
        Some(session) => {
            let device_count = session.device_count().await;
            Ok(Json(json!({
                "edge_id": edge_id,
                "name": session.name(),
                "hostname": session.hostname(),
                "ip_address": session.ip_address(),
                "connected_at": session.connected_at().to_rfc3339(),
                "device_count": device_count,
                "status": "connected",
            })))
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}
```

**Step 4: Run tests**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add crates/nimon-hub/src/server/mod.rs crates/nimon-hub/src/server/routes.rs
git commit -m "feat(hub): complete REST API with /api/v1 prefix and alert acknowledge"
```

---

## Task 5: Add Hub Configuration

The hub hardcodes its bind address and database path. Add a `HubConfig` struct with YAML support and CLI arg parsing.

**Files:**
- Create: `crates/nimon-hub/src/config.rs`
- Modify: `crates/nimon-hub/src/lib.rs`
- Modify: `crates/nimon-hub/src/server/mod.rs`
- Modify: `crates/nimon-hub/src/main.rs`
- Modify: `crates/nimon-hub/Cargo.toml`

**Step 1: Add serde_yaml dependency**

In `crates/nimon-hub/Cargo.toml`, add:
```toml
serde_yaml = "0.9"
```

**Step 2: Create HubConfig**

Create `crates/nimon-hub/src/config.rs`:
```rust
//! Hub server configuration

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Hub server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_db_path")]
    pub database_path: String,
    #[serde(default)]
    pub alert: AlertConfig,
}

fn default_host() -> String { "0.0.0.0".to_string() }
fn default_port() -> u16 { 8080 }
fn default_db_path() -> String { "./data/nimon.db".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    #[serde(default = "default_cooldown")]
    pub default_cooldown_minutes: i64,
    #[serde(default = "default_max_firing")]
    pub max_firing_count: i32,
}

fn default_cooldown() -> i64 { 5 }
fn default_max_firing() -> i32 { 100 }

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            default_cooldown_minutes: default_cooldown(),
            max_firing_count: default_max_firing(),
        }
    }
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            database_path: default_db_path(),
            alert: AlertConfig::default(),
        }
    }
}

impl HubConfig {
    /// Load config from a YAML file, falling back to defaults for missing fields
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: HubConfig = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    /// Load config from file, or return defaults if file doesn't exist
    pub fn load_or_default(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            Self::from_file(path)
        } else {
            Ok(Self::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HubConfig::default();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert_eq!(config.database_path, "./data/nimon.db");
    }

    #[test]
    fn test_from_yaml() {
        let yaml = "port: 9090\ndatabase_path: /tmp/nimon.db";
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.port, 9090);
        assert_eq!(config.database_path, "/tmp/nimon.db");
        assert_eq!(config.host, "0.0.0.0"); // default
    }
}
```

**Step 3: Register module and update run()**

In `crates/nimon-hub/src/lib.rs`, add `pub mod config;`.

Update `run()` signature to accept config:
```rust
pub async fn run(config: HubConfig) -> anyhow::Result<()> {
```

Replace the hardcoded values:
```rust
let db_dir = std::path::Path::new(&config.database_path).parent().unwrap_or(std::path::Path::new("data"));
std::fs::create_dir_all(db_dir)?;
let pool = SqlitePool::connect(format!("sqlite:{}", config.database_path)).await?;
```

Replace the bind address:
```rust
let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
```

Use config for alert manager:
```rust
let alert_config = crate::alert::manager::AlertManagerConfig {
    default_cooldown_minutes: config.alert.default_cooldown_minutes,
    max_firing_count: config.alert.max_firing_count,
    ..Default::default()
};
```

**Step 4: Update main.rs to use config**

Update `crates/nimon-hub/src/main.rs` to parse CLI args and load config:
```rust
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // Parse CLI args
    let config_path = std::env::args().nth(2);
    let config = match config_path {
        Some(path) => {
            let path = PathBuf::from(path);
            info!("Loading config from {:?}", path);
            nimon_hub::config::HubConfig::load_or_default(&path)?
        }
        None => {
            info!("Using default configuration");
            nimon_hub::config::HubConfig::default()
        }
    };

    info!("Starting NIMon Hub Server on {}:{}", config.host, config.port);
    nimon_hub::run(config).await?;
    Ok(())
}
```

**Step 5: Create example config file**

Create `config/hub.example.yaml`:
```yaml
host: "0.0.0.0"
port: 8080
database_path: "./data/nimon.db"
alert:
  default_cooldown_minutes: 5
  max_firing_count: 100
```

**Step 6: Run tests**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 7: Commit**

```bash
git add crates/nimon-hub/src/config.rs crates/nimon-hub/src/lib.rs crates/nimon-hub/src/server/mod.rs crates/nimon-hub/src/main.rs crates/nimon-hub/Cargo.toml config/hub.example.yaml
git commit -m "feat(hub): add HubConfig with YAML support and CLI argument parsing"
```

---

## Task 6: Graceful Shutdown

Add signal handling (Ctrl+C / SIGTERM) to both hub and edge binaries so they shut down cleanly.

**Files:**
- Modify: `crates/nimon-hub/src/server/mod.rs`
- Modify: `crates/nimon-hub/src/main.rs`

**Step 1: Add graceful shutdown to hub server**

In `crates/nimon-hub/src/server/mod.rs`, update `run()` to use `with_graceful_shutdown`:

```rust
pub async fn run(config: HubConfig) -> anyhow::Result<()> {
    // ... existing setup code ...

    // Build shutdown signal
    let shutdown = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
        info!("Shutdown signal received");
    };

    // Allow optional tokio-console
    #[cfg(unix)]
    let shutdown = {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
        let ctrl_c = shutdown;
        async move {
            tokio::select! {
                _ = ctrl_c => {},
                _ = sigterm.recv() => {},
            }
        }
    };

    // ... existing router/app code ...

    // Start the server with graceful shutdown
    info!("Hub server listening on {}", addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;

    info!("Hub server shut down gracefully");
    Ok(())
}
```

Note: On Windows (the current target), `tokio::signal::ctrl_c()` is sufficient. The `#[cfg(unix)]` block adds SIGTERM support for Linux deployment.

**Step 2: Run tests**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 3: Commit**

```bash
git add crates/nimon-hub/src/server/mod.rs crates/nimon-hub/src/main.rs
git commit -m "feat(hub): add graceful shutdown with Ctrl+C signal handling"
```

---

## Task 7: Clean Up Dead Code

Remove unused/warning-generating code identified during reviews.

**Files:**
- Modify: `crates/nimon-hub/src/server/ws.rs` (remove — replaced by mod.rs handler)
- Modify: `crates/nimon-hub/src/server/mod.rs` (remove unused import, fix dead_code)
- Modify: `crates/nimon-hub/src/alert/manager.rs` (fix warnings)

**Step 1: Remove dead ws.rs utility module**

Delete `crates/nimon-hub/src/server/ws.rs` — its functionality has been replaced by the updated `ws_socket_handler` in `mod.rs`. Remove `pub mod ws;` from `crates/nimon-hub/src/server/mod.rs` line 6.

**Step 2: Fix unused variable warnings**

In `crates/nimon-hub/src/server/mod.rs`, prefix unused `state` parameter with underscore in `ws_socket_handler`:
```rust
async fn ws_socket_handler(socket: WebSocket, _state: HubState) {
```

In `crates/nimon-hub/src/alert/manager.rs`, remove the unused `sessions` field from `AlertManager` since it's never read (the hub server's `state.sessions()` is used instead). Update `new()` to not take `SessionStore` parameter. Update the call site in `server/mod.rs`.

**Step 3: Fix other warnings**

- Remove unused `error` import from `crates/nimon-hub/src/action/executor.rs`
- Fix the lifetime warning in `crates/nimon-hub/src/session/session_store.rs:87`

**Step 4: Run tests**

Run: `cargo test --workspace 2>&1 | grep -E "(warning|error|test result)"`
Expected: 0 errors, 0 warnings, all tests pass.

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor(hub): remove dead code and fix compiler warnings"
```
