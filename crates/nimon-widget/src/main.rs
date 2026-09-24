//! NIMon Widget — all-in-one Windows monitoring companion.
//!
//! Single executable that runs:
//! - the NIMon hub (in-process, own actix system thread)
//! - the NIMon edge node (in-process, own actix system thread, started in
//!   the background once the hub answers)
//! - a slim draggable "notch" strip docked at the right of the screen;
//!   hovering or activating a chassis tile opens the detail panel beside it
//!
//! If a hub is already reachable, the widget attaches as a pure viewer.
//! All HTTP is done from Rust (the webview has no network access, see the
//! CSP in `tauri.conf.json`).
//!
//! Usage: nimon-widget [--hub <url>] [--port <n>] [--no-hub] [--no-edge]

// Tray/widget app: no console window in release builds. Closing the
// auto-spawned console would otherwise kill the whole process.
// Debug builds keep the console so logs stay visible during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, RunEvent, WebviewUrl,
    WebviewWindowBuilder, WindowEvent,
};
use tokio::sync::oneshot;

/* window geometry (logical px; converted via the current monitor's scale) */
const STRIP_W: f64 = 60.0; // bar column incl. transparent margins
const PANEL_W: f64 = 380.0;
const PANEL_GAP: f64 = 10.0;
const EXPANDED_W: f64 = STRIP_W + PANEL_GAP + PANEL_W; // 450
const MARGIN: f64 = 8.0;
const DEFAULT_H: f64 = 320.0;

/// Per-request timeout for the local hub API
const HTTP_TIMEOUT: Duration = Duration::from_secs(2);
/// Settings change rarely: refetch at most this often
const SETTINGS_TTL: Duration = Duration::from_secs(60);
/// "starting…" is shown at most this long while our own hub comes up
const STARTUP_GRACE: Duration = Duration::from_secs(60);
/// Bounded waits on quit
const EDGE_STOP_TIMEOUT: Duration = Duration::from_secs(4);
const HUB_STOP_TIMEOUT: Duration = Duration::from_secs(8);

/* ── HTTP ──────────────────────────────────────────────────────────────── */

/// Shared keep-alive HTTP agent (avoids a fresh TCP connection per poll;
/// sized for the concurrent poll fan-out)
fn http_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .max_idle_connections(8)
            .max_idle_connections_per_host(8)
            .build()
    })
}

enum FetchError {
    /// hub answered with a non-2xx status (message from `{error}` if any)
    Status(u16, String, Option<Value>),
    /// hub not reachable / broken response
    Transport(String),
}

impl FetchError {
    fn message(&self) -> String {
        match self {
            FetchError::Status(code, msg, _) => format!("HTTP {code}: {msg}"),
            FetchError::Transport(msg) => msg.clone(),
        }
    }
}

fn map_ureq(e: ureq::Error) -> FetchError {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_json::<Value>().ok();
            let msg = body
                .as_ref()
                .and_then(|b| b["error"].as_str())
                .map(str::to_string)
                .unwrap_or_else(|| "request failed".to_string());
            FetchError::Status(code, msg, body)
        }
        ureq::Error::Transport(t) => FetchError::Transport(t.to_string()),
    }
}

fn get_json(base: &str, path: &str, timeout: Duration) -> Result<Value, FetchError> {
    http_agent()
        .get(&format!("{base}{path}"))
        .timeout(timeout)
        .call()
        .map_err(map_ureq)
        .and_then(|r| {
            r.into_json::<Value>()
                .map_err(|e| FetchError::Transport(format!("bad JSON: {e}")))
        })
}

/// True when *something* HTTP answers at `base` (any status): the hub is
/// listening, even if degraded.
fn hub_answers(base: &str) -> bool {
    match get_json(base, "/health", Duration::from_millis(800)) {
        Ok(_) | Err(FetchError::Status(..)) => true,
        Err(FetchError::Transport(_)) => false,
    }
}

/// Percent-encode one URL path segment (RFC 3986 unreserved chars kept).
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/* ── background services (hub + edge) ──────────────────────────────────── */

#[derive(Default)]
struct Services {
    hub_owned: AtomicBool,
    hub_seen: AtomicBool,
    hub_error: Mutex<Option<String>>,
    hub_tx: Mutex<Option<oneshot::Sender<()>>>,
    hub_thread: Mutex<Option<JoinHandle<()>>>,
    edge_state: Mutex<&'static str>,
    edge_tx: Mutex<Option<oneshot::Sender<()>>>,
    edge_thread: Mutex<Option<JoinHandle<()>>>,
    quitting: AtomicBool,
    stopped: AtomicBool,
}

fn services() -> &'static Services {
    static SERVICES: OnceLock<Services> = OnceLock::new();
    SERVICES.get_or_init(Services::default)
}

fn started_at() -> Instant {
    static T0: OnceLock<Instant> = OnceLock::new();
    *T0.get_or_init(Instant::now)
}

/// Sleep in small steps; returns false when a quit was requested.
fn sleep_unless_quitting(total: Duration) -> bool {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if services().quitting.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100).min(deadline - Instant::now()));
    }
    !services().quitting.load(Ordering::Relaxed)
}

