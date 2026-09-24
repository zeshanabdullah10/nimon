//! Hub Connector Actor
//!
//! Bridges the edge actors to the hub. Owns the WebSocket client and the
//! single offline buffer:
//!
//! - on every (re)connect the edge registers FIRST, then the buffer is
//!   flushed in order, then live traffic flows
//! - frames that were accepted but not written when a connection dropped
//!   come back from the client and are re-queued at the front
//! - when the buffer is full the oldest device-status messages go first;
//!   alerts, predictions, action results and removals are kept
//! - `device_removed` (protocol 1.1) is only sent once the hub on the
//!   current connection is known to speak >= 1.1 (learned from the
//!   envelope version of the first message it sends); until then removals
//!   are held, and for a 1.0 hub they are dropped
//!
//! Incoming hub messages: config updates, remediation actions (answered
//! with `action_result` carrying `reply_to` = the request's `msg_id`), hub
//! commands (`resend_state`, `reload_config`), acks, errors, pings.

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use actix::prelude::*;
use tracing::{debug, info, warn};

use nimon_core::actor::messages::{
    ActionResult, ConfigUpdate, DeviceAlert, DeviceRemoved, DeviceStatusUpdate, EdgeHeartbeat,
    EdgeRegister, ExecuteAction, PredictionResult, ThresholdConfig,
};
use nimon_core::Severity;

use crate::action::ActionRuntime;
use crate::actor::device_manager::{ApplyConfig, DeviceManagerActor, ResendState};
use crate::comm::{
    ConnectionSender, ConnectionState, ErrorMessage, HubCommand, Outbound, OutboundBuffer,
    OutboundClass, WsClient, WsClientConfig, WsEvent, WsMessage, WsMessageType,
};
use crate::config::EdgeConfig;

/// Default maximum number of buffered messages
const DEFAULT_BUFFER_MESSAGES: usize = 1000;
/// Default maximum buffered bytes
const DEFAULT_BUFFER_BYTES: usize = 16 * 1024 * 1024;
/// How long `Disconnect` waits for the connection task to finish
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Hub connector actor configuration
#[derive(Debug, Clone)]
pub struct HubConnectorConfig {
    /// Edge node ID
    pub edge_id: String,
    /// Edge node name
    pub edge_name: String,
    /// Hostname (optional)
    pub hostname: Option<String>,
    /// IP address override (None = local address of the hub socket)
    pub ip_address: Option<String>,
    /// Heartbeat interval
    pub heartbeat_interval: Duration,
    /// Connect when the actor starts (`Connect` is idempotent either way)
    pub auto_connect: bool,
    /// Remediation action execution settings
    pub action_runtime: Arc<ActionRuntime>,
    /// WebSocket transport settings (URL, token, backoff, pings)
    pub ws: WsClientConfig,
    /// Buffer while disconnected
    pub buffer_enabled: bool,
    /// Maximum buffered messages
    pub buffer_max_messages: usize,
    /// Maximum buffered bytes
    pub buffer_max_bytes: usize,
    /// Edge YAML path for the `reload_config` hub command
    pub config_path: Option<PathBuf>,
}

impl Default for HubConnectorConfig {
    fn default() -> Self {
        Self {
            edge_id: "edge-1".to_string(),
            edge_name: "Edge Node 1".to_string(),
            hostname: None,
            ip_address: None,
            heartbeat_interval: Duration::from_secs(30),
            auto_connect: true,
            action_runtime: Arc::new(ActionRuntime::default()),
            ws: WsClientConfig::default(),
            buffer_enabled: true,
            buffer_max_messages: DEFAULT_BUFFER_MESSAGES,
            buffer_max_bytes: DEFAULT_BUFFER_BYTES,
            config_path: None,
        }
    }
}

/// Hub connector actor
pub struct HubConnectorActor {
    config: HubConnectorConfig,
    ws: WsClient,
    state: ConnectionState,
    /// Write handle of the live connection (set after registration)
    sender: Option<ConnectionSender>,
    buffer: OutboundBuffer,
    local_ip: Option<IpAddr>,
    started_at: Instant,
    /// Device count reported by the device manager (used in heartbeats)
    device_count: usize,
    /// Why the edge is degraded (None = online)
    degraded: Option<String>,
    /// Device manager address for config push-down / resend_state
    device_manager: Option<Addr<DeviceManagerActor>>,
    /// Unknown / unexpected message types already logged
    logged_types: HashSet<String>,
    version_warned: bool,
    /// Protocol version (major, minor) of the hub on this connection,
    /// learned from the first message it sends (None = not known yet)
    hub_protocol: Option<(u64, u64)>,
    /// `device_removed` messages waiting until the hub is known to speak
    /// protocol >= 1.1 (a 1.0 hub cannot parse them)
    pending_removals: Vec<DeviceRemoved>,
}

/// First protocol version that knows `device_removed`
const DEVICE_REMOVED_SINCE: (u64, u64) = (1, 1);
/// Most removals held while the hub version is unknown
const MAX_PENDING_REMOVALS: usize = 256;

