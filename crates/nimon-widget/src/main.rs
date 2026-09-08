//! NIMon Widget — all-in-one Windows monitoring companion.
//!
//! Single executable that runs:
//! - the NIMon hub (in-process, own runtime thread)
//! - the NIMon edge node (in-process, actix thread)
//! - a slim draggable "notch" strip docked at the right of the screen;
//!   hovering a chassis tile opens the detail panel beside it
//!
//! If a hub is already reachable, the widget attaches as a pure viewer.
//!
//! Usage: nimon-widget [--hub <url>] [--port <n>] [--no-hub] [--no-edge]

// Tray/widget app: no console window in release builds. Closing the
// auto-spawned console would otherwise kill the whole process.
// Debug builds keep the console so logs stay visible during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl,
    WebviewWindowBuilder,
};

/* window geometry (logical px; converted via monitor scale) */
const STRIP_W: f64 = 60.0; // bar column incl. transparent margins
const PANEL_W: f64 = 380.0;
const PANEL_GAP: f64 = 10.0;
const EXPANDED_W: f64 = STRIP_W + PANEL_GAP + PANEL_W; // 450
const MARGIN: f64 = 8.0;

fn probe_hub(base: &str) -> bool {
    http_agent()
        .get(&format!("{base}/health"))
        .timeout(Duration::from_millis(800))
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

/// Shared keep-alive HTTP agent (avoids a fresh TCP connection per poll)
fn http_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .max_idle_connections(2)
            .build()
    })
}

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

struct WidgetState {
    hub_base: String,
    scale: f64,
    /// physical work area (x, y, w, h)
    work: (i32, i32, u32, u32),
    /// collapsed window geometry, physical (x, y, w, h)
    dock: Mutex<(i32, i32, u32, u32)>,
    expanded: AtomicBool,
    locked: AtomicBool,
    lock_item: Mutex<Option<tauri::menu::CheckMenuItem<tauri::Wry>>>,
    state_path: PathBuf,
}

