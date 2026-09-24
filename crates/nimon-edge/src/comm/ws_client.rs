//! WebSocket transport to the hub
//!
//! One supervisor task per client owns the whole connection lifecycle:
//! connect (with optional `Authorization: Bearer` header, `ws://` or
//! `wss://`), run the session, and reconnect with exponential backoff and
//! jitter. Everything else talks to it through [`WsEvent`] messages
//! (delivered to an actix recipient, normally the hub connector) and the
//! per-connection [`ConnectionSender`].
//!
//! The client never buffers: messages that could not be written when a
//! connection drops are handed back in [`WsEvent::Disconnected`] so the
//! single offline buffer (owned by the hub connector) keeps them in order.

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use actix::prelude::*;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{header::AUTHORIZATION, HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::Message as Frame;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, info, warn};

use super::buffer::{Outbound, OutboundClass};
use super::message::{WsMessage, WsMessageType};

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Most frames written before one flush
const MAX_BATCH: usize = 64;
/// Grace period for the close handshake on shutdown
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// Log an info summary every N consecutive failed connection attempts
const FAILURE_SUMMARY_EVERY: u32 = 30;
/// Socket write bound when pings are disabled (otherwise 3 ping intervals)
const WRITE_TIMEOUT_NO_PING: Duration = Duration::from_secs(90);

fn write_stalled(what: &str, after: Duration) -> String {
    format!(
        "{what} stalled for {:.1}s (hub not reading); dropping the connection",
        after.as_secs_f64()
    )
}

/// Connection state as seen by the hub connector
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    ShuttingDown,
}

impl ConnectionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionState::Disconnected => "disconnected",
            ConnectionState::Connecting => "connecting",
            ConnectionState::Connected => "connected",
            ConnectionState::Reconnecting => "reconnecting",
            ConnectionState::ShuttingDown => "shutting_down",
        }
    }
}

/// Configuration for the WebSocket client
#[derive(Debug, Clone)]
pub struct WsClientConfig {
    /// Hub WebSocket URL (`ws://` or `wss://`)
    pub hub_url: String,
    /// Bearer token for the handshake
    pub auth_token: Option<String>,
    /// First reconnect delay; doubles per failure
    pub reconnect_delay: Duration,
    /// Reconnect delay cap (also used after auth / version rejections)
    pub max_reconnect_delay: Duration,
    /// WebSocket ping interval (`ZERO` disables pings and half-open
    /// detection)
    pub ping_interval: Duration,
    /// TCP + TLS + handshake timeout
    pub connect_timeout: Duration,
    /// A connection that lived this long resets the backoff
    pub stable_after: Duration,
}

impl Default for WsClientConfig {
    fn default() -> Self {
        Self {
            hub_url: "ws://localhost:9090/ws".to_string(),
            auth_token: None,
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_delay: Duration::from_secs(60),
            ping_interval: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            stable_after: Duration::from_secs(60),
        }
    }
}

/// Events from the supervisor task. `generation` identifies the
/// [`WsClient::connect`] call that produced them.
#[derive(Debug, Message)]
#[rtype(result = "()")]
pub enum WsEvent {
    /// A connection is up; write through `sender` (register first)
    Connected {
        generation: u64,
        sender: ConnectionSender,
        local_ip: Option<IpAddr>,
    },
    /// The connection dropped; `unsent` were accepted but not written
    Disconnected {
        generation: u64,
        unsent: Vec<Outbound>,
        reason: String,
    },
    /// A frame from the hub. `unknown_type` carries the raw type name when
    /// `message.msg_type` is `Unknown`.
    Received {
        generation: u64,
        message: WsMessage,
        unknown_type: Option<String>,
    },
    /// The supervisor exited (after `disconnect()`); no reconnects follow
    Stopped { generation: u64 },
}

/// Write handle for one live connection
#[derive(Debug, Clone)]
pub struct ConnectionSender {
    tx: mpsc::UnboundedSender<Outbound>,
}

impl ConnectionSender {
    /// Queue a frame; gives it back when the connection is already gone
    pub fn send(&self, item: Outbound) -> Result<(), Outbound> {
        self.tx.send(item).map_err(|e| e.0)
    }