/// `"major.minor[...]"` -> (major, minor); None when unparseable
fn parse_protocol_version(version: &str) -> Option<(u64, u64)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.trim().parse().ok()?;
    let minor = parts
        .next()
        .map(|m| m.trim().parse().ok())
        .unwrap_or(Some(0))?;
    Some((major, minor))
}

impl HubConnectorActor {
    /// Create a new hub connector actor
    pub fn new(config: HubConnectorConfig) -> Self {
        let max = if config.buffer_enabled {
            config.buffer_max_messages
        } else {
            0
        };
        let buffer = OutboundBuffer::new(max, config.buffer_max_bytes);
        Self {
            ws: WsClient::new(config.ws.clone()),
            config,
            state: ConnectionState::Disconnected,
            sender: None,
            buffer,
            local_ip: None,
            started_at: Instant::now(),
            device_count: 0,
            degraded: None,
            device_manager: None,
            logged_types: HashSet::new(),
            version_warned: false,
            hub_protocol: None,
            pending_removals: Vec::new(),
        }
    }

    /// Send a removal only to hubs that understand it; hold it while the
    /// hub's protocol version is unknown, drop it for older hubs (their
    /// device list ages out on its own)
    fn send_removal(&mut self, msg: DeviceRemoved) {
        match self.hub_protocol {
            Some(v) if v >= DEVICE_REMOVED_SINCE => {
                self.enqueue(WsMessage::device_removed(msg), OutboundClass::Priority)
            }
            Some((major, minor)) => debug!(
                "Hub protocol {major}.{minor} has no device_removed; not reporting removal of {}",
                msg.device_id
            ),
            None => {
                self.pending_removals
                    .retain(|p| p.device_id != msg.device_id);
                if self.pending_removals.len() >= MAX_PENDING_REMOVALS {
                    self.pending_removals.remove(0);
                }
                self.pending_removals.push(msg);
            }
        }
    }

    /// Remember the hub's protocol version (first message of a connection)
    /// and release held removals accordingly
    fn learn_hub_version(&mut self, version: &str) {
        if self.hub_protocol.is_some() {
            return;
        }
        let Some(v) = parse_protocol_version(version) else {
            return;
        };
        self.hub_protocol = Some(v);
        if v < DEVICE_REMOVED_SINCE && !self.pending_removals.is_empty() {
            debug!(
                "Dropping {} held device removal(s): hub protocol {version} predates device_removed",
                self.pending_removals.len()
            );
        }
        for msg in std::mem::take(&mut self.pending_removals) {
            self.send_removal(msg);
        }
    }

    /// Provide the device manager address for config push-down
    pub fn with_device_manager(mut self, addr: Addr<DeviceManagerActor>) -> Self {
        self.device_manager = Some(addr);
        self
    }

    fn registration(&self) -> EdgeRegister {
        EdgeRegister {
            edge_id: self.config.edge_id.clone(),
            name: self.config.edge_name.clone(),
            hostname: self.config.hostname.clone(),
            ip_address: self
                .config
                .ip_address
                .clone()
                .or_else(|| self.local_ip.map(|ip| ip.to_string())),
        }
    }

    fn heartbeat(&self) -> EdgeHeartbeat {
        EdgeHeartbeat {
            edge_id: self.config.edge_id.clone(),
            timestamp: chrono::Utc::now(),
            device_count: self.device_count,
            status: if self.degraded.is_some() {
                "degraded".to_string()
            } else {
                "online".to_string()
            },
            uptime_secs: Some(self.started_at.elapsed().as_secs()),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }
    }

    fn start_connection(&mut self, ctx: &mut Context<Self>) -> bool {
        if self.state == ConnectionState::ShuttingDown {
            return false;
        }
        let started = self.ws.connect(ctx.address().recipient());
        if started {
            info!("Connecting to hub at {}", self.config.ws.hub_url);
            self.state = ConnectionState::Connecting;
        }
        started
    }

    /// Serialize once and send (or buffer)
    fn enqueue(&mut self, msg: WsMessage, class: OutboundClass) {
        match msg.to_json() {
            Ok(json) => self.send_outbound(Outbound::new(json, class)),
            Err(e) => warn!("Failed to serialize {:?}: {}", msg.msg_type, e),
        }
    }

    fn send_outbound(&mut self, item: Outbound) {
        let item = match &self.sender {
            Some(sender) => match sender.send(item) {
                Ok(()) => return,
                Err(item) => {
                    // connection already gone; the Disconnected event follows
                    self.sender = None;
                    item
                }
            },
            None => item,
        };
        let first_drop = self.buffer.is_empty();
        self.buffer.push_back(item);
        if first_drop && !self.buffer.is_empty() {
            debug!("Hub offline, buffering messages");
        }
    }

