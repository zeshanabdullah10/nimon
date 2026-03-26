# NIMon Phase 2: Edge Actors & Hub Server

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement edge node actors for device monitoring, WebSocket client for hub communication, and hub server for aggregation.

**Architecture:** Actix actors for concurrent device monitoring, tokio-tungstenite for WebSocket, Axum for hub HTTP/WebSocket server.

**Tech Stack:** actix, actix-rt, tokio-tungstenite, axum, tower-http, futures-util

---

## Task 1: Core Crate - Actor Infrastructure

**Files:**
- Create: `crates/nimon-core/src/actor/mod.rs`
- Create: `crates/nimon-core/src/actor/messages.rs`
- Create: `crates/nimon-core/src/actor/device_actor.rs`

**Goal:** Create base actor infrastructure with device monitoring actor.

**Steps:**
1. Add actix dependency to nimon-core
2. Create actor message types (DevicePoll, DeviceStatus, DeviceAlert)
3. Create DeviceActor that polls devices and emits status updates
4. Add tests for actor lifecycle
5. Commit: "feat(core): add actor infrastructure for device monitoring"

---

## Task 2: Edge Crate - Device Manager Actor

**Files:**
- Create: `crates/nimon-edge/src/actor/mod.rs`
- Create: `crates/nimon-edge/src/actor/device_manager.rs`
- Create: `crates/nimon-edge/src/actor/prediction_actor.rs`

**Goal:** Create DeviceManager that spawns DeviceActors and PredictionActor.

**Steps:**
1. Create DeviceManagerActor that manages device lifecycle
2. Create PredictionActor that analyzes device metrics
3. Wire up device discovery from NI-SysCfg
4. Add tests for device manager
5. Commit: "feat(edge): add device manager and prediction actors"

---

## Task 3: Edge Crate - WebSocket Client

**Files:**
- Create: `crates/nimon-edge/src/comm/mod.rs`
- Create: `crates/nimon-edge/src/comm/ws_client.rs`
- Create: `crates/nimon-edge/src/comm/message.rs`

**Goal:** Create WebSocket client for hub communication.

**Steps:**
1. Add tokio-tungstenite dependency
2. Create WebSocket client with auto-reconnect
3. Define message protocol (JSON)
4. Implement message serialization
5. Add tests for message protocol
6. Commit: "feat(edge): add WebSocket client for hub communication"

---

## Task 4: Edge Crate - Hub Connector Actor

**Files:**
- Create: `crates/nimon-edge/src/actor/hub_connector.rs`
- Modify: `crates/nimon-edge/src/lib.rs`

**Goal:** Create actor that bridges device actors to WebSocket.

**Steps:**
1. Create HubConnectorActor
2. Subscribe to device status updates
3. Forward updates to hub via WebSocket
4. Handle hub commands (config updates, actions)
5. Add buffering for offline operation
6. Commit: "feat(edge): add hub connector actor"

---

## Task 5: Hub Crate - WebSocket Server

**Files:**
- Create: `crates/nimon-hub/src/main.rs`
- Create: `crates/nimon-hub/src/server/mod.rs`
- Create: `crates/nimon-hub/src/server/ws.rs`
- Create: `crates/nimon-hub/src/server/routes.rs`

**Goal:** Create WebSocket server for edge connections.

**Steps:**
1. Add axum, tower-http dependencies
2. Create WebSocket upgrade handler
3. Create session manager for connected edges
4. Add REST routes for status/health
5. Add tests for WebSocket handling
6. Commit: "feat(hub): add WebSocket server for edge connections"

---

## Task 6: Hub Crate - Edge Session Manager

**Files:**
- Create: `crates/nimon-hub/src/session/mod.rs`
- Create: `crates/nimon-hub/src/session/edge_session.rs`
- Create: `crates/nimon-hub/src/session/session_store.rs`

**Goal:** Manage connected edge sessions and route messages.

**Steps:**
1. Create EdgeSession struct with WebSocket sender
2. Create SessionStore with dashmap for concurrent access
3. Implement message broadcasting
4. Handle edge registration and heartbeat
5. Add tests for session management
6. Commit: "feat(hub): add edge session manager"

---

## Phase 2 Summary

After Phase 2:
1. ✅ Edge node can monitor devices via actors
2. ✅ Edge node connects to hub via WebSocket
3. ✅ Hub server accepts edge connections
4. ✅ Hub aggregates device data from multiple edges
5. ✅ Real-time status updates flow edge → hub

---

## Running Tests

```bash
cargo test --workspace
```

---

## Running the System

```bash
# Terminal 1: Start hub
cargo run -p nimon-hub -- --config config/hub.yaml

# Terminal 2: Start edge
cargo run -p nimon-edge -- --config config/edge.yaml
```