fn spawn_hub(config: nimon_hub::config::HubConfig) {
    let (tx, rx) = oneshot::channel::<()>();
    *services().hub_tx.lock().unwrap() = Some(tx);
    services().hub_owned.store(true, Ordering::Relaxed);
    let handle = std::thread::Builder::new()
        .name("nimon-hub".into())
        .spawn(move || {
            // Actix 0.13 actor timers/spawns require an actix System
            // (tokio reactor + LocalSet context); a bare tokio runtime
            // panics with "no reactor running".
            let system = actix_rt::System::new();
            system.block_on(async move {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(async move {
                        tracing::info!("hub on {}:{}", config.host, config.port);
                        let shutdown = async move {
                            let _ = rx.await;
                        };
                        if let Err(e) = nimon_hub::run_with_shutdown(config, shutdown).await {
                            tracing::error!("hub stopped: {e:#}");
                            *services().hub_error.lock().unwrap() = Some(format!("{e:#}"));
                        }
                    })
                    .await;
            });
        })
        .expect("spawn hub thread");
    *services().hub_thread.lock().unwrap() = Some(handle);
}

/// Start the edge once the local hub answers; restart it (with a pause)
/// if it ever dies. Never blocks the UI thread.
fn spawn_edge_supervisor(root: PathBuf, port: u16, edge_token: Option<String>) {
    *services().edge_state.lock().unwrap() = "waiting for hub";
    let handle = std::thread::Builder::new()
        .name("nimon-edge".into())
        .spawn(move || {
            let local = format!("http://127.0.0.1:{port}");
            let mut waited = Duration::ZERO;
            loop {
                if services().quitting.load(Ordering::Relaxed) {
                    return;
                }
                if !hub_answers(&local) {
                    // fast while the hub starts, then back off
                    let step = if waited < STARTUP_GRACE {
                        Duration::from_millis(500)
                    } else {
                        Duration::from_secs(5)
                    };
                    waited += step;
                    if !sleep_unless_quitting(step) {
                        return;
                    }
                    continue;
                }

                let (tx, rx) = oneshot::channel::<()>();
                *services().edge_tx.lock().unwrap() = Some(tx);
                *services().edge_state.lock().unwrap() = "running";
                let config = load_edge_config(&root, port, edge_token.clone());
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    actix_rt::System::new().block_on(nimon_edge::start_with_shutdown(
                        config,
                        async move {
                            let _ = rx.await;
                        },
                    ));
                }));
                if services().quitting.load(Ordering::Relaxed) {
                    *services().edge_state.lock().unwrap() = "stopped";
                    return;
                }
                tracing::error!(
                    "edge stopped unexpectedly ({}); restarting in 5 s",
                    if run.is_err() { "panic" } else { "returned" }
                );
                *services().edge_state.lock().unwrap() = "restarting";
                services().edge_tx.lock().unwrap().take();
                if !sleep_unless_quitting(Duration::from_secs(5)) {
                    return;
                }
                waited = Duration::ZERO;
            }
        })
        .expect("spawn edge thread");
    *services().edge_thread.lock().unwrap() = Some(handle);
}

fn join_with_timeout(slot: &Mutex<Option<JoinHandle<()>>>, timeout: Duration, what: &str) {
    let Some(handle) = slot.lock().unwrap().take() else {
        return;
    };
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            tracing::warn!("{what} did not stop within {timeout:?}; exiting anyway");
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = handle.join();
}

/// Stop the edge (WS close frame) and then the hub (flushes buffered
/// writes). Idempotent; bounded.
fn shutdown_services() {
    let s = services();
    s.quitting.store(true, Ordering::Relaxed);
    if s.stopped.swap(true, Ordering::SeqCst) {
        return;
    }
    let t0 = Instant::now();
    if let Some(tx) = s.edge_tx.lock().unwrap().take() {
        let _ = tx.send(());
    }
    join_with_timeout(&s.edge_thread, EDGE_STOP_TIMEOUT, "edge");
    let edge_ms = t0.elapsed().as_millis();
    if let Some(tx) = s.hub_tx.lock().unwrap().take() {
        let _ = tx.send(());
    }
    join_with_timeout(&s.hub_thread, HUB_STOP_TIMEOUT, "hub");
    tracing::info!(
        "services stopped (edge {edge_ms} ms, hub {} ms)",
        t0.elapsed().as_millis() - edge_ms
    );
}

/// Quit without freezing the UI thread: hide, stop services in the
/// background, then exit.
fn request_quit(app: &AppHandle) {
    if services().quitting.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    let app = app.clone();
    std::thread::spawn(move || {
        shutdown_services();
        app.exit(0);
    });
}

/* ── widget state ──────────────────────────────────────────────────────── */