    /// Channel pair for tests
    #[cfg(test)]
    pub(crate) fn test_pair() -> (Self, mpsc::UnboundedReceiver<Outbound>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

struct Running {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

/// Handle to the connection supervisor
pub struct WsClient {
    config: Arc<WsClientConfig>,
    generation: u64,
    running: Option<Running>,
    max_backoff: Arc<AtomicBool>,
}

impl WsClient {
    pub fn new(config: WsClientConfig) -> Self {
        Self {
            config: Arc::new(config),
            generation: 0,
            running: None,
            max_backoff: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn config(&self) -> &WsClientConfig {
        &self.config
    }

    /// Generation of the current (or last) supervisor
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// True while a supervisor task is alive (connected or retrying)
    pub fn is_running(&self) -> bool {
        self.running.as_ref().is_some_and(|r| !r.task.is_finished())
    }

    /// Start the supervisor. Idempotent: returns `false` (and does
    /// nothing) when one is already running. Must be called inside a
    /// tokio/actix runtime.
    pub fn connect(&mut self, sink: Recipient<WsEvent>) -> bool {
        if self.is_running() {
            return false;
        }
        self.generation += 1;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = actix_rt::spawn(supervise(
            self.config.clone(),
            self.generation,
            sink,
            shutdown_rx,
            self.max_backoff.clone(),
        ));
        self.running = Some(Running {
            shutdown: shutdown_tx,
            task,
        });
        true
    }

    /// Ask the supervisor to close the connection (close frame) and stop
    /// reconnecting. Returns the task handle to await completion.
    pub fn disconnect(&mut self) -> Option<JoinHandle<()>> {
        let running = self.running.take()?;
        let _ = running.shutdown.send(());
        Some(running.task)
    }

    /// Use the maximum delay before the next reconnect attempt (the hub
    /// rejected us for a reason retrying quickly won't fix)
    pub fn request_max_backoff(&self) {
        self.max_backoff.store(true, Ordering::Relaxed);
    }
}

/// Exponential backoff with "equal jitter": each delay is uniformly
/// distributed in `[d/2, d]`, `d` doubling from `base` up to `max`.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    current: Duration,
    rng: u64,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration, seed: u64) -> Self {
        let base = base.max(Duration::from_millis(100));
        let max = max.max(base);
        Self {
            base,
            max,
            current: base,
            rng: seed | 1,
        }
    }

    fn jitter(&mut self, d: Duration) -> Duration {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let r = self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
        let frac = (r >> 11) as f64 / (1u64 << 53) as f64;
        d.mul_f64(0.5 + 0.5 * frac)
    }

    /// Next delay; doubles the base for the following call
    pub fn next_delay(&mut self) -> Duration {
        let d = self.current;
        self.current = (self.current * 2).min(self.max);
        self.jitter(d)
    }

    /// A delay at the cap (and stay there)
    pub fn max_delay(&mut self) -> Duration {
        self.current = self.max;
        let max = self.max;
        self.jitter(max)
    }

    pub fn reset(&mut self) {
        self.current = self.base;
    }
}

fn seed_from_url(url: &str) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    url.bytes().fold(nanos ^ 0x9E37_79B9_7F4A_7C15, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01B3)
    })
}

/// True for handshake rejections that retrying soon will not fix
fn is_rejection(error: &WsError) -> bool {
    matches!(error, WsError::Http(resp)
        if resp.status() == StatusCode::UNAUTHORIZED || resp.status() == StatusCode::FORBIDDEN)
}

/// Build the handshake request (auth header included)
pub fn build_request(
    config: &WsClientConfig,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    let mut request = config
        .hub_url
        .as_str()
        .into_client_request()
        .map_err(|e| format!("invalid hub URL '{}': {e}", config.hub_url))?;
    if let Some(token) = &config.auth_token {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| "hub token contains characters not allowed in a header".to_string())?;
        request.headers_mut().insert(AUTHORIZATION, value);
    }
    Ok(request)
}

