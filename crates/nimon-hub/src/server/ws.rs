//! Edge WebSocket endpoint.
//!
//! One task per connection. The connection registers with `edge_register`;
//! afterwards every payload's `edge_id` must match the registered edge.
//! Each connection has its own identity and cancellation token, so a
//! replaced or stale connection is closed reliably and can never
//! unregister its successor.

use std::collections::HashMap;
use std::time::Duration;

use axum::{
    extract::{
        ws::{
            rejection::WebSocketUpgradeRejection, Message as AxumWsMessage, WebSocket,
            WebSocketUpgrade,
        },
        Query, State,
    },
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use nimon_core::actor::messages::{
    ActionResult, DeviceAlert, DeviceRemoved, DeviceStatusUpdate, EdgeHeartbeat, EdgeRegister,
    PredictionResult,
};
use nimon_core::db::edge_repo::EdgeRepository;
use nimon_core::protocol::{WsMessage, WsMessageType, PROTOCOL_VERSION};
use nimon_core::{EdgeNode, EdgeStatus};

use crate::action::executor::{CompleteAction, EdgeDisconnected};
use crate::alert::manager::{EdgeActivityKind, IngestDeviceAlert};
use crate::server::auth::{bearer_token, token_matches, unauthorized};
use crate::server::{desired_to_config_update, notify_edge_activity, HubState};
use crate::session::{EdgeSession, HeartbeatInfo, OUTBOUND_CAPACITY};

/// WebSocket handler for edge connections. When `auth.edge_token` is set
/// the token must be given as `Authorization: Bearer <token>` or `?token=`.
pub async fn ws_handler(
    State(state): State<HubState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if let Some(expected) = state.auth().edge_token.as_deref() {
        let ok = token_matches(bearer_token(&headers), expected)
            || token_matches(query.get("token").map(|s| s.as_str()), expected);
        if !ok {
            warn!("Rejected edge WebSocket connection: missing or invalid edge token");
            return unauthorized("missing or invalid edge token");
        }
    }
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };
    info!("WebSocket connection request from edge");
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Mark an edge offline in the DB and tell the alert manager.
pub(crate) fn mark_edge_offline(state: &HubState, edge_id: &str) {
    if let Some(writer) = state.db_writer() {
        let edge_id = edge_id.to_string();
        writer.submit(move |pool| async move {
            if let Err(e) = EdgeRepository::new(&pool)
                .update_status(&edge_id, EdgeStatus::Offline)
                .await
            {
                error!("Failed to mark edge {} offline: {}", edge_id, e);
            }
        });
    }
    notify_edge_activity(state, edge_id, EdgeActivityKind::Disconnected);
    // Results can no longer arrive: fail its pending edge actions now
    if let Some(executor) = state.action_executor() {
        executor.do_send(EdgeDisconnected {
            edge_id: edge_id.to_string(),
        });
    }
}

/// Per-connection protocol state.
struct Connection {
    state: HubState,
    token: CancellationToken,
    out_tx: mpsc::Sender<WsMessage>,
    session: Option<EdgeSession>,
    /// Direct replies queued while handling a message
    replies: Vec<WsMessage>,
    /// Close the socket after sending the replies
    close: bool,
}

/// Longest a single frame write may take before the edge is considered
/// stalled and the connection is dropped
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
/// Best-effort Close frame on a hub-initiated close
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
/// A connection that has not registered within this time is closed
pub const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(15);

type WsSink = futures_util::stream::SplitSink<WebSocket, AxumWsMessage>;

async fn handle_socket(socket: WebSocket, state: HubState) {
    info!("WebSocket connection established");
    let registration_timeout = state.registration_timeout();
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<WsMessage>(OUTBOUND_CAPACITY);
    let server_shutdown = state.shutdown_token();
    // Dropping `conn` (normal exit, panic or task abort) runs the
    // unregister/offline cleanup exactly once
    let mut conn = Connection {
        state,
        token: CancellationToken::new(),
        out_tx,
        session: None,
        replies: Vec::new(),
        close: false,
    };
    let registration_deadline = tokio::time::sleep(registration_timeout);
    tokio::pin!(registration_deadline);

    'outer: loop {
        tokio::select! {
            _ = conn.token.cancelled() => {
                // Replaced, stale or a slow consumer (which is not reading,
                // so a Close write may never complete): bounded attempt only
                info!(
                    "Closing connection of edge {} (replaced, stale or slow)",
                    conn.edge_id().unwrap_or("<unregistered>")
                );
                close_quietly(&mut sender).await;
                break;
            }
            _ = server_shutdown.cancelled() => {
                close_quietly(&mut sender).await;
                break;
            }
            _ = &mut registration_deadline, if conn.session.is_none() => {
                warn!(
                    "Closing WebSocket connection: no valid edge_register within {}s",
                    registration_timeout.as_secs()
                );
                close_quietly(&mut sender).await;
                break;
            }
            outbound = out_rx.recv() => {
                let Some(msg) = outbound else { break };
                if !send_json(&mut sender, &msg).await {
                    break;
                }
            }
            incoming = receiver.next() => {
                let Some(incoming) = incoming else { break };
                match incoming {
                    Ok(AxumWsMessage::Text(text)) => {
                        conn.on_text(&text).await;
                        for reply in std::mem::take(&mut conn.replies) {
                            if !send_json(&mut sender, &reply).await {
                                break 'outer;
                            }
                        }
                        if conn.close {
                            close_quietly(&mut sender).await;
                            break;
                        }
                    }
                    Ok(AxumWsMessage::Close(_)) => {
                        info!("WebSocket close received");
                        break;
                    }
                    Ok(_) => {
                        // Ping/pong/binary: any traffic counts as liveness
                        if let Some(session) = &conn.session {
                            session.touch().await;
                        }
                    }
                    Err(e) => {
                        warn!("WebSocket error: {}", e);
                        break;
                    }
                }
            }
        }
    }

    conn.on_close();
    info!("WebSocket connection closed");
}