/// Passive add-on: run below normal priority so test station software wins
#[cfg(windows)]
fn set_below_normal_priority() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS,
    };
    unsafe {
        SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct PersistedState {
    locked: Option<bool>,
    x: Option<i32>,
    y: Option<i32>,
}

/// Collapsed-bar anchor + last requested layout
struct Geo {
    /// top-left of the bar column (physical, virtual-desktop coordinates)
    dock: (i32, i32),
    panel: bool,
    /// last requested window height (logical px)
    height: f64,
}

#[derive(Default)]
struct Resource {
    data: Value,
    ok_at: Option<Instant>,
    attempted: bool,
    error: Option<String>,
}

impl Resource {
    fn record(&mut self, result: Result<Value, String>) {
        self.attempted = true;
        match result {
            Ok(v) => {
                self.data = v;
                self.ok_at = Some(Instant::now());
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "data": self.data,
            "ok": self.attempted && self.error.is_none(),
            "error": self.error,
            "age_ms": self.ok_at.map(|t| t.elapsed().as_millis() as u64),
        })
    }
}

#[derive(Default)]
struct Cache {
    health: Resource,
    edges: Resource,
    devices: Resource,
    alerts: Resource,
    predictions: Resource,
    settings: Resource,
}

struct WidgetState {
    hub_base: String,
    api_token: Option<String>,
    geo: Mutex<Geo>,
    locked: AtomicBool,
    lock_item: Mutex<Option<tauri::menu::CheckMenuItem<tauri::Wry>>>,
    state_path: PathBuf,
    cache: Mutex<Cache>,
}

impl WidgetState {
    fn persist(&self) {
        let dock = self.geo.lock().unwrap().dock;
        let state = PersistedState {
            locked: Some(self.locked.load(Ordering::Relaxed)),
            x: Some(dock.0),
            y: Some(dock.1),
        };
        if let Ok(text) = serde_json::to_string_pretty(&state) {
            let _ = std::fs::write(&self.state_path, text);
        }
    }

    fn set_locked(&self, app: &AppHandle, locked: bool) {
        self.locked.store(locked, Ordering::Relaxed);
        if let Some(item) = self.lock_item.lock().unwrap().as_ref() {
            let _ = item.set_checked(locked);
        }
        let _ = app.emit("lock-changed", locked);
        self.persist();
    }
}

/* ── monitors ──────────────────────────────────────────────────────────── */

/// A monitor's full bounds, work area and scale (physical px)
#[derive(Clone, Copy, Debug)]
struct Area {
    bounds: (i32, i32, i32, i32),
    work: (i32, i32, i32, i32),
    scale: f64,
}

impl Area {
    fn contains(&self, x: i32, y: i32) -> bool {
        let (bx, by, bw, bh) = self.bounds;
        x >= bx && x < bx + bw && y >= by && y < by + bh
    }

    fn distance2(&self, x: i32, y: i32) -> i64 {
        let (bx, by, bw, bh) = self.bounds;
        let dx = (bx - x).max(0).max(x - (bx + bw - 1)) as i64;
        let dy = (by - y).max(0).max(y - (by + bh - 1)) as i64;
        dx * dx + dy * dy
    }
}

fn area_of(m: &tauri::Monitor) -> Area {
    let (p, s) = (m.position(), m.size());
    let wa = m.work_area();
    Area {
        bounds: (p.x, p.y, s.width as i32, s.height as i32),
        work: (
            wa.position.x,
            wa.position.y,
            wa.size.width as i32,
            wa.size.height as i32,
        ),
        scale: m.scale_factor(),
    }
}

fn all_areas(app: &AppHandle) -> Vec<Area> {
    app.available_monitors()
        .map(|ms| ms.iter().map(area_of).collect())
        .unwrap_or_default()
}

fn primary_area(app: &AppHandle) -> Area {
    app.primary_monitor()
        .ok()
        .flatten()
        .map(|m| area_of(&m))
        .or_else(|| all_areas(app).first().copied())
        .unwrap_or(Area {
            bounds: (0, 0, 1920, 1080),
            work: (0, 0, 1920, 1040),
            scale: 1.0,
        })
}

fn area_containing(app: &AppHandle, x: i32, y: i32) -> Option<Area> {
    all_areas(app).into_iter().find(|a| a.contains(x, y))
}

/// Monitor containing the point, else the nearest one, else the primary.
fn area_nearest(app: &AppHandle, x: i32, y: i32) -> Area {
    let areas = all_areas(app);
    if let Some(a) = areas.iter().find(|a| a.contains(x, y)) {
        return *a;
    }
    areas
        .into_iter()
        .min_by_key(|a| a.distance2(x, y))
        .unwrap_or_else(|| primary_area(app))
}

/// Probe point for the bar at `dock` (a little inside its top-left)
fn dock_probe(dock: (i32, i32)) -> (i32, i32) {
    (dock.0 + 12, dock.1 + 12)
}

/// Physical rectangle (x, y, w, h)
type Rect = (i32, i32, i32, i32);

/// Window rectangle and clamped dock for a layout, all in
/// physical px on `area` (the monitor holding the bar). The whole window
/// always stays inside that monitor's work area, so the window only ever
/// occupies the monitor whose scale was used to size it:
/// - the bar is clamped onto the monitor (also recovers from unplugged
///   monitors)
/// - the expanded panel grows leftward from the bar; at a left monitor
///   edge (bar docked on the left) it is shifted right instead of opening
///   off-screen or onto a neighbouring monitor
fn layout(area: &Area, dock: (i32, i32), panel: bool, height: f64) -> (Rect, (i32, i32)) {
    let s = area.scale;
    let (wx, wy, ww, wh) = area.work;
    let strip_w = (STRIP_W * s).round() as i32;
    let exp_w = (EXPANDED_W * s).round() as i32;
    let margin = (MARGIN * s).round() as i32;
    let max_h = (wh - margin * 2).max(80);
    let h = ((height * s).round() as i32).clamp(160.min(max_h), max_h);

    let dock_x = dock.0.clamp(wx, (wx + ww - strip_w).max(wx));
    let dock_y = dock.1.clamp(wy, (wy + wh - 40).max(wy));

    if panel {
        let x = (dock_x + strip_w - exp_w).clamp(wx, (wx + ww - exp_w).max(wx));
        let y = dock_y.min(wy + wh - h - margin).max(wy);
        ((x, y, exp_w, h), (dock_x, dock_y))
    } else {
        let dock_y = dock_y.min(wy + wh - h).max(wy);
        ((dock_x, dock_y, strip_w, h), (dock_x, dock_y))
    }
}

/// Apply `geo` to the window on whichever monitor holds the bar (see
/// [`layout`]). Returns the final window height in logical px.
fn apply_geometry(app: &AppHandle, window: &tauri::WebviewWindow, geo: &mut Geo) -> f64 {
    let (px, py) = dock_probe(geo.dock);
    let area = area_nearest(app, px, py);
    let ((x, y, w, h), dock) = layout(&area, geo.dock, geo.panel, geo.height);
    let _ = window.set_size(PhysicalSize::new(w as u32, h as u32));
    let _ = window.set_position(PhysicalPosition::new(x, y));
    geo.dock = dock;
    h as f64 / area.scale
}

/// Re-apply the last layout (DPI change, monitor unplugged, …)
fn reapply_geometry(app: &AppHandle) {
    let (Some(state), Some(window)) = (
        app.try_state::<WidgetState>(),
        app.get_webview_window("main"),
    ) else {
        return;
    };
    let final_h = {
        let mut geo = state.geo.lock().unwrap();
        apply_geometry(app, &window, &mut geo)
    };
    let _ = app.emit("geometry-changed", final_h);
}

/* ── main ──────────────────────────────────────────────────────────────── */

fn main() {
    // Passive add-on: keep the process polite on shared test stations.
    // Cap default tokio runtimes (edge/hub actix systems + tauri) to 2 workers.
    std::env::set_var("TOKIO_WORKER_THREADS", "2");
    #[cfg(windows)]
    set_below_normal_priority();
    let _ = started_at();

    let root = resolve_root();
    init_logging(&root);

    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    let port: u16 = opt("--port")
        .and_then(|v| v.parse().ok())
        .unwrap_or(nimon_hub::config::DEFAULT_PORT);
    // 127.0.0.1, not localhost: the hub binds IPv4 and `localhost` may
    // resolve to ::1 first, which would make every poll fail.
    let hub_arg = opt("--hub");
    let hub_base = hub_arg
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
    let hub_base = hub_base.trim_end_matches('/').to_string();
    // our own hub only makes sense where the UI will look for it: a remote
    // --hub that is briefly down at launch must not start a local hub +
    // edge the UI never talks to
    let local_target = hub_arg.is_none() || is_local_hub(&hub_base, port);
    let want_hub = !flag("--no-hub") && local_target;
    let want_edge = !flag("--no-edge");
    if !local_target && !flag("--no-hub") {
        tracing::info!(
            "--hub {hub_base} is not this machine's 127.0.0.1:{port}; no local hub/edge started"
        );
    }

    /* ── 1. hub config (also the source of the API token in attach mode) ── */
    let hub_config = load_hub_config(&root, port);
    let auth = match &hub_config {
        Ok(c) => c.auth.resolved(),
        Err(_) => nimon_hub::config::AuthConfig::default().resolved(),
    };

    /* ── 2. hub: attach to a running hub, else start ours (non-blocking) ── */
    let external_hub = hub_answers(&hub_base);
    if external_hub {
        tracing::info!("hub already running at {hub_base} — attach mode");
        services().hub_seen.store(true, Ordering::Relaxed);
    } else if want_hub {
        match hub_config {
            Ok(config) => spawn_hub(config),
            Err(e) => {
                tracing::error!("hub not started: {e}");
                *services().hub_error.lock().unwrap() = Some(e);
            }
        }
    }

    /* ── 3. edge: started in the background once our hub answers ── */
    if want_edge && !external_hub && want_hub {
        spawn_edge_supervisor(root.clone(), port, auth.edge_token.clone());
    } else {
        *services().edge_state.lock().unwrap() = "disabled";
    }

    /* ── 4. widget window (shown immediately; the UI shows "starting…") ── */
    let api_token = auth.api_token.clone();
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            poll,
            alert_action,
            update_geometry,
            move_by,
            set_locked,
            get_state,
            save_state,
            open_dashboard,
            quit,
            ui_log
        ])
        .setup(move |app| {
            let handle = app.handle().clone();

            /* restore position/lock onto whichever monitor holds it,
            or default to the primary's right edge, centered */
            let state_path = root.join("widget-state.json");
            let saved: PersistedState = std::fs::read(&state_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();

            let restored = match (saved.x, saved.y) {
                (Some(x), Some(y)) => {
                    let (px, py) = dock_probe((x, y));
                    area_containing(&handle, px, py).map(|a| ((x, y), a))
                }
                _ => None,
            };
            let (dock, area) = restored.unwrap_or_else(|| {
                let a = primary_area(&handle);
                let s = a.scale;
                let (wx, wy, ww, wh) = a.work;
                let x = wx + ww - (STRIP_W * s) as i32 - (MARGIN * s) as i32;
                let y = wy + ((wh - (DEFAULT_H * s) as i32) / 2).max(0);
                ((x, y), a)
            });
            let locked = saved.locked.unwrap_or(false);

            let window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("NIMon Widget")
                    .decorations(false)
                    .transparent(true)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .resizable(false)
                    .shadow(false)
                    // never steal focus from LabVIEW/TestStand; clicking
                    // the widget focuses it (then Tab/Esc work)
                    .focused(false)
                    // trim webview background services we never use
                    .additional_browser_args(
                        "--disable-component-update --disable-sync \
                     --disable-background-networking --disable-extensions \
                     --metrics-recording-only",
                    )
                    .inner_size(STRIP_W, DEFAULT_H)
                    .position(dock.0 as f64 / area.scale, dock.1 as f64 / area.scale)
                    .build()?;

            let mut geo = Geo {
                dock,
                panel: false,
                height: DEFAULT_H,
            };
            apply_geometry(&handle, &window, &mut geo);

            app.manage(WidgetState {
                hub_base,
                api_token,
                geo: Mutex::new(geo),
                locked: AtomicBool::new(locked),
                lock_item: Mutex::new(None),
                state_path,
                cache: Mutex::new(Cache::default()),
            });

            /* DPI / monitor changes: re-lay out with the new scale. Deferred
            so it runs after the runtime applied its own suggested size. */
            let h2 = handle.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::ScaleFactorChanged { .. } = event {
                    let app = h2.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(60)).await;
                        reapply_geometry(&app);
                    });
                }
            });

            /* tray: PXI-branded icon drawn in code */
            let icon = tauri::image::Image::new_owned(tray_icon_rgba(), 32, 32);
            let open =
                tauri::menu::MenuItem::with_id(app, "open", "Open Dashboard", true, None::<&str>)?;
            let lock = tauri::menu::CheckMenuItem::with_id(
                app,
                "lock",
                "Lock position",
                true,
                locked,
                None::<&str>,
            )?;
            let quit =
                tauri::menu::MenuItem::with_id(app, "quit", "Quit NIMon", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&open, &lock, &quit])?;

            if let Some(state) = app.try_state::<WidgetState>() {
                *state.lock_item.lock().unwrap() = Some(lock);
            }

            tauri::tray::TrayIconBuilder::with_id("nimon")
                .icon(icon)
                .tooltip("NIMon Widget")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => open_dashboard_cmd(app),
                    "quit" => request_quit(app),
                    "lock" => {
                        if let Some(state) = app.try_state::<WidgetState>() {
                            let locked = state.locked.load(Ordering::Relaxed);
                            state.set_locked(app, !locked);
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building nimon-widget");

    app.run(|_app, event| {
        if let RunEvent::Exit = event {
            // any exit path (window closed, app.exit, …): stop edge + hub
            shutdown_services();
            // the process exits without running destructors: flush the log
            // file writer now
            flush_logs();
        }
    });
}

/* ── commands ──────────────────────────────────────────────────────────── */

fn hub_base_of(app: &AppHandle) -> String {
    app.try_state::<WidgetState>()
        .map(|s| s.hub_base.clone())
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", nimon_hub::config::DEFAULT_PORT))
}

/// Fetch everything the UI needs, concurrently, keeping the last good
/// copy of each resource on failure (with stale/error flags).
#[tauri::command]
async fn poll(app: AppHandle) -> Result<Value, String> {
    let handle = app.clone();
    let out = tauri::async_runtime::spawn_blocking(move || poll_blocking(&handle))
        .await
        .map_err(|e| e.to_string())?;

    // cheap watchdog: bar parked on a monitor that no longer exists
    if let Some(state) = app.try_state::<WidgetState>() {
        let dock = state.geo.lock().unwrap().dock;
        let (px, py) = dock_probe(dock);
        if area_containing(&app, px, py).is_none() {
            reapply_geometry(&app);
        }
    }
    Ok(out)
}

fn poll_blocking(app: &AppHandle) -> Value {
    let Some(state) = app.try_state::<WidgetState>() else {
        return json!({ "up": false });
    };
    let base = state.hub_base.as_str();

    let need_settings = {
        let cache = state.cache.lock().unwrap();
        cache.settings.error.is_some()
            || cache
                .settings
                .ok_at
                .is_none_or(|t| t.elapsed() >= SETTINGS_TTL)
    };

    let fetch = |path: &str, key: Option<&str>| -> Result<Value, FetchError> {
        get_json(base, path, HTTP_TIMEOUT).map(|v| match key {
            Some(k) => v.get(k).cloned().unwrap_or(Value::Null),
            None => v,
        })
    };

    let (health, edges, devices, alerts, predictions, settings) = std::thread::scope(|s| {
        let health = s.spawn(|| fetch("/health", None));
        let edges = s.spawn(|| fetch("/api/v1/edges", Some("edges")));
        let devices = s.spawn(|| fetch("/api/v1/devices", Some("devices")));
        let alerts = s.spawn(|| fetch("/api/v1/alerts", Some("alerts")));
        let predictions = s.spawn(|| fetch("/api/v1/predictions", Some("predictions")));
        let settings = need_settings.then(|| s.spawn(|| fetch("/api/v1/settings", None)));
        let join = |h: std::thread::ScopedJoinHandle<'_, Result<Value, FetchError>>| {
            h.join()
                .unwrap_or_else(|_| Err(FetchError::Transport("fetch panicked".into())))
        };
        (
            join(health),
            join(edges),
            join(devices),
            join(alerts),
            join(predictions),
            settings.map(join),
        )
    });

    // hub reachability: /health answering at all (503 = degraded but up)
    let (up, health_res) = match health {
        Ok(v) => (true, Ok(v)),
        Err(FetchError::Status(503, _, Some(body))) => (true, Ok(body)),
        Err(e) => (matches!(e, FetchError::Status(..)), Err(e.message())),
    };
    static FAILURES: AtomicU32 = AtomicU32::new(0);
    if up {
        services().hub_seen.store(true, Ordering::Relaxed);
        FAILURES.store(0, Ordering::Relaxed);
    } else if let Err(e) = &health_res {
        let n = FAILURES.fetch_add(1, Ordering::Relaxed);
        if !starting() && n.is_multiple_of(12) {
            tracing::warn!("hub unreachable ({base}): {e}");
        }
    }

    let mut cache = state.cache.lock().unwrap();
    cache.health.record(health_res);
    cache.edges.record(edges.map_err(|e| e.message()));
    cache.devices.record(devices.map_err(|e| e.message()));
    cache.alerts.record(alerts.map_err(|e| e.message()));
    cache.predictions.record(predictions.map_err(|e| e.message()));
    if let Some(r) = settings {
        cache.settings.record(r.map_err(|e| e.message()));
    } else if !up {
        cache.settings.error = Some("hub unreachable".into());
    }

    json!({
        "hub": base,
        "up": up,
        "starting": starting(),
        "hub_error": services().hub_error.lock().unwrap().clone(),
        "edge_state": *services().edge_state.lock().unwrap(),
        "has_token": state.api_token.is_some(),
        "res": {
            "health": cache.health.to_json(),
            "edges": cache.edges.to_json(),
            "devices": cache.devices.to_json(),
            "alerts": cache.alerts.to_json(),
            "predictions": cache.predictions.to_json(),
            "settings": cache.settings.to_json(),
        },
    })
}

