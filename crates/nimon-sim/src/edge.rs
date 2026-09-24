//! One simulated edge = one task with its own WebSocket connection.
//!
//! Per connection a writer task drains an outbound queue and a reader
//! task handles every incoming hub message immediately; the edge task
//! drives the sweep ticks and reconnects with exponential backoff.

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use nimon_core::actor::messages::{ConfigUpdate, ExecuteAction};
use nimon_core::protocol::{
    AckMessage, ErrorMessage, HubCommand, PingMessage, WsMessage, WsMessageType, PROTOCOL_VERSION,
};
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch, Notify};
use tokio::task::JoinHandle;
use tokio::time::{sleep, sleep_until, timeout, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{header::AUTHORIZATION, HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, info, warn};

use crate::actions::{backoff_delay, is_fatal_error_code, plan_action_reply, ActionPlan};
use crate::cli::{Scenario, SimConfig};
use crate::model::{EdgeSim, RECONNECT_EVERY_TICKS};
use crate::stats::{self, type_name, Stats};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

const BACKOFF_BASE: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Old socket stays open this long after the replacement registered
/// (reconnect scenario, overlapping cycles)
const OVERLAP_GRACE: Duration = Duration::from_secs(2);

/// Global run state shared by all edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    Running,
    Stopping,
    Fatal(String),
}

/// Everything an edge task needs from the run.
#[derive(Clone)]
pub struct Shared {
    pub cfg: Arc<SimConfig>,
    pub stats: Arc<Stats>,
    pub run: Arc<watch::Sender<RunState>>,
}

impl Shared {
    /// Record a fatal error (first one wins) so `main` exits non-zero.
    pub fn fatal(&self, reason: String) {
        self.run.send_if_modified(|s| {
            if *s == RunState::Running {
                *s = RunState::Fatal(reason);
                true
            } else {
                false
            }
        });
    }
}

fn lock(sim: &Mutex<EdgeSim>) -> MutexGuard<'_, EdgeSim> {
    // a panicked holder leaves consistent-enough state for a simulator
    sim.lock().unwrap_or_else(|e| e.into_inner())
}

enum Outbound {
    Msg(WsMessage),
    Close,
}

/// A live connection: outbound queue plus reader/writer tasks.
struct Connection {
    tx: mpsc::UnboundedSender<Outbound>,
    dead: watch::Receiver<bool>,
    writer: JoinHandle<()>,
    reader: JoinHandle<()>,
}

impl Connection {
    fn start(ws: WsStream, edge: EdgeCtx) -> Self {
        let (sink, stream) = ws.split();
        let (tx, rx) = mpsc::unbounded_channel();
        let (dead_tx, dead) = watch::channel(false);
        let dead_tx = Arc::new(dead_tx);
        let stats = edge.shared.stats.clone();
        let writer = tokio::spawn(write_loop(sink, rx, stats, dead_tx.clone()));
        let ctx = ReaderCtx {
            edge,
            tx: tx.clone(),
        };
        let reader = tokio::spawn(read_loop(stream, ctx, dead_tx));
        Self {
            tx,
            dead,
            writer,
            reader,
        }
    }

    fn send(&self, msg: WsMessage) {
        let _ = self.tx.send(Outbound::Msg(msg));
    }

    /// Resolves once the reader or writer has stopped.
    async fn closed(&mut self) {
        let _ = self.dead.wait_for(|d| *d).await;
    }

    /// Flush queued messages, send a close frame, stop the tasks.
    async fn close(self) {
        let _ = self.tx.send(Outbound::Close);
        let _ = timeout(Duration::from_secs(2), self.writer).await;
        self.reader.abort();
    }
}

async fn write_loop(
    mut sink: SplitSink<WsStream, Message>,
    mut rx: mpsc::UnboundedReceiver<Outbound>,
    stats: Arc<Stats>,
    dead: Arc<watch::Sender<bool>>,
) {
    while let Some(out) = rx.recv().await {
        match out {
            Outbound::Msg(msg) => {
                let json = match msg.to_json() {
                    Ok(j) => j,
                    Err(e) => {
                        warn!("failed to serialize {:?}: {e}", msg.msg_type);
                        continue;
                    }
                };
                if let Err(e) = sink.send(Message::Text(json)).await {
                    debug!("send failed: {e}");
                    break;
                }
                stats.record_sent(msg.msg_type);
                debug!(msg_type = %type_name(msg.msg_type), "sent");
            }
            Outbound::Close => {
                let _ = sink.send(Message::Close(None)).await;
                let _ = sink.close().await;
                break;
            }
        }
    }
    dead.send_replace(true);
}

