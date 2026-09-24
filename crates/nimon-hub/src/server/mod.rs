//! Hub server implementation
//!
//! Provides the WebSocket endpoint for edge connections, the REST API
//! and the embedded dashboard. See [`build_router`] for the route table.

pub mod auth;
pub mod routes;
pub mod ws;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use actix::Actor;
use axum::{
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::{
    compression::CompressionLayer,
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};
use tracing::{error, info, warn};

use crate::action::executor::ActionExecutor;
use crate::alert::manager::{
    AlertManager, AlertManagerConfig, EdgeActivity, EdgeActivityKind, FlushMetrics,
};
use crate::config::{AuthConfig, EdgeDesiredConfig, HubConfig};
use crate::db_writer::DbWriter;
use crate::session::SessionStore;

use nimon_core::actor::messages::{ConfigUpdate, ThresholdConfig};
use nimon_core::{DEFAULT_TEMP_CRITICAL_C, DEFAULT_TEMP_WARNING_C};

/// Embedded single-file dashboard build (see web/; rebuilt via `npm run build`).
const EMBEDDED_DASHBOARD: &str = include_str!("../../../../web/dist/index.html");

/// How often silent edge sessions are swept
const SESSION_SWEEP_INTERVAL: Duration = Duration::from_secs(15);

/// Hub server state (cheap to clone; clones share everything)
#[derive(Clone)]
pub struct HubState {
    /// Connected edge sessions
    sessions: Arc<SessionStore>,
    /// Alert manager actor address (set once at startup)
    alert_manager: Arc<OnceLock<actix::Addr<AlertManager>>>,
    /// Action executor actor address (set once at startup)
    action_executor: Arc<OnceLock<actix::Addr<ActionExecutor>>>,
    /// Database pool (reads)
    db_pool: Option<SqlitePool>,
    /// Ordered database writer (writes)
    db_writer: Option<DbWriter>,
    /// Hub configuration
    config: Arc<HubConfig>,
    /// Effective auth settings (config + env overrides)
    auth: Arc<AuthConfig>,
    /// Desired-state overrides pushed through the API, per edge
    edge_overrides: Arc<DashMap<String, EdgeDesiredConfig>>,
    /// Devices reported removed by their edge (hidden from the API until seen again)
    removed_devices: Arc<DashMap<String, DateTime<Utc>>>,
    /// Process start (uptime)
    started_at: std::time::Instant,
    /// Cancelled on server shutdown (closes edge sockets)
    shutdown: CancellationToken,
    /// WebSocket connections must register within this time
    registration_timeout: Duration,
}

impl HubState {
    /// Create a hub state with the default config and no database (tests)
    pub fn new() -> Self {
        Self::with_config(HubConfig::default(), None, None)
    }

    /// Create a hub state with a database pool (private ordered writer)
    pub fn with_db_pool(pool: SqlitePool) -> Self {
        let writer = DbWriter::spawn(pool.clone());
        Self::with_config(HubConfig::default(), Some(pool), Some(writer))
    }

    /// Create a hub state with an alert manager
    pub fn with_alert_manager(alert_manager: actix::Addr<AlertManager>) -> Self {
        let state = Self::new();
        state.set_alert_manager(alert_manager);
        state
    }

    /// Create a hub state from a config, optional pool and writer. Auth
    /// tokens are resolved against the environment here.
    pub fn with_config(
        config: HubConfig,
        db_pool: Option<SqlitePool>,
        db_writer: Option<DbWriter>,
    ) -> Self {
        let auth = config.auth.resolved();
        Self {
            sessions: Arc::new(SessionStore::new()),
            alert_manager: Arc::new(OnceLock::new()),
            action_executor: Arc::new(OnceLock::new()),
            db_pool,
            db_writer,
            config: Arc::new(config),
            auth: Arc::new(auth),
            edge_overrides: Arc::new(DashMap::new()),
            removed_devices: Arc::new(DashMap::new()),
            started_at: std::time::Instant::now(),
            shutdown: CancellationToken::new(),
            registration_timeout: ws::REGISTRATION_TIMEOUT,
        }
    }

    /// Replace the effective auth settings (tests)
    pub fn with_auth(mut self, auth: AuthConfig) -> Self {
        self.auth = Arc::new(auth);
        self
    }

    /// Override the edge registration deadline (tests; default 15s).
    /// Applies to connections accepted through routers built afterwards.
    pub fn with_registration_timeout(mut self, timeout: Duration) -> Self {
        self.registration_timeout = timeout;
        self
    }

    /// Time a WebSocket connection has to send a valid `edge_register`
    pub fn registration_timeout(&self) -> Duration {
        self.registration_timeout
    }

    /// Set the alert manager (first call wins). Returns false when already set.
    pub fn set_alert_manager(&self, addr: actix::Addr<AlertManager>) -> bool {
        self.alert_manager.set(addr).is_ok()
    }

    /// Get the alert manager address
    pub fn alert_manager(&self) -> Option<actix::Addr<AlertManager>> {
        self.alert_manager.get().cloned()
    }

    /// Set the action executor (first call wins). Returns false when already set.
    pub fn set_action_executor(&self, addr: actix::Addr<ActionExecutor>) -> bool {
        self.action_executor.set(addr).is_ok()
    }

    /// Get the action executor address
    pub fn action_executor(&self) -> Option<actix::Addr<ActionExecutor>> {
        self.action_executor.get().cloned()
    }

    /// Get the session store
    pub fn sessions(&self) -> Arc<SessionStore> {
        self.sessions.clone()
    }

    /// Get the database pool
    pub fn db_pool(&self) -> Option<SqlitePool> {
        self.db_pool.clone()
    }

    /// Get the ordered database writer
    pub fn db_writer(&self) -> Option<DbWriter> {
        self.db_writer.clone()
    }

    /// Hub configuration
    pub fn config(&self) -> &HubConfig {
        &self.config
    }

    /// Effective auth settings
    pub fn auth(&self) -> &AuthConfig {
        &self.auth
    }

    /// Token cancelled when the server shuts down
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// API-pushed desired-state override for an edge
    pub fn edge_override(&self, edge_id: &str) -> Option<EdgeDesiredConfig> {
        self.edge_overrides.get(edge_id).map(|o| o.clone())
    }

    /// Merge `update` into the API override of `edge_id`; returns the merged override
    pub fn merge_edge_override(
        &self,
        edge_id: &str,
        update: &EdgeDesiredConfig,
    ) -> EdgeDesiredConfig {
        let mut entry = self.edge_overrides.entry(edge_id.to_string()).or_default();
        entry.overlay(update);
        entry.clone()
    }

    /// Merge `update` into the API override of `edge_id` only when the
    /// resulting effective desired state (config + merged override)
    /// passes `check`. The override entry stays locked from read to
    /// write, so concurrent requests cannot combine into an invalid
    /// state. Returns the effective desired state that was stored.
    pub fn try_merge_edge_override(
        &self,
        edge_id: &str,
        update: &EdgeDesiredConfig,
        check: impl FnOnce(&EdgeDesiredConfig) -> Result<(), String>,
    ) -> Result<EdgeDesiredConfig, String> {
        use dashmap::mapref::entry::Entry;
        let entry = self.edge_overrides.entry(edge_id.to_string());
        let mut merged = match &entry {
            Entry::Occupied(o) => o.get().clone(),
            Entry::Vacant(_) => EdgeDesiredConfig::default(),
        };
        merged.overlay(update);
        let mut effective = self.config.edges.resolve(edge_id);
        effective.overlay(&merged);
        check(&effective)?;
        entry.insert(merged);
        Ok(effective)
    }

    /// Edge ids with an API override
    pub fn edges_with_overrides(&self) -> Vec<String> {
        self.edge_overrides
            .iter()
            .map(|e| e.key().clone())
            .collect()
    }

    /// Effective desired state: config defaults, config override, API override
    pub fn desired_for(&self, edge_id: &str) -> EdgeDesiredConfig {
        let mut desired = self.config.edges.resolve(edge_id);
        if let Some(ov) = self.edge_override(edge_id) {
            desired.overlay(&ov);
        }
        desired
    }

    /// Mark a device removed (hidden from the API until it reports again)
    pub fn mark_device_removed(&self, device_id: &str) {
        self.removed_devices
            .insert(device_id.to_string(), Utc::now());
    }

    /// Clear the removed mark (device reported again)
    pub fn mark_device_present(&self, device_id: &str) {
        if !self.removed_devices.is_empty() {
            self.removed_devices.remove(device_id);
        }
    }

    /// True when the device was reported removed
    pub fn is_device_removed(&self, device_id: &str) -> bool {
        self.removed_devices.contains_key(device_id)
    }

    /// Seconds since the state was created
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}

impl Default for HubState {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the config push for an edge from a desired state. Peers speaking
/// protocol 1.0 require full thresholds (resolved against the defaults);
/// 1.1+ peers receive only the fields that are set.
pub fn desired_to_config_update(desired: &EdgeDesiredConfig, legacy: bool) -> Option<ConfigUpdate> {
    if desired.is_empty() {
        return None;
    }
    let thresholds = if legacy {
        ThresholdConfig::full(
            desired
                .temperature_warning
                .unwrap_or(DEFAULT_TEMP_WARNING_C),
            desired
                .temperature_critical
                .unwrap_or(DEFAULT_TEMP_CRITICAL_C),
        )
    } else {
        ThresholdConfig {
            temperature_warning: desired.temperature_warning,
            temperature_critical: desired.temperature_critical,
        }
    };
    Some(ConfigUpdate {
        poll_interval_secs: desired.poll_interval_secs,
        thresholds,
    })
}

/// Start the executor and alert manager actors (and the session sweeper)
/// for `config`, persisting to `pool`. Must run inside an actix system.
pub fn start_hub_services(config: HubConfig, pool: SqlitePool) -> HubState {
    let writer = DbWriter::spawn(pool.clone());
    let state = HubState::with_config(config, Some(pool), Some(writer.clone()));

    let executor = ActionExecutor::new()
        .with_sessions(state.sessions())
        .with_db_writer(writer.clone())
        .start();
    state.set_action_executor(executor.clone());

    let alert_config = AlertManagerConfig::from_hub_config(state.config());
    let alert_manager = AlertManager::new(alert_config)
        .with_action_executor(executor)
        .with_db_writer(writer)
        .with_sessions(state.sessions())
        .start();
    state.set_alert_manager(alert_manager);

    tokio::spawn(session_sweeper(state.clone()));
    info!("AlertManager and ActionExecutor started (database persistence enabled)");
    state
}

/// Close sessions of edges that went silent and mark them offline.
async fn session_sweeper(state: HubState) {
    let timeout = state.config().edges.session_timeout_secs;
    let mut interval = tokio::time::interval(SESSION_SWEEP_INTERVAL);
    let shutdown = state.shutdown_token();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = interval.tick() => {}
        }
        for edge_id in state.sessions().remove_stale(timeout).await {
            ws::mark_edge_offline(&state, &edge_id);
        }
    }
}