    /// Number of messages currently buffered
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    fn on_connected(&mut self, sender: ConnectionSender, local_ip: Option<IpAddr>) {
        self.local_ip = local_ip;
        // possibly a different hub now: learn its version again
        self.hub_protocol = None;
        // 1. registration always goes first
        let reg = WsMessage::edge_register(self.registration());
        let reg = match reg.to_json() {
            Ok(json) => Outbound::new(json, OutboundClass::Ephemeral),
            Err(e) => {
                warn!("Failed to serialize registration: {e}");
                return;
            }
        };
        if sender.send(reg).is_err() {
            return; // dropped already; Disconnected follows
        }
        self.state = ConnectionState::Connected;
        info!(
            "Registered with hub as {} ({})",
            self.config.edge_id, self.config.edge_name
        );

        // 2. flush the offline buffer in order
        let dropped = self.buffer.take_dropped();
        if dropped > 0 {
            warn!("{dropped} buffered message(s) were dropped while offline (buffer full)");
        }
        let pending = self.buffer.len();
        while let Some(item) = self.buffer.pop_front() {
            if let Err(item) = sender.send(item) {
                self.buffer.push_front(item);
                return;
            }
        }
        if pending > 0 {
            info!("Flushed {pending} buffered message(s) to hub");
        }
        // 3. live traffic
        self.sender = Some(sender);
        // a fresh heartbeat so the hub knows count/version immediately
        self.send_heartbeat();
    }

    fn on_disconnected(&mut self, unsent: Vec<Outbound>, reason: &str) {
        self.sender = None;
        // removals raised while offline wait for the next hub's version
        self.hub_protocol = None;
        if !unsent.is_empty() {
            debug!("Re-queueing {} unsent message(s)", unsent.len());
        }
        self.buffer.requeue_front(unsent);
        if self.state != ConnectionState::ShuttingDown {
            self.state = ConnectionState::Reconnecting;
        }
        debug!(
            "Hub connection down ({reason}); {} buffered",
            self.buffer.len()
        );
    }

    fn send_heartbeat(&mut self) {
        let Some(sender) = &self.sender else {
            return; // heartbeats are never buffered
        };
        if let Ok(json) = WsMessage::heartbeat(self.heartbeat()).to_json() {
            if sender
                .send(Outbound::new(json, OutboundClass::Ephemeral))
                .is_err()
            {
                self.sender = None;
            }
        }
    }

    fn log_once(&mut self, kind: &str) {
        if self.logged_types.insert(kind.to_string()) {
            debug!("Ignoring hub message of type '{kind}' (logged once)");
        }
    }

    /// Handle a message received from the hub
    fn handle_message(
        &mut self,
        ws_msg: WsMessage,
        unknown_type: Option<String>,
        ctx: &mut Context<Self>,
    ) {
        self.learn_hub_version(&ws_msg.version);
        if !ws_msg.version_compatible() && !self.version_warned {
            self.version_warned = true;
            warn!(
                "Hub speaks protocol {} (edge {}); continuing",
                ws_msg.version,
                nimon_core::protocol::PROTOCOL_VERSION
            );
        }

        match ws_msg.msg_type {
            WsMessageType::ConfigUpdate => match ws_msg.into_payload::<ConfigUpdate>() {
                Ok(config) => {
                    debug!(
                        "Config update from hub: poll={:?}, thresholds={:?}",
                        config.poll_interval_secs, config.thresholds
                    );
                    self.apply_config(ApplyConfig {
                        poll_interval_secs: config.poll_interval_secs,
                        thresholds: config.thresholds,
                    });
                }
                Err(e) => warn!("Failed to parse ConfigUpdate payload: {}", e),
            },
            WsMessageType::ExecuteAction => self.handle_execute_action(ws_msg, ctx),
            WsMessageType::HubCommand => self.handle_hub_command(ws_msg),
            WsMessageType::Ping => {
                let ts = ws_msg
                    .payload::<nimon_core::protocol::PingMessage>()
                    .map(|p| p.timestamp)
                    .unwrap_or(ws_msg.timestamp);
                let pong = WsMessage::pong(ts).with_reply_to(ws_msg.msg_id);
                self.enqueue(pong, OutboundClass::Ephemeral);
            }
            WsMessageType::Pong => debug!("Pong from hub"),
            WsMessageType::Ack => debug!(
                "Hub ack for {}",
                ws_msg.reply_to.as_deref().unwrap_or(ws_msg.msg_id.as_str())
            ),
            WsMessageType::Error => match ws_msg.into_payload::<ErrorMessage>() {
                Ok(err) => {
                    warn!(
                        "Hub error {}: {}{}",
                        err.code,
                        err.message,
                        err.details.map(|d| format!(" ({d})")).unwrap_or_default()
                    );
                    if is_fatal_error_code(&err.code) {
                        // retrying every few seconds will not fix this
                        self.ws.request_max_backoff();
                    }
                }
                Err(e) => warn!("Hub sent an unparseable error: {e}"),
            },
            WsMessageType::Unknown => {
                let kind = unknown_type.unwrap_or_else(|| "unknown".to_string());
                self.log_once(&kind);
            }
            other => {
                // edge->hub types echoed back: not expected
                self.log_once(&format!("{other:?}"));
            }
        }
    }