impl WidgetState {
    fn persist(&self) {
        let dock = *self.dock.lock().unwrap();
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

fn main() {
    // Passive add-on: keep the process polite on shared test stations.
    // Cap default tokio runtimes (edge actix system + tauri) to 2 workers;
    // the hub gets a dedicated 1-worker runtime below.
    std::env::set_var("TOKIO_WORKER_THREADS", "2");
    #[cfg(windows)]
    set_below_normal_priority();

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let opt = |name: &str| {
        args.iter().position(|a| a == name)
            .and_then(|i| args.get(i + 1)).cloned()
    };

    let port: u16 = opt("--port").and_then(|v| v.parse().ok()).unwrap_or(9090);
    // 127.0.0.1, not localhost: the hub binds IPv4 and `localhost` may
    // resolve to ::1 first, which would make every poll fail.
    let hub_base = opt("--hub").unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
    let want_hub = !flag("--no-hub");
    let want_edge = !flag("--no-edge");

    let root = resolve_root();

    /* ── 1. hub: probe for an already-running hub first ── */
    let external_hub = probe_hub(&hub_base);
    if external_hub {
        eprintln!("nimon-widget: hub already running at {hub_base} — attach mode");
    } else if want_hub {
        let root = root.clone();
        std::thread::Builder::new()
            .name("nimon-hub".into())
            .spawn(move || {
                // 1 worker: the hub is nearly idle (a few timers + rare HTTP)
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                    .expect("hub runtime");
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async move {
                    let config = load_hub_config(&root, port);
                    tracing::info!("hub on {}:{}", config.host, config.port);
                    if let Err(e) = nimon_hub::run(config).await {
                        tracing::error!("hub stopped: {}", e);
                    }
                });
            })
            .expect("spawn hub thread");
    }

    /* ── 2. wait for the hub to answer /health ── */
    let mut hub_up = false;
    for _ in 0..60 {
        if probe_hub(&hub_base) {
            hub_up = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if !hub_up {
        eprintln!("nimon-widget: hub not reachable at {hub_base} (widget will keep retrying)");
    }

    /* ── 3. edge node (own actix system thread; only when we own the hub) ── */
    if want_edge && !external_hub && hub_up {
        let root = root.clone();
        std::thread::Builder::new()
            .name("nimon-edge".into())
            .spawn(move || {
                let config = load_edge_config(&root, port);
                actix_rt::System::new().block_on(nimon_edge::start(config));
            })
            .expect("spawn edge thread");
    }

    /* ── 4. widget window ── */
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            poll,
            update_geometry,
            move_by,
            set_locked,
            get_state,
            save_state,
            open_dashboard,
            quit
        ])
        .setup(move |app| {
            let monitor = app
                .primary_monitor()
                .ok()
                .flatten()
                .or_else(|| {
                    app.available_monitors()
                        .ok()
                        .and_then(|m| m.first().cloned())
                })
                .expect("no monitor available");

            let scale = monitor.scale_factor();
            let area = monitor.work_area();
            let work = (
                area.position.x,
                area.position.y,
                area.size.width,
                area.size.height,
            );

            /* restore position/lock, or default to the right edge, centered */
            let state_path = root.join("widget-state.json");
            let saved: PersistedState = std::fs::read(&state_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();

            let strip_w = (STRIP_W * scale) as u32;
            let default_h = (320.0 * scale) as u32;
            let default_x =
                work.0 + work.2 as i32 - strip_w as i32 - (MARGIN * scale) as i32;
            let default_y = work.1 + ((work.3 as i32 - default_h as i32) / 2).max(0);

            let (x, y) = match (saved.x, saved.y) {
                (Some(x), Some(y))
                    if x >= work.0 - 40
                        && x <= work.0 + work.2 as i32
                        && y >= work.1
                        && y <= work.1 + work.3 as i32 =>
                {
                    (x, y)
                }
                _ => (default_x, default_y),
            };
            let locked = saved.locked.unwrap_or(false);

            let window = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::App("index.html".into()),
            )
                .title("NIMon Widget")
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .shadow(false)
                .focused(false)
                // trim webview background services we never use
                .additional_browser_args(
                    "--disable-component-update --disable-sync \
                     --disable-background-networking --disable-extensions \
                     --metrics-recording-only",
                )
                .inner_size(STRIP_W, 320.0)
                .position(x as f64 / scale, y as f64 / scale)
                .build()?;
            let _ = window.set_size(PhysicalSize::new(strip_w, default_h));
            let _ = window.set_position(PhysicalPosition::new(x, y));

            app.manage(WidgetState {
                hub_base,
                scale,
                work,
                dock: Mutex::new((x, y, strip_w, default_h)),
                expanded: AtomicBool::new(false),
                locked: AtomicBool::new(locked),
                lock_item: Mutex::new(None),
                state_path,
            });

            /* tray: PXI-branded icon drawn in code */
            let icon = tauri::image::Image::new_owned(tray_icon_rgba(), 32, 32);
            let open = tauri::menu::MenuItem::with_id(
                app, "open", "Open Dashboard", true, None::<&str>)?;
            let lock = tauri::menu::CheckMenuItem::with_id(
                app, "lock", "Lock position", true, locked, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(
                app, "quit", "Quit NIMon", true, None::<&str>)?;
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
                    "quit" => app.exit(0),
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
        .run(tauri::generate_context!())
        .expect("error while running nimon-widget");
}

/* ── commands ──────────────────────────────────────────────────────────── */

#[tauri::command]
async fn poll(app: AppHandle) -> Result<Value, String> {
    let base = app
        .try_state::<WidgetState>()
        .map(|s| s.hub_base.clone())
        .unwrap_or_else(|| "http://127.0.0.1:9090".to_string());
    tauri::async_runtime::spawn_blocking(move || poll_blocking(&base))
        .await
        .map_err(|e| e.to_string())
}

fn poll_blocking(base: &str) -> Value {
    let agent = http_agent();
    let get = |path: &str| -> Result<Value, String> {
        agent
            .get(&format!("{base}{path}"))
            .timeout(Duration::from_secs(2))
            .call()
            .map_err(|e| e.to_string())
            .and_then(|r| r.into_json::<Value>().map_err(|e| e.to_string()))
    };

    let edges = match get("/api/v1/edges") {
        Ok(v) => v,
        Err(e) => {
            static FAILURES: std::sync::atomic::AtomicU32 =
                std::sync::atomic::AtomicU32::new(0);
            let n = FAILURES.fetch_add(1, Ordering::Relaxed);
            if n == 0 || n % 12 == 0 {
                eprintln!("nimon-widget: poll failed ({base}): {e}");
            }
            return json!({ "up": false, "hub": base });
        }
    };

    let mut devices: Vec<Value> = Vec::new();
    if let Some(list) = edges["edges"].as_array() {
        for edge in list {
            if let Some(id) = edge["edge_id"].as_str() {
                if let Ok(devs) = get(&format!("/api/v1/edges/{id}/devices")) {
                    if let Some(arr) = devs["devices"].as_array() {
                        devices.extend(arr.iter().cloned());
                    }
                }
            }
        }
    }

    let alerts = get("/api/v1/alerts").unwrap_or_else(|_| json!({ "alerts": [] }));
    let predictions =
        get("/api/v1/predictions").unwrap_or_else(|_| json!({ "predictions": [] }));

    json!({
        "up": true,
        "hub": base,
        "edges": edges["edges"],
        "devices": devices,
        "alerts": alerts["alerts"],
        "predictions": predictions["predictions"],
    })
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

    let scale = state.scale;
    let (wx, wy, ww, wh) = state.work;
    let margin = (MARGIN * scale) as u32;
    let h = ((height * scale) as u32)
        .min(wh.saturating_sub(margin * 2))
        .max(160);

    if mode == "panel" {
        state.expanded.store(true, Ordering::Relaxed);
        let exp_w = (EXPANDED_W * scale) as u32;
        let (dock_x, dock_y, dock_w, _) = *state.dock.lock().unwrap();
        let x = dock_x + dock_w as i32 - exp_w as i32;
        let max_y = wy + wh as i32 - h as i32 - margin as i32;
        let y = dock_y.min(max_y).max(wy);
        let _ = window.set_size(PhysicalSize::new(exp_w, h));
        let _ = window.set_position(PhysicalPosition::new(x, y));
    } else {
        state.expanded.store(false, Ordering::Relaxed);
        let strip_w = (STRIP_W * scale) as u32;
        let (dock_x, dock_y, _, _) = *state.dock.lock().unwrap();
        let _ = window.set_size(PhysicalSize::new(strip_w, h));
        let _ = window.set_position(PhysicalPosition::new(dock_x, dock_y));
        *state.dock.lock().unwrap() = (dock_x, dock_y, strip_w, h);
    }

    h as f64 / scale
}

/// Manual drag: move by logical deltas from the current window position.
/// The dock is always kept in sync so collapsing never snaps the bar back:
/// while expanded, the bar's on-screen position is derived from the window
/// (bar column = window x + EXPANDED_W - STRIP_W, top = window y).
#[tauri::command]
fn move_by(app: AppHandle, dx: f64, dy: f64) {
    let Some(state) = app.try_state::<WidgetState>() else { return };
    let Some(window) = app.get_webview_window("main") else { return };
    if state.locked.load(Ordering::Relaxed) {
        return;
    }

    let Ok(pos) = window.outer_position() else { return };
    let Ok(size) = window.outer_size() else { return };
    let (wx, wy, ww, wh) = state.work;

    let nx = (pos.x + (dx * state.scale) as i32)
        .clamp(wx, wx + ww as i32 - size.width as i32);
    let ny = (pos.y + (dy * state.scale) as i32)
        .clamp(wy, wy + wh as i32 - size.height as i32);
    let _ = window.set_position(PhysicalPosition::new(nx, ny));

    let mut dock = state.dock.lock().unwrap();
    if state.expanded.load(Ordering::Relaxed) {
        let exp_w = (EXPANDED_W * state.scale) as i32;
        let strip_w = (STRIP_W * state.scale) as i32;
        // bar lives in the right column of the expanded window
        let (dw, dh) = (dock.2, dock.3);
        *dock = (nx + exp_w - strip_w, ny, dw, dh);
    } else {
        *dock = (nx, ny, size.width, size.height);
    }
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
    app.exit(0);
}

fn open_dashboard_cmd(app: &AppHandle) {
    let base = app
        .try_state::<WidgetState>()
        .map(|s| s.hub_base.clone())
        .unwrap_or_else(|| "http://127.0.0.1:9090".to_string());
    let url = format!("{base}/");
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

fn load_hub_config(root: &PathBuf, port: u16) -> nimon_hub::config::HubConfig {
    let path = root.join("config").join("test-hub.yaml");
    let mut config = if path.exists() {
        nimon_hub::config::HubConfig::load_or_default(&path)
            .unwrap_or_default()
    } else {
        nimon_hub::config::HubConfig::default()
    };
    config.host = "127.0.0.1".to_string();
    if config.port == 8080 && port != 8080 {
        config.port = port;
    }
    let db = root.join(&config.database_path);
    config.database_path = db.display().to_string();
    config
}

fn load_edge_config(root: &PathBuf, port: u16) -> nimon_edge::config::EdgeConfig {
    let path = root.join("config").join("edge.yaml");
    if let Ok(config) = nimon_edge::config::EdgeConfig::from_file(&path) {
        return config;
    }
    use nimon_edge::config::{
        ApiConfig, ApiSettings, BufferConfig, EdgeConfig, LoggingConfig, NodeConfig,
        PredictionConfig,
    };
    EdgeConfig {
        node: NodeConfig {
            id: "edge-01".to_string(),
            name: "Local Edge 1".to_string(),
            hub_address: format!("127.0.0.1:{port}"),
            reconnect_interval_secs: 5,
        },
        // 30s sweep: NI temperatures drift slowly and each NI-SysCfg
        // enumeration costs real CPU on constrained test stations
        api: ApiConfig {
            syscfg: ApiSettings {
                enabled: true,
                poll_interval_secs: 30,
            },
            daqmx: ApiSettings::default(),
            visa: ApiSettings::default(),
            xnet: ApiSettings::default(),
        },
        prediction: PredictionConfig::default(),
        buffer: BufferConfig::default(),
        logging: LoggingConfig::default(),
    }
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
            let inside = cx == radius || cy == radius
                || ((fx - cx.min(fx)).powi(2) + (fy - cy.min(fy)).powi(2)
                    <= radius * radius);
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