/// Build the HTTP router (routes, auth, CORS, tracing) for `state`.
///
/// Routes: `GET /ws`, `GET /health`, `GET /`, `GET /api/v1/status`,
/// `GET /api/v1/settings`, `GET /api/v1/alerts`,
/// `GET /api/v1/alerts/history`, `POST /api/v1/alerts/:id/acknowledge`,
/// `POST /api/v1/alerts/:id/resolve`, `GET /api/v1/predictions`,
/// `GET /api/v1/edges`, `GET /api/v1/edges/:id`,
/// `GET /api/v1/edges/:id/devices`, `POST /api/v1/edges/:id/config`,
/// `GET /api/v1/devices`, `GET /api/v1/devices/:device_id/metrics`,
/// `POST /api/v1/devices/:device_id/actions`, `GET /api/v1/actions`.
pub fn build_router(state: HubState) -> Router {
    let dashboard = Router::new()
        .route("/", get(dashboard_handler))
        .layer(CompressionLayer::new());

    let api = Router::new()
        .route("/health", get(routes::health_handler))
        .route("/api/v1/status", get(routes::status_handler))
        .route("/api/v1/settings", get(routes::settings_handler))
        .route("/api/v1/alerts", get(routes::get_alerts_handler))
        .route(
            "/api/v1/alerts/history",
            get(routes::get_alert_history_handler),
        )
        .route(
            "/api/v1/alerts/:alert_id/acknowledge",
            post(routes::acknowledge_alert_handler),
        )
        .route(
            "/api/v1/alerts/:alert_id/resolve",
            post(routes::resolve_alert_handler),
        )
        .route("/api/v1/predictions", get(routes::get_predictions_handler))
        .route("/api/v1/edges", get(routes::list_edges))
        .route("/api/v1/edges/:edge_id", get(routes::get_edge))
        .route(
            "/api/v1/edges/:edge_id/devices",
            get(routes::get_edge_devices),
        )
        .route(
            "/api/v1/edges/:edge_id/config",
            post(routes::push_edge_config_handler),
        )
        .route("/api/v1/devices", get(routes::list_devices_handler))
        .route(
            "/api/v1/devices/:device_id/metrics",
            get(routes::device_metrics_handler),
        )
        .route(
            "/api/v1/devices/:device_id/actions",
            post(routes::device_action_handler),
        )
        .route("/api/v1/actions", get(routes::list_actions_handler));

    let mut router = Router::new()
        .route("/ws", get(ws::ws_handler))
        .merge(api)
        .merge(dashboard)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::api_auth,
        ));

    if let Some(cors) = cors_layer(&state.config().cors_allowed_origins) {
        router = router.layer(cors);
    }

    router.layer(TraceLayer::new_for_http()).with_state(state)
}