    fn apply_config(&self, config: ApplyConfig) {
        match self.device_manager {
            Some(ref manager) => manager.do_send(config),
            None => warn!("Config update received but no device manager attached"),
        }
    }

    fn handle_execute_action(&mut self, ws_msg: WsMessage, ctx: &mut Context<Self>) {
        let request_id = ws_msg.msg_id.clone();
        let field = |name: &str| {
            ws_msg
                .payload
                .get(name)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let raw_action_id = field("action_id");
        let raw_type = ws_msg
            .payload
            .get("action_type")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "<missing>".to_string());

        let action = match ws_msg.into_payload::<ExecuteAction>() {
            Ok(action) => action,
            Err(e) => {
                warn!("Rejecting execute_action {request_id}: {e}");
                match raw_action_id {
                    Some(action_id) => {
                        let result = ActionResult {
                            action_id,
                            success: false,
                            output: None,
                            error: Some(format!(
                                "unsupported or malformed action (type {raw_type}): {e}"
                            )),
                            exit_code: None,
                            duration_ms: 0,
                        };
                        self.enqueue(
                            WsMessage::action_result(result).with_reply_to(request_id),
                            OutboundClass::Priority,
                        );
                    }
                    None => self.enqueue(
                        WsMessage::error(
                            "INVALID_ACTION".to_string(),
                            format!("execute_action without action_id: {e}"),
                            None,
                        )
                        .with_reply_to(request_id),
                        OutboundClass::Priority,
                    ),
                }
                return;
            }
        };

        debug!("Executing action {} from hub", action.action_id);
        let device_id = action.device_id.clone();
        let action_type = action.action_type.to_string();
        let runtime = self.config.action_runtime.clone();
        let fut = async move { runtime.run(action).await }
            .into_actor(self)
            .map(move |result: ActionResult, actor, _ctx| {
                info!(
                    "Action {} ({}) finished: success={} in {}ms",
                    result.action_id, action_type, result.success, result.duration_ms
                );
                if !result.success {
                    let alert = DeviceAlert {
                        device_id,
                        edge_id: actor.config.edge_id.clone(),
                        severity: Severity::Warning,
                        message: format!(
                            "Action {} ({}) failed: {}",
                            action_type,
                            result.action_id,
                            result.error.as_deref().unwrap_or("unknown error")
                        ),
                        metric_name: None,
                        metric_value: None,
                        timestamp: chrono::Utc::now(),
                    };
                    actor.enqueue(WsMessage::device_alert(alert), OutboundClass::Priority);
                }
                actor.enqueue(
                    WsMessage::action_result(result).with_reply_to(request_id),
                    OutboundClass::Priority,
                );
            });
        ctx.spawn(fut);
    }

    fn handle_hub_command(&mut self, ws_msg: WsMessage) {
        let request_id = ws_msg.msg_id.clone();
        let command = match ws_msg.into_payload::<HubCommand>() {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to parse HubCommand payload: {e}");
                return;
            }
        };
        let outcome: Result<(), String> = match command.command.as_str() {
            "resend_state" => {
                info!("Hub requested resend_state");
                self.enqueue(
                    WsMessage::edge_register(self.registration()),
                    OutboundClass::Ephemeral,
                );
                if let Some(ref manager) = self.device_manager {
                    manager.do_send(ResendState);
                }
                self.send_heartbeat();
                Ok(())
            }
            "reload_config" => self.reload_config(),
            other => {
                warn!("Unknown hub command '{other}' ignored");
                Err(format!("unknown command '{other}'"))
            }
        };
        let (ok, err) = match outcome {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e)),
        };
        self.enqueue(
            WsMessage::ack(request_id, ok, err),
            OutboundClass::Ephemeral,
        );
    }

    /// Re-read the edge YAML and apply thresholds + poll interval
    fn reload_config(&self) -> Result<(), String> {
        let Some(ref path) = self.config.config_path else {
            return Err("edge was started without a config file".to_string());
        };
        let config = EdgeConfig::from_file(path).map_err(|e| {
            warn!("reload_config: {}: {e}", path.display());
            e.to_string()
        })?;
        info!("Reloaded {}", path.display());
        self.apply_config(ApplyConfig {
            poll_interval_secs: Some(config.api.syscfg.poll_interval_secs),
            thresholds: ThresholdConfig::full(
                config.prediction.temperature_warning,
                config.prediction.temperature_critical,
            ),
        });
        Ok(())
    }
}

/// Hub error codes after which quick reconnects are pointless
fn is_fatal_error_code(code: &str) -> bool {
    let code = code.to_ascii_uppercase();
    code == "VERSION_MISMATCH"
        || code.contains("AUTH")
        || code.contains("FORBIDDEN")
        || code.contains("TOKEN")
}

impl Actor for HubConnectorActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("HubConnectorActor started for edge {}", self.config.edge_id);
        if self.config.auto_connect {
            self.start_connection(ctx);
        }
        let interval = self.config.heartbeat_interval.max(Duration::from_secs(1));
        ctx.run_interval(interval, |actor, _ctx| actor.send_heartbeat());
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        if let Some(task) = self.ws.disconnect() {
            task.abort();
        }
        info!("HubConnectorActor stopped");
    }
}