fn local_ip(ws: &WsStream) -> Option<IpAddr> {
    match ws.get_ref() {
        MaybeTlsStream::Plain(s) => s.local_addr().ok().map(|a| a.ip()),
        MaybeTlsStream::Rustls(s) => s.get_ref().0.local_addr().ok().map(|a| a.ip()),
        _ => None,
    }
}

async fn supervise(
    config: Arc<WsClientConfig>,
    generation: u64,
    sink: Recipient<WsEvent>,
    mut shutdown: oneshot::Receiver<()>,
    max_backoff: Arc<AtomicBool>,
) {
    let mut backoff = Backoff::new(
        config.reconnect_delay,
        config.max_reconnect_delay,
        seed_from_url(&config.hub_url),
    );
    let mut failures: u32 = 0;
    let mut first = true;

    'outer: loop {
        if !first {
            let delay = if max_backoff.swap(false, Ordering::Relaxed) {
                backoff.max_delay()
            } else {
                backoff.next_delay()
            };
            debug!("Reconnecting to hub in {:.1}s", delay.as_secs_f64());
            tokio::select! {
                _ = &mut shutdown => break 'outer,
                _ = tokio::time::sleep(delay) => {}
            }
        }
        first = false;

        let request = match build_request(&config) {
            Ok(r) => r,
            Err(e) => {
                // configuration problem: retry slowly
                if failures == 0 {
                    warn!("Cannot connect to hub: {e}");
                }
                failures += 1;
                max_backoff.store(true, Ordering::Relaxed);
                continue;
            }
        };

        let attempt = tokio::time::timeout(config.connect_timeout, connect_async(request));
        let result = tokio::select! {
            _ = &mut shutdown => break 'outer,
            r = attempt => r,
        };
        let ws = match result {
            Ok(Ok((ws, _response))) => ws,
            Ok(Err(e)) => {
                if is_rejection(&e) {
                    warn!("Hub rejected the connection ({e}); check node.hub_token");
                    max_backoff.store(true, Ordering::Relaxed);
                } else {
                    log_failure(failures, &config.hub_url, &e.to_string());
                }
                failures += 1;
                continue;
            }
            Err(_) => {
                log_failure(failures, &config.hub_url, "connect timed out");
                failures += 1;
                continue;
            }
        };

        if failures > 0 {
            info!(
                "Connected to hub at {} after {failures} failed attempt(s)",
                config.hub_url
            );
        } else {
            info!("Connected to hub at {}", config.hub_url);
        }
        failures = 0;

        let (tx, rx) = mpsc::unbounded_channel();
        sink.do_send(WsEvent::Connected {
            generation,
            sender: ConnectionSender { tx },
            local_ip: local_ip(&ws),
        });

        let started = Instant::now();
        let outcome = run_session(ws, rx, &sink, generation, &config, &mut shutdown).await;
        if started.elapsed() >= config.stable_after {
            backoff.reset();
        }
        if outcome.shutdown {
            info!("Hub connection closed ({})", outcome.reason);
        } else {
            warn!("Hub connection lost: {}", outcome.reason);
        }
        sink.do_send(WsEvent::Disconnected {
            generation,
            unsent: outcome.unsent,
            reason: outcome.reason,
        });
        if outcome.shutdown {
            break;
        }
    }
    sink.do_send(WsEvent::Stopped { generation });
}

fn log_failure(previous_failures: u32, url: &str, error: &str) {
    if previous_failures == 0 {
        warn!("Hub connection to {url} failed: {error} (retrying with backoff)");
    } else if (previous_failures + 1).is_multiple_of(FAILURE_SUMMARY_EVERY) {
        info!(
            "Hub still unreachable at {url} after {} attempts: {error}",
            previous_failures + 1
        );
    } else {
        debug!("Hub connection attempt failed: {error}");
    }
}

struct SessionOutcome {
    unsent: Vec<Outbound>,
    reason: String,
    shutdown: bool,
}