/// Per-edge state that outlives individual connections.
#[derive(Clone)]
struct EdgeCtx {
    edge_id: String,
    sim: Arc<Mutex<EdgeSim>>,
    shared: Shared,
    /// Wakes the tick loop when the poll interval changes
    wake: Arc<Notify>,
}

/// State the reader needs to answer hub messages.
struct ReaderCtx {
    edge: EdgeCtx,
    tx: mpsc::UnboundedSender<Outbound>,
}

async fn read_loop(
    mut stream: SplitStream<WsStream>,
    ctx: ReaderCtx,
    dead: Arc<watch::Sender<bool>>,
) {
    while let Some(frame) = stream.next().await {
        match frame {
            Ok(Message::Text(text)) => handle_incoming(&text, &ctx),
            Ok(Message::Binary(bytes)) => match std::str::from_utf8(&bytes) {
                Ok(text) => handle_incoming(text, &ctx),
                Err(_) => warn!(edge = %ctx.edge.edge_id, "ignoring non-UTF8 binary frame"),
            },
            Ok(Message::Close(frame)) => {
                info!(edge = %ctx.edge.edge_id, "hub closed the connection: {frame:?}");
                break;
            }
            // ws-level ping/pong are answered by tungstenite
            Ok(_) => {}
            Err(e) => {
                warn!(edge = %ctx.edge.edge_id, "websocket error: {e}");
                break;
            }
        }
    }
    dead.send_replace(true);
}

fn handle_incoming(text: &str, ctx: &ReaderCtx) {
    let edge = ctx.edge.edge_id.as_str();
    let counters = &ctx.edge.shared.stats;
    let msg = match WsMessage::from_json(text) {
        Ok(m) => m,
        Err(e) => {
            stats::inc(&counters.parse_errors);
            warn!(edge, "unparseable message from hub: {e}");
            return;
        }
    };
    counters.record_received(msg.msg_type);
    debug!(edge, msg_type = %type_name(msg.msg_type), msg_id = %msg.msg_id, "received");
    if !msg.version_compatible() {
        warn!(
            edge,
            "hub speaks protocol {} (ours {PROTOCOL_VERSION})", msg.version
        );
    }
    let send = |m: WsMessage| {
        let _ = ctx.tx.send(Outbound::Msg(m));
    };

    match msg.msg_type {
        WsMessageType::ConfigUpdate => match msg.payload::<ConfigUpdate>() {
            Ok(update) => {
                let change = lock(&ctx.edge.sim).apply_config(&update);
                info!(
                    edge,
                    "config_update: thresholds {:?} -> {:?}, interval {:?} -> {:?}",
                    change.old_thresholds,
                    change.new_thresholds,
                    change.old_interval,
                    change.new_interval
                );
                if change.interval_changed() {
                    ctx.edge.wake.notify_one();
                }
            }
            Err(e) => warn!(edge, "bad config_update payload: {e}"),
        },
        WsMessageType::ExecuteAction => match msg.payload::<ExecuteAction>() {
            Ok(action) => {
                stats::inc(&counters.actions_received);
                let known = lock(&ctx.edge.sim).has_device(&action.device_id);
                if !known {
                    warn!(edge, device = %action.device_id, "execute_action for unknown device");
                }
                let mode = ctx.edge.shared.cfg.action_mode;
                info!(
                    edge,
                    action_id = %action.action_id,
                    device = %action.device_id,
                    "execute_action {} (mode {mode})",
                    action.action_type
                );
                match plan_action_reply(mode, &msg.msg_id, &action, known) {
                    ActionPlan::Withhold => {
                        stats::inc(&counters.actions_withheld);
                        warn!(edge, action_id = %action.action_id, "timeout mode: not replying");
                    }
                    ActionPlan::Reply { after, reply } => {
                        let tx = ctx.tx.clone();
                        let counters = counters.clone();
                        tokio::spawn(async move {
                            sleep(after).await;
                            if tx.send(Outbound::Msg(reply)).is_ok() {
                                stats::inc(&counters.actions_replied);
                            }
                        });
                    }
                }
            }
            Err(e) => warn!(edge, "bad execute_action payload: {e}"),
        },
        WsMessageType::HubCommand => match msg.payload::<HubCommand>() {
            Ok(cmd) if cmd.command == "resend_state" => {
                let msgs = lock(&ctx.edge.sim).state_snapshot();
                info!(
                    edge,
                    "hub_command resend_state: re-sending {} messages",
                    msgs.len()
                );
                msgs.into_iter().for_each(send);
            }
            Ok(cmd) => info!(
                edge,
                "ignoring hub_command '{}' {:?}", cmd.command, cmd.parameters
            ),
            Err(e) => warn!(edge, "bad hub_command payload: {e}"),
        },
        WsMessageType::Ack => match msg.payload::<AckMessage>() {
            Ok(ack) if ack.success => debug!(edge, "ack for {}", ack.original_msg_id),
            Ok(ack) => warn!(
                edge,
                "negative ack for {}: {:?}", ack.original_msg_id, ack.error
            ),
            Err(e) => warn!(edge, "bad ack payload: {e}"),
        },
        WsMessageType::Error => {
            stats::inc(&counters.hub_errors);
            match msg.payload::<ErrorMessage>() {
                Ok(err) => {
                    warn!(edge, code = %err.code, "hub error: {} {:?}", err.message, err.details);
                    if is_fatal_error_code(&err.code) {
                        ctx.edge
                            .shared
                            .fatal(format!("{edge}: hub error {}: {}", err.code, err.message));
                    }
                }
                Err(e) => warn!(edge, "bad error payload: {e}"),
            }
        }
        WsMessageType::Ping => {
            let ts = msg
                .payload::<PingMessage>()
                .map(|p| p.timestamp)
                .unwrap_or(msg.timestamp);
            send(WsMessage::pong(ts));
        }
        WsMessageType::Pong => debug!(edge, "pong"),
        WsMessageType::Unknown => info!(edge, "ignoring unknown message type (newer hub?)"),
        other => debug!(edge, "unexpected {} from hub", type_name(other)),
    }
}