// ============================================================================
// Message Handlers
// ============================================================================

/// Attach the device manager after both actors have started (avoids the
/// circular constructor dependency).
#[derive(Message)]
#[rtype(result = "()")]
pub struct AttachDeviceManager {
    pub manager: Addr<DeviceManagerActor>,
}

impl Handler<AttachDeviceManager> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: AttachDeviceManager, _ctx: &mut Self::Context) -> Self::Result {
        self.device_manager = Some(msg.manager);
    }
}

/// Start connecting (idempotent: returns false when a connection or
/// reconnect loop is already running, or during shutdown)
#[derive(Message)]
#[rtype(result = "bool")]
pub struct Connect;

impl Handler<Connect> for HubConnectorActor {
    type Result = bool;

    fn handle(&mut self, _msg: Connect, ctx: &mut Self::Context) -> Self::Result {
        self.start_connection(ctx)
    }
}

/// Close the hub connection (close frame) and stop reconnecting. Resolves
/// when the connection task has finished (bounded wait). Buffered
/// messages stay buffered.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Disconnect;

impl Handler<Disconnect> for HubConnectorActor {
    type Result = ResponseFuture<()>;

    fn handle(&mut self, _msg: Disconnect, _ctx: &mut Self::Context) -> Self::Result {
        self.state = ConnectionState::ShuttingDown;
        self.sender = None;
        let task = self.ws.disconnect();
        Box::pin(async move {
            if let Some(task) = task {
                if tokio::time::timeout(DISCONNECT_TIMEOUT, task)
                    .await
                    .is_err()
                {
                    warn!("Hub connection task did not stop in time");
                }
            }
        })
    }
}

impl Handler<WsEvent> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, event: WsEvent, ctx: &mut Self::Context) -> Self::Result {
        let current = self.ws.generation();
        match event {
            WsEvent::Connected {
                generation,
                sender,
                local_ip,
            } => {
                // stale supervisor or shutting down: dropping the sender
                // makes that connection close itself
                if generation == current && self.state != ConnectionState::ShuttingDown {
                    self.on_connected(sender, local_ip);
                }
            }
            WsEvent::Disconnected {
                generation,
                unsent,
                reason,
            } => {
                if generation == current {
                    self.on_disconnected(unsent, &reason);
                } else {
                    self.buffer.requeue_front(unsent);
                }
            }
            WsEvent::Received {
                generation,
                message,
                unknown_type,
            } => {
                if generation == current {
                    self.handle_message(message, unknown_type, ctx);
                }
            }
            WsEvent::Stopped { generation } => {
                if generation == current {
                    self.sender = None;
                    // after Disconnect the connector stays shut down: a
                    // later Connect must not revive it
                    if self.state != ConnectionState::ShuttingDown {
                        self.state = ConnectionState::Disconnected;
                    }
                }
            }
        }
    }
}

/// Handle device status updates
impl Handler<DeviceStatusUpdate> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: DeviceStatusUpdate, _ctx: &mut Self::Context) -> Self::Result {
        // the device is back: a held removal must not overtake this status
        if !self.pending_removals.is_empty() {
            self.pending_removals
                .retain(|p| p.device_id != msg.device_id);
        }
        self.enqueue(WsMessage::device_status(msg), OutboundClass::Droppable);
    }
}

/// Handle device alerts
impl Handler<DeviceAlert> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: DeviceAlert, _ctx: &mut Self::Context) -> Self::Result {
        debug!("Edge alert ({}): {}", msg.severity, msg.message);
        self.enqueue(WsMessage::device_alert(msg), OutboundClass::Priority);
    }
}

/// Handle prediction results
impl Handler<PredictionResult> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: PredictionResult, _ctx: &mut Self::Context) -> Self::Result {
        if msg.probability >= 0.8 {
            info!(
                "High-confidence prediction: {} for device {} (p={:.2}, ETA {:?} min)",
                msg.prediction_type, msg.device_id, msg.probability, msg.eta_minutes
            );
        } else {
            debug!(
                "Prediction {} for device {} (p={:.2})",
                msg.prediction_type, msg.device_id, msg.probability
            );
        }
        self.enqueue(WsMessage::prediction(msg), OutboundClass::Priority);
    }
}

/// Handle device removals
impl Handler<DeviceRemoved> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: DeviceRemoved, _ctx: &mut Self::Context) -> Self::Result {
        self.send_removal(msg);
    }
}

/// Device count and degraded state for heartbeats
#[derive(Message, Debug, Clone)]
#[rtype(result = "()")]
pub struct UpdateDeviceCount {
    pub count: usize,
    /// Why the edge is degraded (None = online)
    pub degraded: Option<String>,
}

impl Handler<UpdateDeviceCount> for HubConnectorActor {
    type Result = ();

    fn handle(&mut self, msg: UpdateDeviceCount, _ctx: &mut Self::Context) -> Self::Result {
        debug!("Device count {} (degraded: {:?})", msg.count, msg.degraded);
        self.device_count = msg.count;
        self.degraded = msg.degraded;
    }
}