/// CORS layer for the configured origins (None when empty).
fn cors_layer(origins: &[String]) -> Option<CorsLayer> {
    if origins.is_empty() {
        return None;
    }
    let allow_origin = if origins.iter().any(|o| o.trim() == "*") {
        AllowOrigin::any()
    } else {
        let list: Vec<HeaderValue> = origins
            .iter()
            .filter_map(|o| match HeaderValue::from_str(o.trim()) {
                Ok(v) => Some(v),
                Err(_) => {
                    warn!("Ignoring invalid CORS origin '{}'", o);
                    None
                }
            })
            .collect();
        AllowOrigin::list(list)
    };
    Some(
        CorsLayer::new()
            .allow_origin(allow_origin)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]),
    )
}

fn dashboard_etag() -> &'static str {
    static ETAG: OnceLock<String> = OnceLock::new();
    ETAG.get_or_init(|| {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        EMBEDDED_DASHBOARD.hash(&mut hasher);
        format!("\"{:x}-{}\"", hasher.finish(), EMBEDDED_DASHBOARD.len())
    })
}

/// Embedded dashboard (gzip via the compression layer; revalidated with
/// an ETag so a rebuilt hub never serves a stale page).
async fn dashboard_handler(headers: HeaderMap) -> Response {
    let etag = dashboard_etag();
    let matches = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(',').any(|t| t.trim() == etag))
        .unwrap_or(false);
    let cache = [(header::CACHE_CONTROL, "no-cache"), (header::ETAG, etag)];
    if matches {
        return (StatusCode::NOT_MODIFIED, cache).into_response();
    }
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        cache,
        EMBEDDED_DASHBOARD,
    )
        .into_response()
}