/// Send a Close frame without letting a stalled peer block the loop.
async fn close_quietly(sender: &mut WsSink) {
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, sender.send(AxumWsMessage::Close(None))).await;
}

/// Write one message; false when the socket failed or the write did not
/// complete within [`SEND_TIMEOUT`] (stalled edge).
async fn send_json(sender: &mut WsSink, msg: &WsMessage) -> bool {
    match msg.to_json() {
        Ok(json) => {
            match tokio::time::timeout(SEND_TIMEOUT, sender.send(AxumWsMessage::Text(json))).await {
                Ok(result) => result.is_ok(),
                Err(_) => {
                    warn!(
                        "WebSocket write stalled for more than {}s; dropping the connection",
                        SEND_TIMEOUT.as_secs()
                    );
                    false
                }
            }
        }
        Err(e) => {
            error!("Failed to serialize outbound message: {}", e);
            true
        }
    }
}

/// True when `device_id` belongs to `edge_id`: `<edge_id>:<name>` (edge
/// device ids, see nimon-edge `devices::device_id`) or the edge id itself
/// (edge-level alerts).
pub fn edge_owns_device(edge_id: &str, device_id: &str) -> bool {
    device_id == edge_id
        || device_id
            .strip_prefix(edge_id)
            .and_then(|rest| rest.strip_prefix(':'))
            .is_some_and(|name| !name.is_empty())
}

impl Drop for Connection {
    fn drop(&mut self) {
        // No-op after a regular on_close (the session was taken)
        self.on_close();
    }
}

impl Connection {
    fn edge_id(&self) -> Option<&str> {
        self.session.as_ref().map(|s| s.edge_id())
    }

    fn reply_error(&mut self, code: &str, message: String, reply_to: Option<&str>) {
        let mut msg = WsMessage::error(code.to_string(), message, None);
        if let Some(id) = reply_to {
            msg = msg.with_reply_to(id);
        }
        self.replies.push(msg);
    }