/// Get current connection state
#[derive(Message)]
#[rtype(result = "ConnectionState")]
pub struct GetConnectionState;

impl Handler<GetConnectionState> for HubConnectorActor {
    type Result = MessageResult<GetConnectionState>;

    fn handle(&mut self, _msg: GetConnectionState, _ctx: &mut Self::Context) -> Self::Result {
        MessageResult(self.state)
    }
}

/// Number of buffered (not yet sent) messages
#[derive(Message)]
#[rtype(result = "usize")]
pub struct GetBufferedCount;

impl Handler<GetBufferedCount> for HubConnectorActor {
    type Result = usize;

    fn handle(&mut self, _msg: GetBufferedCount, _ctx: &mut Self::Context) -> Self::Result {
        self.buffered()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_core::actor::messages::ActionType;
    use std::collections::HashMap;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn offline_config() -> HubConnectorConfig {
        HubConnectorConfig {
            auto_connect: false,
            edge_id: "e1".into(),
            edge_name: "Edge 1".into(),
            ..HubConnectorConfig::default()
        }
    }

    fn status(device: &str) -> DeviceStatusUpdate {
        DeviceStatusUpdate {
            device_id: device.to_string(),
            edge_id: "e1".to_string(),
            status: nimon_core::HealthStatus::Healthy,
            metrics: HashMap::new(),
            timestamp: chrono::Utc::now(),
            is_simulated: false,
        }
    }

    fn alert(msg: &str) -> DeviceAlert {
        DeviceAlert {
            device_id: "d".to_string(),
            edge_id: "e1".to_string(),
            severity: Severity::Critical,
            message: msg.to_string(),
            metric_name: None,
            metric_value: None,
            timestamp: chrono::Utc::now(),
        }
    }

    async fn drain(rx: &mut UnboundedReceiver<Outbound>) -> Vec<WsMessage> {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut out = Vec::new();
        while let Ok(item) = rx.try_recv() {
            out.push(WsMessage::from_json(&item.json).unwrap());
        }
        out
    }

    async fn connect(addr: &Addr<HubConnectorActor>) -> UnboundedReceiver<Outbound> {
        let (sender, rx) = ConnectionSender::test_pair();
        addr.send(WsEvent::Connected {
            generation: 0,
            sender,
            local_ip: Some("10.1.2.3".parse().unwrap()),
        })
        .await
        .unwrap();
        rx
    }

    #[test]
    fn test_config_default() {
        let config = HubConnectorConfig::default();
        assert!(config.auto_connect);
        assert_eq!(config.ws.hub_url, "ws://localhost:9090/ws");
        assert_eq!(config.heartbeat_interval, Duration::from_secs(30));
        assert_eq!(config.buffer_max_messages, 1000);
    }

    #[test]
    fn test_fatal_error_codes() {
        assert!(is_fatal_error_code("VERSION_MISMATCH"));
        assert!(is_fatal_error_code("UNAUTHORIZED"));
        assert!(is_fatal_error_code("auth_failed"));
        assert!(is_fatal_error_code("INVALID_TOKEN"));
        assert!(!is_fatal_error_code("PARSE_ERROR"));
    }

    #[test]
    fn test_heartbeat_fields() {
        let mut actor = HubConnectorActor::new(offline_config());
        actor.device_count = 3;
        let hb = actor.heartbeat();
        assert_eq!(hb.status, "online");
        assert_eq!(hb.device_count, 3);
        assert_eq!(hb.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
        assert!(hb.uptime_secs.is_some());
        actor.degraded = Some("NI-SysCfg failing".into());
        assert_eq!(actor.heartbeat().status, "degraded");
    }

    #[actix::test]
    async fn test_register_first_then_flush_in_order_with_local_ip() {
        let addr = HubConnectorActor::new(offline_config()).start();
        addr.send(status("d1")).await.unwrap();
        addr.send(alert("a1")).await.unwrap();
        assert_eq!(addr.send(GetBufferedCount).await.unwrap(), 2);

        let mut rx = connect(&addr).await;
        let msgs = drain(&mut rx).await;
        let types: Vec<_> = msgs.iter().map(|m| m.msg_type).collect();
        assert_eq!(
            types,
            vec![
                WsMessageType::EdgeRegister,
                WsMessageType::DeviceStatus,
                WsMessageType::DeviceAlert,
                WsMessageType::Heartbeat
            ]
        );
        let reg: EdgeRegister = msgs[0].payload().unwrap();
        assert_eq!(reg.ip_address.as_deref(), Some("10.1.2.3"));
        assert_eq!(addr.send(GetBufferedCount).await.unwrap(), 0);
        assert_eq!(
            addr.send(GetConnectionState).await.unwrap(),
            ConnectionState::Connected
        );

        // live traffic goes straight out
        addr.send(status("d2")).await.unwrap();
        let msgs = drain(&mut rx).await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].msg_type, WsMessageType::DeviceStatus);
    }