enum ConnectError {
    Fatal(String),
    Retry(String),
}

async fn connect(url: &str, token: Option<&str>) -> Result<WsStream, ConnectError> {
    let mut req = url
        .into_client_request()
        .map_err(|e| ConnectError::Fatal(format!("invalid hub url '{url}': {e}")))?;
    if let Some(token) = token {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| ConnectError::Fatal("token is not a valid header value".into()))?;
        req.headers_mut().insert(AUTHORIZATION, value);
    }
    match timeout(CONNECT_TIMEOUT, connect_async(req)).await {
        Err(_) => Err(ConnectError::Retry("connect timed out".into())),
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(WsError::Http(resp)))
            if matches!(
                resp.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ) =>
        {
            Err(ConnectError::Fatal(format!(
                "hub rejected the handshake with HTTP {} (check --token / NIMON_EDGE_TOKEN)",
                resp.status()
            )))
        }
        Ok(Err(WsError::Http(resp))) => Err(ConnectError::Retry(format!(
            "handshake HTTP {}",
            resp.status()
        ))),
        Ok(Err(e)) => Err(ConnectError::Retry(e.to_string())),
    }
}

enum SessionEnd {
    Stop,
    Dropped,
    PlannedReconnect,
}

fn is_running(rx: &watch::Receiver<RunState>) -> bool {
    *rx.borrow() == RunState::Running
}