    /// Unregister on disconnect — only if the stored session is still ours.
    fn on_close(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        let sessions = self.state.sessions();
        if sessions
            .remove_if_current(session.edge_id(), session.conn_id())
            .is_some()
        {
            info!("Edge disconnected: {}", session.edge_id());
            mark_edge_offline(&self.state, session.edge_id());
        } else {
            debug!(
                "Connection {} of edge {} closed after being replaced; session kept",
                session.conn_id(),
                session.edge_id()
            );
        }
    }

    /// Payload device ids must belong to the registered edge
    /// (`<edge_id>:<name>`, or the edge id for edge-level reports), so one
    /// edge can never touch another edge's devices or alerts.
    fn check_device(&mut self, device_id: &str, msg_id: &str) -> bool {
        let Some(registered) = self.edge_id() else {
            return false;
        };
        if edge_owns_device(registered, device_id) {
            return true;
        }
        let registered = registered.to_string();
        warn!(
            "Rejecting payload for device '{}' not owned by edge '{}'",
            device_id, registered
        );
        self.reply_error(
            "DEVICE_ID_NOT_OWNED",
            format!(
                "device_id '{}' does not belong to edge '{}' (expected '{}:<name>' or '{}')",
                device_id, registered, registered, registered
            ),
            Some(msg_id),
        );
        false
    }

    /// Payload edge ids must match the registered edge.
    fn check_edge(&mut self, claimed: &str, msg_id: &str) -> bool {
        match self.edge_id() {
            Some(registered) if registered == claimed => true,
            Some(registered) => {
                let registered = registered.to_string();
                warn!(
                    "Rejecting payload for edge '{}' on connection registered as '{}'",
                    claimed, registered
                );
                self.reply_error(
                    "EDGE_ID_MISMATCH",
                    format!(
                        "payload edge_id '{}' does not match registered edge '{}'",
                        claimed, registered
                    ),
                    Some(msg_id),
                );
                false
            }
            None => false,
        }
    }

    async fn on_text(&mut self, text: &str) {
        debug!("Received text message: {}", text);
        let ws_msg = match WsMessage::from_json(text) {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to parse WebSocket message: {}", e);
                self.reply_error(
                    "PARSE_ERROR",
                    format!("Failed to parse message: {}", e),
                    None,
                );
                return;
            }
        };

        if let Some(session) = &self.session {
            session.touch().await;
        }

        if !ws_msg.version_compatible() {
            warn!(
                "Incompatible protocol version {} (ours {}), closing",
                ws_msg.version, PROTOCOL_VERSION
            );
            let msg_id = ws_msg.msg_id.clone();
            self.reply_error(
                "VERSION_MISMATCH",
                format!(
                    "incompatible protocol version {} (hub speaks {})",
                    ws_msg.version, PROTOCOL_VERSION
                ),
                Some(&msg_id),
            );
            self.close = true;
            return;
        }