    #[actix::test]
    async fn test_disconnect_requeues_unsent_before_newer() {
        let addr = HubConnectorActor::new(offline_config()).start();
        let rx = connect(&addr).await;
        drop(rx); // the connection is gone: sends now fail
        addr.send(alert("newer")).await.unwrap();
        let unsent = vec![Outbound::new(
            WsMessage::device_alert(alert("older")).to_json().unwrap(),
            OutboundClass::Priority,
        )];
        addr.send(WsEvent::Disconnected {
            generation: 0,
            unsent,
            reason: "test".into(),
        })
        .await
        .unwrap();
        assert_eq!(addr.send(GetBufferedCount).await.unwrap(), 2);
        assert_eq!(
            addr.send(GetConnectionState).await.unwrap(),
            ConnectionState::Reconnecting
        );

        let mut rx = connect(&addr).await;
        let msgs = drain(&mut rx).await;
        let alerts: Vec<String> = msgs
            .iter()
            .filter(|m| m.msg_type == WsMessageType::DeviceAlert)
            .map(|m| m.payload::<DeviceAlert>().unwrap().message)
            .collect();
        assert_eq!(alerts, vec!["older", "newer"]);
        assert_eq!(msgs[0].msg_type, WsMessageType::EdgeRegister);
    }

    #[actix::test]
    async fn test_heartbeats_not_buffered_offline() {
        let mut actor = HubConnectorActor::new(offline_config());
        actor.send_heartbeat();
        assert_eq!(actor.buffered(), 0);
    }

    #[actix::test]
    async fn test_action_result_replies_with_reply_to() {
        let addr = HubConnectorActor::new(offline_config()).start();
        let mut rx = connect(&addr).await;
        let _ = drain(&mut rx).await;

        let request = WsMessage::execute_action(ExecuteAction {
            action_id: "act-1".into(),
            device_id: "e1:nope".into(),
            action_type: ActionType::ResetDriver,
            parameters: HashMap::new(),
        });
        let request_id = request.msg_id.clone();
        addr.send(WsEvent::Received {
            generation: 0,
            message: request,
            unknown_type: None,
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let msgs = drain(&mut rx).await;
        let reply = msgs
            .iter()
            .find(|m| m.msg_type == WsMessageType::ActionResult)
            .expect("action_result sent");
        assert_eq!(reply.reply_to.as_deref(), Some(request_id.as_str()));
        let result: ActionResult = reply.payload().unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not managed"));
        // failures raise an edge alert too
        assert!(msgs
            .iter()
            .any(|m| m.msg_type == WsMessageType::DeviceAlert));
    }

    #[actix::test]
    async fn test_unknown_action_type_gets_failed_reply() {
        let addr = HubConnectorActor::new(offline_config()).start();
        let mut rx = connect(&addr).await;
        let _ = drain(&mut rx).await;

        let mut request = WsMessage::new(
            WsMessageType::ExecuteAction,
            serde_json::json!({
                "action_id": "act-9",
                "device_id": "e1:x",
                "action_type": "self_destruct",
                "parameters": {}
            }),
        );
        request.msg_id = "req-9".into();
        addr.send(WsEvent::Received {
            generation: 0,
            message: request,
            unknown_type: None,
        })
        .await
        .unwrap();
        let msgs = drain(&mut rx).await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].msg_type, WsMessageType::ActionResult);
        assert_eq!(msgs[0].reply_to.as_deref(), Some("req-9"));
        let result: ActionResult = msgs[0].payload().unwrap();
        assert_eq!(result.action_id, "act-9");
        assert!(!result.success);
        assert!(result.error.unwrap().contains("self_destruct"));
    }

    #[actix::test]
    async fn test_hub_commands_are_acked() {
        let addr = HubConnectorActor::new(offline_config()).start();
        let mut rx = connect(&addr).await;
        let _ = drain(&mut rx).await;

        for (cmd, ok) in [
            ("resend_state", true),
            ("reload_config", false),
            ("dance", false),
        ] {
            let msg = WsMessage::hub_command(HubCommand {
                command: cmd.into(),
                parameters: HashMap::new(),
            });
            let id = msg.msg_id.clone();
            addr.send(WsEvent::Received {
                generation: 0,
                message: msg,
                unknown_type: None,
            })
            .await
            .unwrap();
            let msgs = drain(&mut rx).await;
            if cmd == "resend_state" {
                assert_eq!(msgs[0].msg_type, WsMessageType::EdgeRegister);
            }
            let ack = msgs
                .iter()
                .find(|m| m.msg_type == WsMessageType::Ack)
                .unwrap();
            assert_eq!(ack.reply_to.as_deref(), Some(id.as_str()));
            let payload: nimon_core::protocol::AckMessage = ack.payload().unwrap();
            assert_eq!(payload.success, ok, "{cmd}");
        }
    }