/// Our own hub is still coming up (show "starting…", not "HUB DOWN")
fn starting() -> bool {
    let s = services();
    s.hub_owned.load(Ordering::Relaxed)
        && !s.hub_seen.load(Ordering::Relaxed)
        && s.hub_error.lock().unwrap().is_none()
        && started_at().elapsed() < STARTUP_GRACE
}

/// Acknowledge or resolve an alert (`action` = "acknowledge" | "resolve").
#[tauri::command]
async fn alert_action(app: AppHandle, id: String, action: String) -> Result<Value, String> {
    if action != "acknowledge" && action != "resolve" {
        return Err(format!("unknown action '{action}'"));
    }
    if id.trim().is_empty() {
        return Err("missing alert id".into());
    }
    let (base, token) = match app.try_state::<WidgetState>() {
        Some(s) => (s.hub_base.clone(), s.api_token.clone()),
        None => return Err("widget not ready".into()),
    };
    tauri::async_runtime::spawn_blocking(move || {
        let url = format!("{base}/api/v1/alerts/{}/{action}", encode_segment(&id));
        let mut req = http_agent().post(&url).timeout(Duration::from_secs(5));
        if let Some(t) = &token {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        match req.call() {
            Ok(r) => Ok(r.into_json::<Value>().unwrap_or(Value::Null)),
            Err(e) => {
                let e = map_ureq(e);
                Err(match (&e, token.is_some()) {
                    (FetchError::Status(401, ..), false) => {
                        "hub requires an API token (set auth.api_token or NIMON_API_TOKEN)"
                            .to_string()
                    }
                    (FetchError::Status(401, ..), true) => "hub rejected the API token".into(),
                    (FetchError::Transport(_), _) => "hub unreachable".into(),
                    _ => e.message(),
                })
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Resize the window for the requested mode. `height` is logical px.
/// Strip mode anchors to the dock's top-left; panel mode grows leftward
/// from the dock and keeps the bar at its on-screen position.
/// Returns the final window height in logical px (clamped to the work area).
#[tauri::command]
fn update_geometry(app: AppHandle, mode: String, height: f64) -> f64 {
    let (Some(state), Some(window)) = (
        app.try_state::<WidgetState>(),
        app.get_webview_window("main"),
    ) else {
        return height;
    };
    let mut geo = state.geo.lock().unwrap();
    geo.panel = mode == "panel";
    if height.is_finite() && height > 0.0 {
        geo.height = height;
    }
    apply_geometry(&app, &window, &mut geo)
}

/// Manual drag: move by logical deltas from the current window position.
/// The bar may be dragged onto any monitor; it is clamped to the work area
/// of the monitor it ends up on. The dock is kept in sync so collapsing
/// never snaps the bar back.
#[tauri::command]
fn move_by(app: AppHandle, dx: f64, dy: f64) {
    let (Some(state), Some(window)) = (
        app.try_state::<WidgetState>(),
        app.get_webview_window("main"),
    ) else {
        return;
    };
    if state.locked.load(Ordering::Relaxed) || !dx.is_finite() || !dy.is_finite() {
        return;
    }
    let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return;
    };
    let mut geo = state.geo.lock().unwrap();
    let (px, py) = dock_probe(geo.dock);
    let current = area_nearest(&app, px, py);
    let s = current.scale;
    let strip_w = (STRIP_W * s).round() as i32;
    let (w, h) = (size.width as i32, size.height as i32);

    let nx = pos.x + (dx * s).round() as i32;
    let ny = pos.y + (dy * s).round() as i32;
    // bar column: right edge of the window
    let bar_x = nx + w - strip_w;
    let target = area_containing(&app, bar_x + strip_w / 2, ny + 12).unwrap_or(current);
    let (wx, wy, ww, wh) = target.work;
    let nx = nx.clamp(wx.min(wx + ww - w), (wx + ww - w).max(wx));
    let ny = ny.clamp(wy, (wy + wh - h).max(wy));
    let _ = window.set_position(PhysicalPosition::new(nx, ny));
    geo.dock = (nx + w - strip_w, ny);
}

#[tauri::command]
fn set_locked(app: AppHandle, locked: bool) {
    if let Some(state) = app.try_state::<WidgetState>() {
        state.set_locked(&app, locked);
    }
}

#[tauri::command]
fn get_state(app: AppHandle) -> Value {
    match app.try_state::<WidgetState>() {
        Some(state) => json!({ "locked": state.locked.load(Ordering::Relaxed) }),
        None => json!({ "locked": false }),
    }
}

#[tauri::command]
fn save_state(app: AppHandle) {
    if let Some(state) = app.try_state::<WidgetState>() {
        state.persist();
    }
}

#[tauri::command]
fn open_dashboard(app: AppHandle) {
    open_dashboard_cmd(&app);
}

#[tauri::command]
fn quit(app: AppHandle) {
    request_quit(&app);
}

/// UI diagnostics (JS errors, CSP violations) to the log, rate-limited.
#[tauri::command]
fn ui_log(level: String, message: String) {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    if COUNT.fetch_add(1, Ordering::Relaxed) < 50 {
        let msg: String = message.chars().take(500).collect();
        match level.as_str() {
            "error" => tracing::error!("ui: {msg}"),
            "warn" => tracing::warn!("ui: {msg}"),
            _ => tracing::info!("ui [{level}]: {msg}"),
        }
    }
}

fn open_dashboard_cmd(app: &AppHandle) {
    let url = format!("{}/", hub_base_of(app));
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", &url])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
}

/* ── helpers ───────────────────────────────────────────────────────────── */

/// Keeps the log file writer alive for the process lifetime
fn log_guard() -> &'static Mutex<Option<Box<dyn Send>>> {
    static GUARD: OnceLock<Mutex<Option<Box<dyn Send>>>> = OnceLock::new();
    GUARD.get_or_init(|| Mutex::new(None))
}

/// Widget log file (hub + edge + widget all log here)
fn widget_log_path(root: &Path) -> PathBuf {
    root.join("logs").join("nimon-widget.log")
}

/// Logging for the whole process. Release builds are GUI-subsystem apps
/// without a console (stdout/stderr go nowhere), so everything — widget,
/// embedded hub and embedded edge — is written to
/// `<root>/logs/nimon-widget.log` (rotated daily, 7 files kept). The level
/// is `logging.level` from `config/edge.yaml` (`RUST_LOG` overrides);
/// debug builds also log to the console.
fn init_logging(root: &Path) {
    let level = nimon_edge::config::EdgeConfig::from_file(root.join("config").join("edge.yaml"))
        .map(|c| c.logging.level)
        .unwrap_or_else(|_| "info".to_string());
    let config = nimon_edge::config::LoggingConfig {
        level,
        file: widget_log_path(root).display().to_string(),
        stdout: cfg!(debug_assertions),
    };
    if let Some(guard) = nimon_edge::logging::init(&config) {
        *log_guard().lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(guard));
    }
    tracing::info!(
        "nimon-widget {} starting (log file {})",
        env!("CARGO_PKG_VERSION"),
        config.file
    );
}

/// Flush and stop the log file writer (at exit)
fn flush_logs() {
    let guard = log_guard().lock().unwrap_or_else(|e| e.into_inner()).take();
    drop(guard);
}

/// True when `base` (an http URL) is the local hub on `port`, i.e. the hub
/// this widget would start itself
fn is_local_hub(base: &str, port: u16) -> bool {
    let Some(rest) = base.trim().strip_prefix("http://") else {
        return false; // https: never our embedded plain-http hub
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port_str) = match authority.strip_prefix('[') {
        // [::1]:9090
        Some(v6) => match v6.split_once(']') {
            Some((h, tail)) => (h, tail.strip_prefix(':')),
            None => return false,
        },
        None => match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        },
    };
    let actual_port = match port_str {
        Some(p) => match p.parse::<u16>() {
            Ok(p) => p,
            Err(_) => return false,
        },
        None => 80,
    };
    let host = host.to_ascii_lowercase();
    let local = host == "localhost" || host == "::1" || host.starts_with("127.");
    local && actual_port == port
}

fn resolve_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.join("config").is_dir() {
        return cwd;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.to_path_buf();
        }
    }
    cwd
}