/// Extract `"type"` from a raw frame (only used for unknown types)
fn raw_type_name(text: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct TypeOnly {
        #[serde(rename = "type")]
        msg_type: String,
    }
    serde_json::from_str::<TypeOnly>(text)
        .ok()
        .map(|t| t.msg_type)
}

async fn run_session(
    ws: WsStream,
    mut rx: mpsc::UnboundedReceiver<Outbound>,
    sink: &Recipient<WsEvent>,
    generation: u64,
    config: &WsClientConfig,
    shutdown: &mut oneshot::Receiver<()>,
) -> SessionOutcome {
    let (mut write, mut read) = ws.split();
    let mut unsent: Vec<Outbound> = Vec::new();
    // priority frames of the current batch, kept until the flush succeeds
    let mut in_flight: Vec<Outbound> = Vec::new();

    let ping_enabled = !config.ping_interval.is_zero();
    let ping_period = if ping_enabled {
        config.ping_interval
    } else {
        Duration::from_secs(3600)
    };
    let dead_after = ping_period * 3;
    // Every socket write is bounded: a hub that stopped reading (full send
    // buffer) must not block this loop, which also does the reads
    // (execute_action), the half-open check and shutdown. Expiry is a lost
    // connection; unsent/in-flight priority frames are handed back.
    let write_timeout = if ping_enabled {
        dead_after
    } else {
        WRITE_TIMEOUT_NO_PING
    };
    let mut ping = tokio::time::interval_at(Instant::now() + ping_period, ping_period);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_rx = Instant::now();

    let mut shutdown_requested = false;
    let reason: String = loop {
        tokio::select! {
            biased;
            _ = &mut *shutdown => {
                shutdown_requested = true;
                let _ = tokio::time::timeout(CLOSE_TIMEOUT, write.send(Frame::Close(None))).await;
                break "shutdown requested".to_string();
            }
            item = rx.recv() => {
                let Some(first) = item else {
                    // the owner dropped every sender: nothing left to do
                    shutdown_requested = true;
                    let _ = tokio::time::timeout(CLOSE_TIMEOUT, write.send(Frame::Close(None))).await;
                    break "connection owner stopped".to_string();
                };
                let mut next = Some(first);
                let mut batch = 0;
                let mut failed = None;
                while let Some(item) = next.take() {
                    let backup = (item.class == OutboundClass::Priority).then(|| item.clone());
                    let fed = tokio::time::timeout(write_timeout, write.feed(Frame::Text(item.json))).await;
                    let error = match fed {
                        Ok(Ok(())) => None,
                        Ok(Err(e)) => Some(format!("send failed: {e}")),
                        Err(_) => Some(write_stalled("send", write_timeout)),
                    };
                    if let Some(e) = error {
                        // the frame never (fully) left: keep it if it
                        // matters (status frames are superseded by the
                        // next sweep)
                        if let Some(b) = backup {
                            unsent.push(b);
                        }
                        failed = Some(e);
                        break;
                    }
                    if let Some(b) = backup {
                        in_flight.push(b);
                    }
                    batch += 1;
                    if batch < MAX_BATCH {
                        next = rx.try_recv().ok();
                    }
                }
                if failed.is_none() {
                    match tokio::time::timeout(write_timeout, write.flush()).await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => failed = Some(format!("flush failed: {e}")),
                        Err(_) => failed = Some(write_stalled("flush", write_timeout)),
                    }
                }
                match failed {
                    Some(reason) => {
                        // possibly undelivered: resend priority frames
                        unsent.splice(0..0, in_flight.drain(..));
                        break reason;
                    }
                    None => in_flight.clear(),
                }
            }
            frame = read.next() => {
                match frame {
                    Some(Ok(frame)) => {
                        last_rx = Instant::now();
                        match frame {
                            Frame::Text(text) => match WsMessage::from_json(&text) {
                                Ok(message) => {
                                    let unknown_type = (message.msg_type == WsMessageType::Unknown)
                                        .then(|| raw_type_name(&text))
                                        .flatten();
                                    sink.do_send(WsEvent::Received { generation, message, unknown_type });
                                }
                                Err(e) => debug!("Ignoring unparseable frame from hub: {e}"),
                            },
                            Frame::Close(_) => break "closed by hub".to_string(),
                            // tungstenite answers pings itself; pongs only
                            // refresh liveness
                            _ => {}
                        }
                    }
                    Some(Err(e)) => break format!("read error: {e}"),
                    None => break "stream ended".to_string(),
                }
            }
            _ = ping.tick(), if ping_enabled => {
                if last_rx.elapsed() >= dead_after {
                    break format!(
                        "nothing received for {}s (half-open connection)",
                        last_rx.elapsed().as_secs()
                    );
                }
                match tokio::time::timeout(write_timeout, write.send(Frame::Ping(Vec::new()))).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => break format!("ping failed: {e}"),
                    Err(_) => break write_stalled("ping", write_timeout),
                }
            }
        }
    };

    // everything still queued for this connection goes back to the owner
    rx.close();
    while let Ok(item) = rx.try_recv() {
        unsent.push(item);
    }
    SessionOutcome {
        unsent,
        reason,
        shutdown: shutdown_requested,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = WsClientConfig::default();
        assert_eq!(config.reconnect_delay, Duration::from_secs(5));
        assert_eq!(config.max_reconnect_delay, Duration::from_secs(60));
        assert!(config.auth_token.is_none());
    }

    #[test]
    fn test_backoff_grows_with_jitter_and_caps() {
        let mut b = Backoff::new(Duration::from_secs(5), Duration::from_secs(60), 42);
        let expected = [5u64, 10, 20, 40, 60, 60, 60];
        for d in expected {
            let got = b.next_delay();
            let d = Duration::from_secs(d);
            assert!(
                got >= d / 2 && got <= d,
                "{got:?} not in [{:?}, {d:?}]",
                d / 2
            );
        }
        b.reset();
        assert!(b.next_delay() <= Duration::from_secs(5));
        let m = b.max_delay();
        assert!(m >= Duration::from_secs(30) && m <= Duration::from_secs(60));
    }

    #[test]
    fn test_backoff_jitter_varies() {
        let mut b = Backoff::new(Duration::from_secs(60), Duration::from_secs(60), 7);
        let a = b.next_delay();
        let c = b.next_delay();
        assert_ne!(a, c);
    }

    #[test]
    fn test_build_request_auth_header() {
        let config = WsClientConfig {
            hub_url: "ws://127.0.0.1:1/ws".into(),
            auth_token: Some("s3cret".into()),
            ..WsClientConfig::default()
        };
        let req = build_request(&config).unwrap();
        assert_eq!(req.headers().get(AUTHORIZATION).unwrap(), "Bearer s3cret");

        let config = WsClientConfig {
            auth_token: None,
            ..config
        };
        assert!(build_request(&config)
            .unwrap()
            .headers()
            .get(AUTHORIZATION)
            .is_none());

        let bad = WsClientConfig {
            auth_token: Some("bad\ntoken".into()),
            ..WsClientConfig::default()
        };
        assert!(build_request(&bad).is_err());
        let bad_url = WsClientConfig {
            hub_url: "not a url".into(),
            ..WsClientConfig::default()
        };
        assert!(build_request(&bad_url).is_err());
    }

    #[test]
    fn test_raw_type_name() {
        assert_eq!(
            raw_type_name(r#"{"type":"future_thing","x":1}"#).as_deref(),
            Some("future_thing")
        );
        assert_eq!(raw_type_name("garbage"), None);
    }

    #[test]
    fn test_connection_state_names() {
        assert_eq!(ConnectionState::Connected.as_str(), "connected");
        assert_ne!(ConnectionState::Connected, ConnectionState::Disconnected);
    }

    /// A hub that accepts the connection but never reads: the send buffer
    /// fills up and writes block. The session must give up (bounded write)
    /// and hand the priority frames back instead of hanging forever.
    #[actix::test]
    async fn test_stalled_hub_write_times_out_and_requeues() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            // hold the socket open, never read
            tokio::time::sleep(Duration::from_secs(120)).await;
        });

        struct Sink(mpsc::UnboundedSender<WsEvent>);
        impl Actor for Sink {
            type Context = Context<Self>;
        }
        impl Handler<WsEvent> for Sink {
            type Result = ();
            fn handle(&mut self, msg: WsEvent, _: &mut Context<Self>) {
                let _ = self.0.send(msg);
            }
        }
        let (event_tx, mut events) = mpsc::unbounded_channel();
        let sink = Sink(event_tx).start().recipient();
        let mut client = WsClient::new(WsClientConfig {
            hub_url: format!("ws://{addr}/ws"),
            ping_interval: Duration::from_millis(300),
            reconnect_delay: Duration::from_secs(30),
            ..WsClientConfig::default()
        });
        assert!(client.connect(sink));
        let sender = match tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            WsEvent::Connected { sender, .. } => sender,
            other => panic!("unexpected {other:?}"),
        };
        let chunk = format!("\"{}\"", "x".repeat(128 * 1024));
        for _ in 0..400 {
            if sender
                .send(Outbound::new(chunk.clone(), OutboundClass::Priority))
                .is_err()
            {
                break;
            }
        }
        let (unsent, reason) = loop {
            match tokio::time::timeout(Duration::from_secs(15), events.recv()).await {
                Ok(Some(WsEvent::Disconnected { unsent, reason, .. })) => break (unsent, reason),
                Ok(Some(_)) => continue,
                other => panic!("session did not give up on the stalled hub: {other:?}"),
            }
        };
        assert!(reason.contains("stalled"), "{reason}");
        assert!(!unsent.is_empty(), "priority frames handed back");
        if let Some(task) = client.disconnect() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
    }

    /// Minimal hub: accepts one connection, records the Authorization
    /// header, reads one text frame and closes.
    #[actix::test]
    async fn test_connect_is_idempotent_and_delivers_frames() {
        use std::sync::Mutex;
        use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen_auth: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let seen = seen_auth.clone();
        let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let callback = |req: &Request, resp: Response| {
                *seen.lock().unwrap() = req
                    .headers()
                    .get(AUTHORIZATION)
                    .map(|v| v.to_str().unwrap().to_string());
                Ok(resp)
            };
            let mut ws = tokio_tungstenite::accept_hdr_async(stream, callback)
                .await
                .unwrap();
            while let Some(Ok(frame)) = ws.next().await {
                if let Frame::Text(t) = frame {
                    let _ = frame_tx.send(t);
                }
            }
        });

        struct Sink(mpsc::UnboundedSender<WsEvent>);
        impl Actor for Sink {
            type Context = Context<Self>;
        }
        impl Handler<WsEvent> for Sink {
            type Result = ();
            fn handle(&mut self, msg: WsEvent, _: &mut Context<Self>) {
                let _ = self.0.send(msg);
            }
        }
        let (event_tx, mut events) = mpsc::unbounded_channel();
        let sink = Sink(event_tx).start().recipient();

        let mut client = WsClient::new(WsClientConfig {
            hub_url: format!("ws://{addr}/ws"),
            auth_token: Some("tok".into()),
            ..WsClientConfig::default()
        });
        assert!(client.connect(sink.clone()));
        assert!(!client.connect(sink), "second connect must be a no-op");

        let sender = match tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            WsEvent::Connected {
                sender, local_ip, ..
            } => {
                assert_eq!(local_ip, Some("127.0.0.1".parse().unwrap()));
                sender
            }
            other => panic!("unexpected {other:?}"),
        };
        sender
            .send(Outbound::new(
                "{\"hello\":1}".into(),
                OutboundClass::Priority,
            ))
            .unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), frame_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, "{\"hello\":1}");
        assert_eq!(seen_auth.lock().unwrap().as_deref(), Some("Bearer tok"));

        let task = client.disconnect().unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(!client.is_running());
        // Disconnected then Stopped
        let mut saw_stopped = false;
        while let Ok(Some(ev)) =
            tokio::time::timeout(Duration::from_millis(500), events.recv()).await
        {
            if matches!(ev, WsEvent::Stopped { .. }) {
                saw_stopped = true;
            }
        }
        assert!(saw_stopped);
    }
}
