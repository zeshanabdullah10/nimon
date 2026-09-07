//! NIMon Widget — all-in-one Windows monitoring companion.
//!
//! Single executable that runs:
//! - the NIMon hub (in-process, own runtime thread)
//! - the NIMon edge node (in-process, actix thread)
//! - a slim always-on-top widget docked to the right edge of the screen
//!
//! If the hub port is already occupied (hub running externally), the widget
//! starts in attach mode: no hub/edge spawned, it just renders hub data.
//!
//! Usage: nimon-widget [--hub <url>] [--port <n>] [--no-hub] [--no-edge]

use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

fn probe_hub(base: &str) -> bool {
    ureq::get(&format!("{base}/health"))
        .timeout(Duration::from_millis(800))
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder,
};

const RAIL_WIDTH: f64 = 64.0;
const EXPANDED_WIDTH: f64 = 400.0;

struct WidgetState {
    hub_base: String,
    /// (x, y, height) of the right-docked window, physical pixels
    dock: (i32, i32, u32),
    scale: f64,
}

fn main() {
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

    /* ── 1. hub: probe for an already-running hub first ──
     * A TCP bind probe is unreliable on Windows (a specific-address bind
     * can succeed over an existing wildcard bind), so probe HTTP instead. */
    let external_hub = probe_hub(&hub_base);
    if external_hub {
        eprintln!("nimon-widget: hub already running at {hub_base} — attach mode");
    } else if want_hub {
        let root = root.clone();
        std::thread::Builder::new()
            .name("nimon-hub".into())
            .spawn(move || {
                let rt = tokio::runtime::Runtime::new().expect("hub runtime");
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
            set_expanded,
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
            let (wx, wy, ww, wh) = (
                area.position.x as i64,
                area.position.y as i64,
                area.size.width as i64,
                area.size.height as i64,
            );

            let rail_px = (RAIL_WIDTH * scale) as i64;
            let height = wh as u32;
            let x = wx + ww - rail_px; // right edge
            let y = wy as i32;

            let window = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::App("index.html".into()),
            )
                .title("NIMon Widget")
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .shadow(false)
                .focused(false)
                .inner_size(RAIL_WIDTH, wh as f64)
                .position(x as f64, y as f64)
                .build()?;

            window.set_size(PhysicalSize::new(rail_px as u32, height))?;
            window.set_position(PhysicalPosition::new(x as i32, y))?;

            app.manage(WidgetState {
                hub_base,
                dock: (x as i32, y, height),
                scale,
            });

            /* tray icon drawn in code — no asset files needed */
            let icon = tauri::image::Image::new_owned(tray_icon_rgba(), 32, 32);
            let open = tauri::menu::MenuItem::with_id(
                app, "open", "Open Dashboard", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(
                app, "quit", "Quit NIMon", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&open, &quit])?;
            tauri::tray::TrayIconBuilder::with_id("nimon")
                .icon(icon)
                .tooltip("NIMon Widget")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => open_dashboard_cmd(app),
                    "quit" => app.exit(0),
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
        .unwrap_or_else(|| "http://localhost:9090".to_string());
    tauri::async_runtime::spawn_blocking(move || poll_blocking(&base))
        .await
        .map_err(|e| e.to_string())
}

fn poll_blocking(base: &str) -> Value {
    let get = |path: &str| -> Result<Value, String> {
        ureq::get(&format!("{base}{path}"))
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
            let n = FAILURES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

#[tauri::command]
fn set_expanded(app: AppHandle, expanded: bool) {
    let Some(state) = app.try_state::<WidgetState>() else { return };
    let Some(window) = app.get_webview_window("main") else { return };
    let (x, y, height) = state.dock;
    let rail_px = (RAIL_WIDTH * state.scale) as i32;
    let width = ((if expanded { EXPANDED_WIDTH } else { RAIL_WIDTH }
    ) * state.scale) as i32;
    let _ = window.set_size(PhysicalSize::new(width as u32, height));
    let _ = window.set_position(PhysicalPosition::new(x + rail_px - width, y));
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
        .unwrap_or_else(|| "http://localhost:9090".to_string());
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
        ApiConfig, BufferConfig, EdgeConfig, LoggingConfig, NodeConfig, PredictionConfig,
    };
    EdgeConfig {
        node: NodeConfig {
            id: "edge-01".to_string(),
            name: "Local Edge 1".to_string(),
            hub_address: format!("localhost:{port}"),
            reconnect_interval_secs: 5,
        },
        api: ApiConfig::default(),
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
            // rounded-rect mask
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