/// Hub config for the embedded hub. An invalid file is an error (running
/// with silently different rules/auth would be worse than not running).
fn load_hub_config(root: &Path, port: u16) -> Result<nimon_hub::config::HubConfig, String> {
    let path = root.join("config").join("test-hub.yaml");
    let mut config = nimon_hub::config::HubConfig::load_or_default(&path)
        .map_err(|e| format!("{}: {e:#}", path.display()))?;
    config.host = "127.0.0.1".to_string();
    // --port is explicit user intent; it always wins over config files
    config.port = port;
    let db = root.join(&config.database_path);
    config.database_path = db.display().to_string();
    Ok(config)
}

fn load_edge_config(
    root: &Path,
    port: u16,
    hub_edge_token: Option<String>,
) -> nimon_edge::config::EdgeConfig {
    use nimon_edge::config::{ApiConfig, ApiSettings, EdgeConfig, NodeConfig};
    let path = root.join("config").join("edge.yaml");
    let mut config = EdgeConfig::from_file(&path).unwrap_or_else(|e| {
        if path.exists() {
            tracing::warn!("{} invalid ({e}); using defaults", path.display());
        }
        EdgeConfig {
            node: NodeConfig::new("edge-01", "Local Edge 1", format!("127.0.0.1:{port}")),
            // 30s sweep: NI temperatures drift slowly and each NI-SysCfg
            // enumeration costs real CPU on constrained test stations
            api: ApiConfig {
                syscfg: ApiSettings {
                    enabled: true,
                    poll_interval_secs: 30,
                },
                ..Default::default()
            },
            ..Default::default()
        }
    });
    // The embedded edge always talks to the hub we launched (127.0.0.1,
    // the --port override), plain ws.
    config.node.hub_address = format!("127.0.0.1:{port}");
    config.node.tls = false;
    if config.node.hub_token.is_none() {
        config.node.hub_token = hub_edge_token;
    }
    config
}