        match ws_msg.msg_type {
            WsMessageType::EdgeRegister => self.on_register(ws_msg).await,
            WsMessageType::Ping => {
                let ping_ts = ws_msg
                    .payload::<nimon_core::protocol::PingMessage>()
                    .map(|p| p.timestamp)
                    .unwrap_or(ws_msg.timestamp);
                self.replies
                    .push(WsMessage::pong(ping_ts).with_reply_to(ws_msg.msg_id));
            }
            WsMessageType::Unknown => {
                info!(
                    "Ignoring message of unknown type from edge {} (version {})",
                    self.edge_id().unwrap_or("<unregistered>"),
                    ws_msg.version
                );
            }
            WsMessageType::Ack | WsMessageType::Pong | WsMessageType::Error => {
                debug!("{:?} from edge: {}", ws_msg.msg_type, ws_msg.payload);
            }
            WsMessageType::ConfigUpdate
            | WsMessageType::ExecuteAction
            | WsMessageType::HubCommand => {
                warn!(
                    "Ignoring hub-to-edge message type {:?} sent by an edge",
                    ws_msg.msg_type
                );
            }
            WsMessageType::DeviceStatus
            | WsMessageType::DeviceAlert
            | WsMessageType::Prediction
            | WsMessageType::Heartbeat
            | WsMessageType::ActionResult
            | WsMessageType::DeviceRemoved => {
                if self.session.is_none() {
                    let msg_id = ws_msg.msg_id.clone();
                    self.reply_error(
                        "NOT_REGISTERED",
                        "send edge_register before other messages".to_string(),
                        Some(&msg_id),
                    );
                    return;
                }
                self.on_edge_payload(ws_msg).await;
            }
        }
    }

    async fn on_register(&mut self, ws_msg: WsMessage) {
        let msg_id = ws_msg.msg_id.clone();
        let version = ws_msg.version.clone();
        let registration = match ws_msg.into_payload::<EdgeRegister>() {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to parse EdgeRegister payload: {}", e);
                self.reply_error(
                    "INVALID_PAYLOAD",
                    format!("invalid edge_register payload: {}", e),
                    Some(&msg_id),
                );
                return;
            }
        };

        if let Some(existing) = self.edge_id() {
            if existing != registration.edge_id {
                let existing = existing.to_string();
                warn!(
                    "Rejecting re-registration as '{}' on connection registered as '{}'",
                    registration.edge_id, existing
                );
                self.reply_error(
                    "EDGE_ID_MISMATCH",
                    format!("connection is already registered as '{}'", existing),
                    Some(&msg_id),
                );
            } else {
                self.replies.push(WsMessage::ack(msg_id, true, None));
            }
            return;
        }
        if registration.edge_id.trim().is_empty() {
            self.reply_error(
                "INVALID_PAYLOAD",
                "edge_id must not be empty".to_string(),
                Some(&msg_id),
            );
            return;
        }

        info!(
            "Edge registered: {} ({}), protocol {}",
            registration.edge_id, registration.name, version
        );

        let session = EdgeSession::for_connection(
            registration.edge_id.clone(),
            registration.name.clone(),
            registration.hostname.clone(),
            registration.ip_address.clone(),
            version,
            self.out_tx.clone(),
            self.token.clone(),
        );

        if let Some(old) = self.state.sessions().add(session.clone()) {
            if !old.same_connection(&session) {
                warn!(
                    "Duplicate registration for edge {}, closing the previous connection",
                    registration.edge_id
                );
                old.request_shutdown();
            }
        }
        self.session = Some(session.clone());

        if let Some(writer) = self.state.db_writer() {
            let edge_node = EdgeNode {
                id: registration.edge_id.clone(),
                name: registration.name,
                hostname: registration.hostname,
                ip_address: registration.ip_address,
                last_seen: Some(chrono::Utc::now()),
                status: EdgeStatus::Online,
            };
            writer.submit(move |pool| async move {
                if let Err(e) = EdgeRepository::new(&pool).upsert(&edge_node).await {
                    error!("Failed to upsert edge node: {}", e);
                }
            });
        }
        notify_edge_activity(
            &self.state,
            &registration.edge_id,
            EdgeActivityKind::Connected,
        );

        // Desired state (config file + API overrides)
        let desired = self.state.desired_for(&registration.edge_id);
        if let Some(update) = desired_to_config_update(&desired, session.is_legacy_protocol()) {
            if let Err(e) = session.send(WsMessage::config_update(update)) {
                warn!(
                    "Failed to push desired config to {}: {}",
                    registration.edge_id, e
                );
            }
        }

        self.replies.push(WsMessage::ack(msg_id, true, None));
    }

    async fn on_edge_payload(&mut self, ws_msg: WsMessage) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let msg_id = ws_msg.msg_id.clone();
        let msg_type = ws_msg.msg_type;
        let state = self.state.clone();

        macro_rules! payload {
            ($ty:ty) => {
                match ws_msg.into_payload::<$ty>() {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("Failed to parse {:?} payload: {}", msg_type, e);
                        self.reply_error(
                            "INVALID_PAYLOAD",
                            format!("invalid {:?} payload: {}", msg_type, e),
                            Some(&msg_id),
                        );
                        return;
                    }
                }
            };
        }

        match msg_type {
            WsMessageType::DeviceStatus => {
                let status = payload!(DeviceStatusUpdate);
                if !self.check_edge(&status.edge_id, &msg_id)
                    || !self.check_device(&status.device_id, &msg_id)
                {
                    return;
                }
                debug!(
                    "Device status from {}: device={}, status={:?}",
                    status.edge_id, status.device_id, status.status
                );
                state.mark_device_present(&status.device_id);
                session
                    .update_device(
                        status.device_id.clone(),
                        status.status,
                        status.metrics.clone(),
                        status.is_simulated,
                    )
                    .await;
                if let Some(am) = state.alert_manager() {
                    am.do_send(status);
                }
            }
            WsMessageType::Prediction => {
                let prediction = payload!(PredictionResult);
                if !self.check_edge(&prediction.edge_id, &msg_id)
                    || !self.check_device(&prediction.device_id, &msg_id)
                {
                    return;
                }
                if let Some(am) = state.alert_manager() {
                    am.do_send(prediction);
                }
            }
            WsMessageType::DeviceAlert => {
                let alert = payload!(DeviceAlert);
                if !self.check_edge(&alert.edge_id, &msg_id)
                    || !self.check_device(&alert.device_id, &msg_id)
                {
                    return;
                }
                info!("Device alert received: {}", alert.message);
                if let Some(am) = state.alert_manager() {
                    am.do_send(IngestDeviceAlert { alert });
                }
            }
            WsMessageType::DeviceRemoved => {
                let removed = payload!(DeviceRemoved);
                if !self.check_edge(&removed.edge_id, &msg_id)
                    || !self.check_device(&removed.device_id, &msg_id)
                {
                    return;
                }
                session.remove_device(&removed.device_id).await;
                state.mark_device_removed(&removed.device_id);
                if let Some(am) = state.alert_manager() {
                    am.do_send(removed);
                }
            }
            WsMessageType::ActionResult => {
                // Correlate with the request: reply_to, else the msg_id
                let correlation_id = ws_msg.correlation_id().to_string();
                let result = payload!(ActionResult);
                debug!(
                    "Action result for {} (request {}): success={}",
                    result.action_id, correlation_id, result.success
                );
                // The sender is the connection's registered edge: only
                // actions dispatched to that edge can be completed
                if let Some(executor) = state.action_executor() {
                    executor.do_send(CompleteAction {
                        edge_id: session.edge_id().to_string(),
                        msg_id: correlation_id,
                        result,
                    });
                }
            }
            WsMessageType::Heartbeat => {
                let heartbeat = payload!(EdgeHeartbeat);
                if !self.check_edge(&heartbeat.edge_id, &msg_id) {
                    return;
                }
                session
                    .update_heartbeat(HeartbeatInfo {
                        device_count: Some(heartbeat.device_count),
                        status: Some(heartbeat.status),
                        uptime_secs: heartbeat.uptime_secs,
                        version: heartbeat.version,
                    })
                    .await;
                // A live heartbeat resolves an edge-offline alert raised
                // while this connection was silent
                notify_edge_activity(&state, &heartbeat.edge_id, EdgeActivityKind::Seen);
                if let Some(writer) = state.db_writer() {
                    let edge_id = heartbeat.edge_id;
                    writer.submit(move |pool| async move {
                        if let Err(e) = EdgeRepository::new(&pool)
                            .update_status(&edge_id, EdgeStatus::Online)
                            .await
                        {
                            error!("Failed to update edge last_seen: {}", e);
                        }
                    });
                }
            }
            _ => {}
        }
    }
}