/// Parse the configured bind host (`localhost` is accepted).
pub fn parse_host(host: &str) -> anyhow::Result<IpAddr> {
    let host = host.trim();
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .map_err(|_| {
            anyhow::anyhow!(
                "invalid 'host' value '{}': expected an IP address such as 0.0.0.0, 127.0.0.1 or ::",
                host
            )
        })
}

/// Future resolving on Ctrl+C or (unix) SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!("Failed to listen for Ctrl+C: {}", e);
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                error!("Failed to listen for SIGTERM: {}", e);
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    info!("Shutdown signal received, gracefully stopping...");
}

/// Start the hub server (console mode: shuts down on Ctrl+C / SIGTERM)
pub async fn run(config: HubConfig) -> anyhow::Result<()> {
    run_with_shutdown(config, shutdown_signal()).await
}

/// Open the database for `config`, creating the parent directory.
pub async fn open_database(config: &HubConfig) -> anyhow::Result<SqlitePool> {
    let raw = config.database_path.trim();
    let target = if raw == ":memory:" || raw.starts_with("sqlite:") {
        raw.to_string()
    } else {
        let path = std::path::Path::new(raw);
        let path = if path.is_relative() {
            std::env::current_dir().unwrap_or_default().join(path)
        } else {
            path.to_path_buf()
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow::anyhow!(
                    "cannot create database directory {}: {}",
                    parent.display(),
                    e
                )
            })?;
        }
        path.display().to_string()
    };

    let pool = nimon_core::db::connect(&target)
        .await
        .map_err(|e| anyhow::anyhow!("cannot open database {}: {}", target, e))?;
    let report = nimon_core::db::migrate(&pool).await?;
    info!(
        "Database ready at {} (schema v{} -> v{})",
        target, report.from_version, report.to_version
    );
    if !report.foreign_key_violations.is_empty() {
        warn!(
            "Database has {} pre-existing foreign key violations (left in place)",
            report.foreign_key_violations.len()
        );
        for v in report.foreign_key_violations.iter().take(20) {
            warn!(
                "  FK violation: table {} rowid {:?} -> parent {}",
                v.table, v.rowid, v.parent
            );
        }
    }
    Ok(pool)
}

/// Start the hub server with a custom shutdown future (service mode uses
/// an SCM stop-request channel instead of Ctrl+C).
pub async fn run_with_shutdown(
    config: HubConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    config.validate()?;
    let addr = SocketAddr::new(parse_host(&config.host)?, config.port);
    let pool = open_database(&config).await?;

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow::anyhow!("cannot listen on {}: {}", addr, e))?;

    let state = start_hub_services(config, pool);
    if state.auth().api_token.is_some() {
        info!("API write authentication enabled");
    }
    if state.auth().edge_token.is_some() {
        info!("Edge WebSocket authentication enabled");
    }
    let app = build_router(state.clone());

    info!("Hub server listening on {}", addr);

    let token = state.shutdown_token();
    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.await;
            token.cancel();
        })
        .await;

    // Best effort: persist buffered metrics and drain queued writes
    state.shutdown_token().cancel();
    if let Some(am) = state.alert_manager() {
        let _ = tokio::time::timeout(Duration::from_secs(2), am.send(FlushMetrics)).await;
    }
    if let Some(writer) = state.db_writer() {
        let _ = tokio::time::timeout(Duration::from_secs(5), writer.flush()).await;
    }

    serve_result?;
    info!("Hub server shut down gracefully");
    Ok(())
}