/// 32x32 RGBA tray icon: teal rounded square with a light center dot.
fn tray_icon_rgba() -> Vec<u8> {
    const S: usize = 32;
    let mut px = vec![0u8; S * S * 4];
    let base = [0x00, 0x69, 0x6d, 0xff];
    let dot = [0x9c, 0xf1, 0xf2, 0xff];
    let radius = 7.0f32;
    let center = (S as f32 - 1.0) / 2.0;
    for y in 0..S {
        for x in 0..S {
            let fx = x as f32;
            let fy = y as f32;
            let cx = fx.min(S as f32 - 1.0 - fx).min(radius);
            let cy = fy.min(S as f32 - 1.0 - fy).min(radius);
            let inside = cx == radius
                || cy == radius
                || ((fx - cx.min(fx)).powi(2) + (fy - cy.min(fy)).powi(2) <= radius * radius);
            if !inside {
                continue;
            }
            let dist = ((fx - center).powi(2) + (fy - center).powi(2)).sqrt();
            let color = if dist < 5.0 { dot } else { base };
            let i = (y * S + x) * 4;
            px[i..i + 4].copy_from_slice(&color);
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_segment_escapes_reserved() {
        assert_eq!(encode_segment("abc-1_2.3~"), "abc-1_2.3~");
        assert_eq!(encode_segment("edge:PXI#2/x y"), "edge%3APXI%232%2Fx%20y");
    }

    #[test]
    fn local_hub_detection() {
        assert!(is_local_hub("http://127.0.0.1:9090", 9090));
        assert!(is_local_hub("http://localhost:9090/", 9090));
        assert!(is_local_hub("http://[::1]:9090", 9090));
        assert!(!is_local_hub("http://127.0.0.1:9191", 9090), "other port");
        assert!(!is_local_hub("http://hub.plant.local:9090", 9090));
        assert!(!is_local_hub("http://10.0.0.5:9090", 9090));
        assert!(!is_local_hub("https://127.0.0.1:9090", 9090));
        assert!(is_local_hub("http://127.0.0.1", 80));
        assert!(!is_local_hub("http://127.0.0.1:x", 9090));
    }

    fn monitor(x: i32, w: i32, scale: f64) -> Area {
        Area {
            bounds: (x, 0, w, 1080),
            work: (x, 0, w, 1040),
            scale,
        }
    }

    #[test]
    fn expanded_panel_stays_on_the_bar_monitor() {
        for scale in [1.0, 1.5] {
            let a = monitor(0, 1920, scale);
            let (wx, _, ww, _) = a.work;
            // bar docked at the left edge: panel shifted right, on screen
            let ((x, _, w, _), dock) = layout(&a, (0, 100), true, 300.0);
            assert!(x >= wx && x + w <= wx + ww, "scale {scale}: x={x} w={w}");
            assert_eq!(dock.0, 0, "dock itself is unchanged");
            // bar docked at the right edge: panel grows leftward as before
            let strip = (STRIP_W * scale).round() as i32;
            let right = ww - strip;
            let ((x, _, w, _), _) = layout(&a, (right, 100), true, 300.0);
            assert_eq!(x + w, right + strip);
            // collapsed strip sits exactly at the dock
            let ((x, _, w, _), _) = layout(&a, (right, 100), false, 300.0);
            assert_eq!((x, w), (right, strip));
        }
        // secondary monitor to the right of the primary: its left edge
        let second = monitor(1920, 1280, 1.25);
        let ((x, _, w, _), _) = layout(&second, (1920, 50), true, 300.0);
        assert!(x >= 1920 && x + w <= 1920 + 1280, "x={x} w={w}");
    }

    #[test]
    fn area_nearest_math() {
        let a = Area {
            bounds: (0, 0, 100, 100),
            work: (0, 0, 100, 90),
            scale: 1.0,
        };
        assert!(a.contains(0, 0) && a.contains(99, 99) && !a.contains(100, 50));
        assert_eq!(a.distance2(50, 50), 0);
        assert_eq!(a.distance2(103, 50), 16);
    }
}