/// Drive one simulated edge until the run stops.
pub async fn run_edge(index: usize, shared: Shared) {
    let cfg = shared.cfg.clone();
    let sim = Arc::new(Mutex::new(EdgeSim::new(
        index,
        &cfg.edge_prefix,
        cfg.devices,
        cfg.scenario,
        cfg.seed,
        cfg.interval,
    )));
    let edge_id = lock(&sim).edge_id.clone();
    let started = Instant::now();
    let wake = Arc::new(Notify::new());
    let mut run_rx = shared.run.subscribe();
    let mut backoff_rng = StdRng::seed_from_u64(cfg.seed.wrapping_add(index as u64));
    let mut attempt: u32 = 0;
    let mut cycle: u64 = 0;

    info!(edge = %edge_id, devices = cfg.devices, "edge starting");

    while is_running(&run_rx) {
        let result = tokio::select! {
            r = connect(&cfg.url, cfg.token.as_deref()) => r,
            _ = run_rx.changed() => continue,
        };
        let ws = match result {
            Ok(ws) => ws,
            Err(ConnectError::Fatal(reason)) => {
                shared.fatal(format!("{edge_id}: {reason}"));
                break;
            }
            Err(ConnectError::Retry(reason)) => {
                stats::inc(&shared.stats.connect_failures);
                let delay = backoff_delay(attempt, BACKOFF_BASE, BACKOFF_MAX, &mut backoff_rng);
                attempt = attempt.saturating_add(1);
                warn!(edge = %edge_id, "connect to {} failed: {reason}; retry in {delay:.1?}", cfg.url);
                tokio::select! {
                    _ = sleep(delay) => {}
                    _ = run_rx.changed() => {}
                }
                continue;
            }
        };

        stats::inc(&shared.stats.connects);
        stats::inc(&shared.stats.connected);
        info!(edge = %edge_id, "connected to {}", cfg.url);
        let ctx = EdgeCtx {
            edge_id: edge_id.clone(),
            sim: sim.clone(),
            shared: shared.clone(),
            wake: wake.clone(),
        };
        let mut conn = Connection::start(ws, ctx);
        conn.send(lock(&sim).register());

        let reconnect_after = (cfg.scenario == Scenario::Reconnect)
            .then(|| RECONNECT_EVERY_TICKS + (index as u64 % 3));
        let (end, ticks) = session(
            &mut conn,
            &sim,
            &wake,
            &mut run_rx,
            started,
            reconnect_after,
        )
        .await;
        stats::dec(&shared.stats.connected);
        if ticks >= 2 {
            attempt = 0;
        }

        match end {
            SessionEnd::Stop => {
                conn.close().await;
                break;
            }
            SessionEnd::Dropped => {
                stats::inc(&shared.stats.disconnects);
                conn.close().await;
                let delay = backoff_delay(attempt, BACKOFF_BASE, BACKOFF_MAX, &mut backoff_rng);
                attempt = attempt.saturating_add(1);
                warn!(edge = %edge_id, "connection lost; reconnecting in {delay:.1?}");
                tokio::select! {
                    _ = sleep(delay) => {}
                    _ = run_rx.changed() => {}
                }
            }
            SessionEnd::PlannedReconnect => {
                stats::inc(&shared.stats.planned_reconnects);
                cycle += 1;
                if cycle % 2 == 1 {
                    // Overlap: register the new session while the old
                    // socket is still open, so the hub must replace it.
                    info!(edge = %edge_id, "reconnect scenario: overlapping re-register");
                    tokio::spawn(async move {
                        sleep(OVERLAP_GRACE).await;
                        conn.close().await;
                    });
                } else {
                    info!(edge = %edge_id, "reconnect scenario: clean close + reconnect");
                    conn.close().await;
                    sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }
    info!(edge = %edge_id, "edge stopped");
}

/// Tick loop for one connection. Returns how it ended and the number of
/// ticks completed.
async fn session(
    conn: &mut Connection,
    sim: &Mutex<EdgeSim>,
    wake: &Notify,
    run_rx: &mut watch::Receiver<RunState>,
    started: Instant,
    reconnect_after: Option<u64>,
) -> (SessionEnd, u64) {
    let mut ticks = 0u64;
    let mut last_tick = Instant::now();
    let mut next = last_tick; // first sweep immediately after register

    loop {
        tokio::select! {
            _ = sleep_until(next) => {
                let uptime = started.elapsed().as_secs();
                let (msgs, heartbeat, interval) = {
                    let mut s = lock(sim);
                    let msgs = s.step();
                    (msgs, s.heartbeat(uptime), s.interval)
                };
                msgs.into_iter().for_each(|m| conn.send(m));
                conn.send(heartbeat);
                ticks += 1;
                last_tick = Instant::now();
                // keep cadence, but never try to "catch up" a backlog
                next = (next + interval).max(last_tick);
                if reconnect_after.is_some_and(|n| ticks >= n) {
                    return (SessionEnd::PlannedReconnect, ticks);
                }
            }
            _ = wake.notified() => {
                next = last_tick + lock(sim).interval;
            }
            _ = conn.closed() => return (SessionEnd::Dropped, ticks),
            _ = run_rx.changed() => {
                if !is_running(run_rx) {
                    return (SessionEnd::Stop, ticks);
                }
            }
        }
    }
}