/// Tell the alert manager an edge connected / disconnected.
pub(crate) fn notify_edge_activity(state: &HubState, edge_id: &str, kind: EdgeActivityKind) {
    if let Some(am) = state.alert_manager() {
        am.do_send(EdgeActivity {
            edge_id: edge_id.to_string(),
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hub_state_creation() {
        let state = HubState::new();
        assert_eq!(state.sessions().len(), 0);
        assert!(state.alert_manager().is_none());
        assert!(state.action_executor().is_none());
    }

    #[test]
    fn test_parse_host() {
        assert_eq!(
            parse_host("0.0.0.0").unwrap(),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(
            parse_host("localhost").unwrap(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert!(parse_host("::").is_ok());
        assert!(parse_host("[::1]").is_ok());
        let err = parse_host("not a host").unwrap_err().to_string();
        assert!(err.contains("invalid 'host'"), "{}", err);
    }

    #[test]
    fn test_desired_to_config_update() {
        assert!(desired_to_config_update(&EdgeDesiredConfig::default(), false).is_none());
        let desired = EdgeDesiredConfig {
            poll_interval_secs: Some(30),
            temperature_warning: Some(60.0),
            temperature_critical: None,
        };
        // 1.1: only the provided fields
        let update = desired_to_config_update(&desired, false).unwrap();
        assert_eq!(update.poll_interval_secs, Some(30));
        assert_eq!(update.thresholds.temperature_warning, Some(60.0));
        assert_eq!(update.thresholds.temperature_critical, None);
        // 1.0: full thresholds resolved against the defaults
        let update = desired_to_config_update(&desired, true).unwrap();
        assert_eq!(update.thresholds, ThresholdConfig::full(60.0, 75.0));
    }

    #[test]
    fn test_desired_for_merges_api_override() {
        let yaml = "edges:\n  defaults:\n    temperature_warning: 60\n    poll_interval_secs: 20\n";
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        let state = HubState::with_config(config, None, None);
        state.merge_edge_override(
            "e1",
            &EdgeDesiredConfig {
                temperature_critical: Some(90.0),
                ..Default::default()
            },
        );
        let desired = state.desired_for("e1");
        assert_eq!(desired.temperature_warning, Some(60.0));
        assert_eq!(desired.temperature_critical, Some(90.0));
        assert_eq!(desired.poll_interval_secs, Some(20));
        assert_eq!(state.desired_for("e2").temperature_critical, None);
    }

    #[test]
    fn test_concurrent_config_pushes_never_store_warning_above_critical() {
        // Defaults 65/75. Alone, warning=72 and critical=70 are both valid;
        // together they are not. Racing them must never store both.
        for _ in 0..50 {
            let state = HubState::new();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let handles: Vec<_> = [
                EdgeDesiredConfig {
                    temperature_warning: Some(72.0),
                    ..Default::default()
                },
                EdgeDesiredConfig {
                    temperature_critical: Some(70.0),
                    ..Default::default()
                },
            ]
            .into_iter()
            .map(|update| {
                let (state, barrier) = (state.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    state
                        .try_merge_edge_override("e1", &update, routes::check_threshold_order)
                        .is_ok()
                })
            })
            .collect();
            let accepted: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert_eq!(
                accepted.iter().filter(|ok| **ok).count(),
                1,
                "{:?}",
                accepted
            );
            assert!(routes::check_threshold_order(&state.desired_for("e1")).is_ok());
        }
        // A rejected push on a fresh edge stores nothing
        let state = HubState::new();
        let bad = EdgeDesiredConfig {
            temperature_warning: Some(99.0),
            ..Default::default()
        };
        assert!(state
            .try_merge_edge_override("e2", &bad, routes::check_threshold_order)
            .is_err());
        assert!(state.edge_override("e2").is_none());
    }

    #[test]
    fn test_cors_layer_only_when_configured() {
        assert!(cors_layer(&[]).is_none());
        assert!(cors_layer(&["https://ops.example.com".to_string()]).is_some());
        assert!(cors_layer(&["*".to_string()]).is_some());
    }
}