    #[actix::test]
    async fn test_reload_config_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edge.yaml");
        std::fs::write(&path, "node: { id: e1, name: n, hub_address: \"h:1\" }\n").unwrap();
        let config = HubConnectorConfig {
            config_path: Some(path),
            ..offline_config()
        };
        let actor = HubConnectorActor::new(config);
        assert!(actor.reload_config().is_ok());
        let bad = HubConnectorActor::new(HubConnectorConfig {
            config_path: Some(dir.path().join("missing.yaml")),
            ..offline_config()
        });
        assert!(bad.reload_config().is_err());
    }

    #[actix::test]
    async fn test_ping_answered_and_stale_generation_ignored() {
        let addr = HubConnectorActor::new(offline_config()).start();
        let mut rx = connect(&addr).await;
        let _ = drain(&mut rx).await;
        let ping = WsMessage::ping();
        addr.send(WsEvent::Received {
            generation: 0,
            message: ping,
            unknown_type: None,
        })
        .await
        .unwrap();
        // a stale generation's frames are ignored
        addr.send(WsEvent::Received {
            generation: 99,
            message: WsMessage::ping(),
            unknown_type: None,
        })
        .await
        .unwrap();
        let msgs = drain(&mut rx).await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].msg_type, WsMessageType::Pong);
    }

    fn removed(device: &str) -> DeviceRemoved {
        DeviceRemoved {
            edge_id: "e1".into(),
            device_id: device.into(),
            timestamp: chrono::Utc::now(),
        }
    }

    fn hub_msg(version: &str) -> WsEvent {
        let mut message = WsMessage::ack("x".into(), true, None);
        message.version = version.into();
        WsEvent::Received {
            generation: 0,
            message,
            unknown_type: None,
        }
    }

    #[test]
    fn test_parse_protocol_version() {
        assert_eq!(parse_protocol_version("1.1"), Some((1, 1)));
        assert_eq!(parse_protocol_version("1.0"), Some((1, 0)));
        assert_eq!(parse_protocol_version("2"), Some((2, 0)));
        assert_eq!(parse_protocol_version("1.10.3"), Some((1, 10)));
        assert_eq!(parse_protocol_version("x"), None);
    }

    #[actix::test]
    async fn test_device_removed_held_until_hub_speaks_1_1() {
        let addr = HubConnectorActor::new(offline_config()).start();
        let mut rx = connect(&addr).await;
        let _ = drain(&mut rx).await;
        addr.send(removed("e1:gone")).await.unwrap();
        // re-appearing device cancels its held removal
        addr.send(removed("e1:back")).await.unwrap();
        addr.send(status("e1:back")).await.unwrap();
        let msgs = drain(&mut rx).await;
        assert!(
            !msgs
                .iter()
                .any(|m| m.msg_type == WsMessageType::DeviceRemoved),
            "hub version unknown: removal held"
        );
        // the hub's registration ack reveals protocol 1.1
        addr.send(hub_msg("1.1")).await.unwrap();
        let msgs = drain(&mut rx).await;
        let gone: Vec<String> = msgs
            .iter()
            .filter(|m| m.msg_type == WsMessageType::DeviceRemoved)
            .map(|m| m.payload::<DeviceRemoved>().unwrap().device_id)
            .collect();
        assert_eq!(gone, vec!["e1:gone"]);
        // later removals go straight out
        addr.send(removed("e1:later")).await.unwrap();
        let msgs = drain(&mut rx).await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].msg_type, WsMessageType::DeviceRemoved);
    }

    #[actix::test]
    async fn test_device_removed_not_sent_to_1_0_hub() {
        let addr = HubConnectorActor::new(offline_config()).start();
        let mut rx = connect(&addr).await;
        let _ = drain(&mut rx).await;
        addr.send(removed("e1:held")).await.unwrap();
        addr.send(hub_msg("1.0")).await.unwrap();
        addr.send(removed("e1:after")).await.unwrap();
        let msgs = drain(&mut rx).await;
        assert!(
            !msgs
                .iter()
                .any(|m| m.msg_type == WsMessageType::DeviceRemoved),
            "{:?}",
            msgs.iter().map(|m| m.msg_type).collect::<Vec<_>>()
        );
    }

    #[actix::test]
    async fn test_disconnect_message_sets_shutdown() {
        // a real supervisor (retrying against a closed port), so its
        // Stopped event arrives after Disconnect
        let config = HubConnectorConfig {
            ws: WsClientConfig {
                hub_url: "ws://127.0.0.1:1/ws".into(),
                reconnect_delay: Duration::from_millis(100),
                connect_timeout: Duration::from_millis(500),
                ..WsClientConfig::default()
            },
            ..offline_config()
        };
        let addr = HubConnectorActor::new(config).start();
        assert!(addr.send(Connect).await.unwrap(), "supervisor started");
        tokio::time::sleep(Duration::from_millis(200)).await;
        addr.send(Disconnect).await.unwrap();
        // let the supervisor's Stopped event be processed
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            addr.send(GetConnectionState).await.unwrap(),
            ConnectionState::ShuttingDown
        );
        assert!(
            !addr.send(Connect).await.unwrap(),
            "no connect after shutdown"
        );
        assert_eq!(
            addr.send(GetConnectionState).await.unwrap(),
            ConnectionState::ShuttingDown
        );
    }
}
