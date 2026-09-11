//! Hub server implementation
//!
//! Provides WebSocket server for edge connections and REST API for status/health.

pub mod routes;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use actix::Actor;
use axum::{
    extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{debug, error, info, warn};

use crate::action::executor::{ActionExecutor, CompleteAction};
use crate::alert::manager::{
    AlertManager, AlertManagerConfig, GetActiveAlerts, GetAlertHistory, GetRecentPredictions,
    IngestDeviceAlert,
};
use crate::config::{try_rule_config_to_alert_rule, EdgeDesiredConfig, HubConfig};
use crate::session::{EdgeSession, HeartbeatInfo, SessionStore};

use nimon_core::actor::messages::{ConfigUpdate, ThresholdConfig};
use nimon_core::db::edge_repo::EdgeRepository;
use nimon_core::protocol::{WsMessage, WsMessageType};
use nimon_core::{EdgeNode, EdgeStatus};

/// Embedded single-file dashboard build (see web/; rebuilt via `npm run build`).
/// Falls back to CWD files for development when the embed is unavailable.
const EMBEDDED_DASHBOARD: &str = include_str!("../../../../web/dist/index.html");

/// Hub server state
#[derive(Clone)]
pub struct HubState {
    /// Connected edge sessions (shared via Arc)
    sessions: Arc<SessionStore>,
    /// Alert manager actor address
    alert_manager: Arc<Mutex<Option<actix::Addr<AlertManager>>>>,
    /// Action executor actor address
    action_executor: Arc<Mutex<Option<actix::Addr<ActionExecutor>>>>,
    /// Database pool for edge persistence
    db_pool: Option<Arc<SqlitePool>>,
    /// Desired-state config pushed to edges on registration
    edges_config: crate::config::EdgesConfig,
}

impl HubState {
    /// Create a new hub state (without database pool - for testing)
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(SessionStore::new()),
            alert_manager: Arc::new(Mutex::new(None)),
            action_executor: Arc::new(Mutex::new(None)),
            db_pool: None,
            edges_config: Default::default(),
        }
    }

    /// Create a new hub state with a database pool
    pub fn with_db_pool(pool: SqlitePool) -> Self {
        Self {
            sessions: Arc::new(SessionStore::new()),
            alert_manager: Arc::new(Mutex::new(None)),
            action_executor: Arc::new(Mutex::new(None)),
            db_pool: Some(Arc::new(pool)),
            edges_config: Default::default(),
        }
    }

    /// Create a new hub state with an alert manager (for backwards compatibility)
    pub fn with_alert_manager(alert_manager: actix::Addr<AlertManager>) -> Self {
        Self {
            sessions: Arc::new(SessionStore::new()),
            alert_manager: Arc::new(Mutex::new(Some(alert_manager))),
            action_executor: Arc::new(Mutex::new(None)),
            db_pool: None,
            edges_config: Default::default(),
        }
    }

    /// Set the alert manager
    pub async fn set_alert_manager(&self, addr: actix::Addr<AlertManager>) {
        let mut guard = self.alert_manager.lock().await;
        *guard = Some(addr);
    }

    /// Get the alert manager address
    pub async fn alert_manager(&self) -> Option<actix::Addr<AlertManager>> {
        let guard = self.alert_manager.lock().await;
        guard.clone()
    }

    /// Set the action executor
    pub async fn set_action_executor(&self, addr: actix::Addr<ActionExecutor>) {
        let mut guard = self.action_executor.lock().await;
        *guard = Some(addr);
    }

    /// Get the action executor address
    pub async fn action_executor(&self) -> Option<actix::Addr<ActionExecutor>> {
        let guard = self.action_executor.lock().await;
        guard.clone()
    }

    /// Get the session store
    pub fn sessions(&self) -> Arc<SessionStore> {
        self.sessions.clone()
    }

    /// Get the database pool
    pub fn db_pool(&self) -> Option<Arc<SqlitePool>> {
        self.db_pool.clone()
    }
}

impl Default for HubState {
    fn default() -> Self {
        Self::new()
    }
}

/// Health check response
#[derive(serde::Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub connected_edges: usize,
}

/// Alerts response
#[derive(serde::Serialize)]
pub struct AlertsResponse {
    pub alerts: Vec<nimon_core::alert::Alert>,
    pub total: usize,
}

/// Alert history response
#[derive(serde::Serialize)]
pub struct AlertHistoryResponse {
    pub alerts: Vec<serde_json::Value>,
    pub total: usize,
}

/// Predictions response
#[derive(serde::Serialize)]
pub struct PredictionsResponse {
    pub predictions: Vec<serde_json::Value>,
    pub total: usize,
}

/// Handler to get active alerts from the AlertManager
async fn get_alerts_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
) -> impl IntoResponse {
    let empty_response = || {
        Json(AlertsResponse {
            alerts: vec![],
            total: 0,
        })
    };

    match state.alert_manager().await {
        Some(addr) => {
            let alerts = match addr.send(GetActiveAlerts).await {
                Ok(alerts) => alerts,
                Err(e) => {
                    error!("Failed to query AlertManager: {}", e);
                    return (StatusCode::SERVICE_UNAVAILABLE, empty_response()).into_response();
                }
            };
            let total = alerts.len();
            Json(AlertsResponse { alerts, total }).into_response()
        }
        None => empty_response().into_response(),
    }
}

/// Handler to get alert history from the database
async fn get_alert_history_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let limit = params
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100);

    match state.alert_manager().await {
        Some(addr) => {
            match addr.send(GetAlertHistory { limit }).await {
                Ok(Ok(mut alerts)) => {
                    // Apply client-side filtering
                    if let Some(severity) = params.get("severity") {
                        alerts.retain(|a| {
                            a.get("severity").and_then(|v| v.as_str()) == Some(severity)
                        });
                    }
                    if let Some(edge_id) = params.get("edge_id") {
                        alerts
                            .retain(|a| a.get("edge_id").and_then(|v| v.as_str()) == Some(edge_id));
                    }
                    if let Some(device_id) = params.get("device_id") {
                        alerts.retain(|a| {
                            a.get("device_id").and_then(|v| v.as_str()) == Some(device_id)
                        });
                    }
                    let total = alerts.len();
                    Json(AlertHistoryResponse { alerts, total }).into_response()
                }
                Ok(Err(e)) => {
                    error!("Failed to get alert history: {}", e);
                    Json(AlertHistoryResponse {
                        alerts: vec![],
                        total: 0,
                    })
                    .into_response()
                }
                Err(e) => {
                    error!("AlertManager unavailable: {}", e);
                    Json(AlertHistoryResponse {
                        alerts: vec![],
                        total: 0,
                    })
                    .into_response()
                }
            }
        }
        None => Json(AlertHistoryResponse {
            alerts: vec![],
            total: 0,
        })
        .into_response(),
    }
}

/// Handler to get active predictions
async fn get_predictions_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
) -> impl IntoResponse {
    match state.alert_manager().await {
        Some(addr) => match addr.send(GetRecentPredictions { limit: 100 }).await {
            Ok(Ok(predictions)) => {
                let total = predictions.len();
                Json(PredictionsResponse { predictions, total }).into_response()
            }
            Ok(Err(e)) => {
                error!("Failed to get predictions: {}", e);
                Json(PredictionsResponse {
                    predictions: vec![],
                    total: 0,
                })
                .into_response()
            }
            Err(e) => {
                error!("AlertManager unavailable: {}", e);
                Json(PredictionsResponse {
                    predictions: vec![],
                    total: 0,
                })
                .into_response()
            }
        },
        None => Json(PredictionsResponse {
            predictions: vec![],
            total: 0,
        })
        .into_response(),
    }
}

/// Handler to acknowledge (resolve) an alert
async fn acknowledge_alert_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
    axum::extract::Path(alert_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.alert_manager().await {
        Some(addr) => {
            match addr
                .send(crate::alert::manager::ResolveAlert { alert_id })
                .await
            {
                Ok(Ok(())) => Json(serde_json::json!({ "status": "resolved" })).into_response(),
                Ok(Err(e)) => (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": format!("{}", e) })),
                )
                    .into_response(),
                Err(e) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "error": format!("{}", e) })),
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "AlertManager not available" })),
        )
            .into_response(),
    }
}

/// Request body for pushing desired-state config to an edge
#[derive(serde::Deserialize)]
struct EdgeConfigRequest {
    poll_interval_secs: Option<u64>,
    temperature_warning: Option<f64>,
    temperature_critical: Option<f64>,
}

impl EdgeConfigRequest {
    fn has_any(&self) -> bool {
        self.poll_interval_secs.is_some()
            || self.temperature_warning.is_some()
            || self.temperature_critical.is_some()
    }
}

fn desired_to_config_update(desired: &EdgeDesiredConfig) -> Option<ConfigUpdate> {
    if desired.poll_interval_secs.is_none()
        && desired.temperature_warning.is_none()
        && desired.temperature_critical.is_none()
    {
        return None;
    }
    let defaults = ThresholdConfig::default();
    Some(ConfigUpdate {
        poll_interval_secs: desired.poll_interval_secs,
        thresholds: ThresholdConfig {
            temperature_warning: desired
                .temperature_warning
                .unwrap_or(defaults.temperature_warning),
            temperature_critical: desired
                .temperature_critical
                .unwrap_or(defaults.temperature_critical),
        },
    })
}

/// Push desired-state configuration to a connected edge node.
async fn push_edge_config_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
    axum::extract::Path(edge_id): axum::extract::Path<String>,
    body: Result<Json<EdgeConfigRequest>, axum::extract::rejection::JsonRejection>,
) -> axum::response::Response {
    let Ok(Json(request)) = body else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid request body" })),
        )
            .into_response();
    };

    let Some(session) = state.sessions().get(&edge_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("edge '{}' is not connected", edge_id) })),
        )
            .into_response();
    };

    if !request.has_any() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "no configuration fields provided" })),
        )
            .into_response();
    }

    let update = ConfigUpdate {
        poll_interval_secs: request.poll_interval_secs,
        thresholds: ThresholdConfig {
            temperature_warning: request
                .temperature_warning
                .unwrap_or_else(|| ThresholdConfig::default().temperature_warning),
            temperature_critical: request
                .temperature_critical
                .unwrap_or_else(|| ThresholdConfig::default().temperature_critical),
        },
    };

    match session.send(WsMessage::config_update(update)) {
        Ok(()) => Json(serde_json::json!({ "status": "pushed" })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("failed to push config: {}", e) })),
        )
            .into_response(),
    }
}

/// Handler for the dashboard: embedded single-file build first, then
/// development files relative to the CWD.
async fn dashboard_handler() -> impl axum::response::IntoResponse {
    if !EMBEDDED_DASHBOARD.is_empty() {
        return axum::response::Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html")
            .body(EMBEDDED_DASHBOARD.to_string().into())
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    for path in ["web/dist/index.html", "static/index.html"] {
        let index_path = PathBuf::from(path);
        if let Ok(body) = tokio::fs::read(&index_path).await {
            return axum::response::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/html")
                .body(body.into())
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

/// Handler for serving the dev CSS file
async fn static_css_handler() -> impl axum::response::IntoResponse {
    let path = PathBuf::from("static/style.css");
    match tokio::fs::read(&path).await {
        Ok(body) => axum::response::Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/css")
            .body(body.into())
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Handler for serving the dev JS file
async fn static_js_handler() -> impl axum::response::IntoResponse {
    let path = PathBuf::from("static/app.js");
    match tokio::fs::read(&path).await {
        Ok(body) => axum::response::Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/javascript")
            .body(body.into())
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Start the hub server (console mode: shuts down on Ctrl+C)
pub async fn run(config: HubConfig) -> anyhow::Result<()> {
    let shutdown = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
        info!("Shutdown signal received, gracefully stopping...");
    };
    run_with_shutdown(config, shutdown).await
}

/// Start the hub server with a custom shutdown future (service mode uses
/// an SCM stop-request channel instead of Ctrl+C).
pub async fn run_with_shutdown(
    config: HubConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    // Resolve database path to absolute and ensure the data directory exists
    let db_path = std::path::Path::new(&config.database_path);
    let db_path = if db_path.is_relative() {
        std::env::current_dir().unwrap_or_default().join(db_path)
    } else {
        db_path.to_path_buf()
    };
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Connect to SQLite database and initialize schema (with migrations)
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = SqlitePool::connect(&db_url).await?;
    nimon_core::db::init_database(&pool).await?;
    info!("Database initialized at {}", db_url);

    // Create HubState with the database pool
    let mut state = HubState::with_db_pool(pool.clone());
    state.edges_config = config.edges.clone();

    // Create and start the ActionExecutor actor
    let action_executor = ActionExecutor::new()
        .with_sessions(state.sessions())
        .with_db_pool(pool.clone())
        .start();
    state.set_action_executor(action_executor.clone()).await;

    // Convert config rules to AlertRule, falling back to defaults if conversion fails or empty
    let rules: Vec<nimon_core::alert::rules::AlertRule> = config
        .alert
        .rules
        .iter()
        .filter_map(|r| {
            match try_rule_config_to_alert_rule(r) {
                Ok(mut rule) => {
                    // Carry the rule's action through to the runtime rule
                    if let Some(action) = &r.action {
                        rule.action = Some(config_action_to_ref(action));
                    }
                    Some(rule)
                }
                Err(e) => {
                    error!("Failed to convert rule '{}': {}", r.name, e);
                    None
                }
            }
        })
        .collect();
    let alert_config = AlertManagerConfig {
        default_cooldown_minutes: config.alert.default_cooldown_minutes,
        max_firing_count: config.alert.max_firing_count,
        cleanup_interval_hours: config.alert.cleanup_interval_hours,
        prediction_alert_threshold: config.alert.prediction_alert_threshold,
        notification_channels: config.alert.notification_channels.clone(),
        rules,
        maintenance: config.maintenance.clone(),
    };
    let alert_manager = AlertManager::new(alert_config)
        .with_action_executor(action_executor)
        .with_db_pool(pool)
        .with_sessions(state.sessions());
    let alert_manager_addr = alert_manager.start();
    state.set_alert_manager(alert_manager_addr).await;

    info!(
        "AlertManager actor started with rule-driven remediation and database persistence enabled"
    );

    // Build our application with routes
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(health_handler))
        .route("/", get(dashboard_handler))
        .route("/style.css", get(static_css_handler))
        .route("/app.js", get(static_js_handler))
        .route("/api/v1/status", get(status_handler))
        .route("/api/v1/alerts", get(get_alerts_handler))
        .route("/api/v1/alerts/history", get(get_alert_history_handler))
        .route(
            "/api/v1/alerts/:alert_id/acknowledge",
            post(acknowledge_alert_handler),
        )
        .route("/api/v1/predictions", get(get_predictions_handler))
        .route("/api/v1/edges", get(routes::list_edges))
        .route("/api/v1/edges/:edge_id", get(routes::get_edge))
        .route(
            "/api/v1/edges/:edge_id/devices",
            get(routes::get_edge_devices),
        )
        .route(
            "/api/v1/edges/:edge_id/config",
            post(push_edge_config_handler),
        )
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Bind TCP listener
    let addr = SocketAddr::new(
        config
            .host
            .parse()
            .unwrap_or_else(|_| std::net::Ipv4Addr::UNSPECIFIED.into()),
        config.port,
    );
    let listener = TcpListener::bind(addr).await?;

    info!("Hub server listening on {}", addr);

    // Start the server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    info!("Hub server shut down gracefully");

    Ok(())
}

/// Convert a config action into the core rule action reference.
fn config_action_to_ref(
    action: &crate::config::ActionConfig,
) -> nimon_core::alert::rules::ActionRef {
    use crate::config::ActionConfig;
    use nimon_core::alert::rules::ActionRef;
    match action {
        ActionConfig::Script {
            script,
            args,
            timeout_secs,
        } => ActionRef::Script {
            script: script.clone(),
            args: args.clone(),
            timeout_secs: *timeout_secs,
        },
        ActionConfig::RestartService { service_name } => ActionRef::RestartService {
            service_name: service_name.clone(),
        },
        ActionConfig::PowerCycle { delay_secs } => ActionRef::PowerCycle {
            delay_secs: *delay_secs,
        },
        ActionConfig::EdgeCommand {
            command,
            parameters,
        } => ActionRef::EdgeCommand {
            command: command.clone(),
            parameters: parameters.clone(),
        },
        ActionConfig::CustomScript { script } => ActionRef::CustomScript {
            script: script.clone(),
        },
    }
}

/// WebSocket handler for edge connections
async fn ws_handler(
    ws: WebSocketUpgrade,
    axum::extract::State(state): axum::extract::State<HubState>,
) -> impl IntoResponse {
    info!("WebSocket connection request from edge");
    ws.on_upgrade(move |socket| async move {
        let _ = ws_socket_handler(socket, state).await;
    })
}

/// Handle WebSocket connection
async fn ws_socket_handler(socket: WebSocket, state: HubState) {
    info!("WebSocket connection established");

    // Split the socket into sender and receiver
    let (mut sender, mut receiver) = socket.split();

    // Outbound channel: hub -> this edge
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<WsMessage>();

    // Track the edge ID so we can unregister on disconnect
    let mut edge_id: Option<String> = None;
    let mut shutdown_notify: Option<Arc<tokio::sync::Notify>> = None;
    let mut replaced = false;

    // Send a message on the socket, breaking the loop on failure
    macro_rules! send_or_break {
        ($msg:expr) => {
            if let Ok(json) = $msg.to_json() {
                if sender.send(AxumWsMessage::Text(json)).await.is_err() {
                    break;
                }
            }
        };
    }

    loop {
        tokio::select! {
            // Outbound messages from the hub (actions, config pushes)
            outbound = out_rx.recv() => {
                match outbound {
                    Some(msg) => send_or_break!(msg),
                    None => break,
                }
            }
            // Duplicate registration: this socket was replaced
            _ = async {
                match &shutdown_notify {
                    Some(notify) => notify.notified().await,
                    None => std::future::pending().await,
                }
            } => {
                if shutdown_notify.is_some() {
                    info!("Edge session replaced by a new registration, closing old socket");
                    replaced = true;
                    let _ = sender.send(AxumWsMessage::Close(None)).await;
                    break;
                }
            }
            // Incoming messages from the edge
            msg = receiver.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Ok(AxumWsMessage::Text(text)) => {
                        debug!("Received text message: {}", text);

                        let ws_msg = match WsMessage::from_json(&text) {
                            Ok(m) => m,
                            Err(e) => {
                                warn!("Failed to parse WebSocket message: {}", e);
                                let err_msg = WsMessage::error(
                                    "PARSE_ERROR".to_string(),
                                    format!("Failed to parse message: {}", e),
                                    None,
                                );
                                send_or_break!(err_msg);
                                continue;
                            }
                        };

                        // Protocol version gate: same-major versions only
                        if !ws_msg.version_compatible() {
                            warn!(
                                "Incompatible protocol version {} (ours {}), rejecting",
                                ws_msg.version,
                                nimon_core::protocol::PROTOCOL_VERSION
                            );
                            let err_msg = WsMessage::error(
                                "VERSION_MISMATCH".to_string(),
                                format!(
                                    "incompatible protocol version {} (hub speaks {})",
                                    ws_msg.version,
                                    nimon_core::protocol::PROTOCOL_VERSION
                                ),
                                None,
                            );
                            send_or_break!(err_msg);
                            let _ = sender.send(AxumWsMessage::Close(None)).await;
                            break;
                        }

                        match ws_msg.msg_type {
                            WsMessageType::EdgeRegister => {
                                let registration = match ws_msg.payload::<nimon_core::actor::messages::EdgeRegister>() {
                                    Ok(r) => r,
                                    Err(e) => {
                                        warn!("Failed to parse EdgeRegister payload: {}", e);
                                        continue;
                                    }
                                };

                                info!(
                                    "Edge registered: {} ({})",
                                    registration.edge_id, registration.name
                                );

                                let session = EdgeSession::with_outbound(
                                    registration.edge_id.clone(),
                                    registration.name.clone(),
                                    registration.hostname.clone(),
                                    registration.ip_address.clone(),
                                    out_tx.clone(),
                                );

                                // Duplicate registration: replace and close the old socket
                                if let Some(old) = state.sessions().add(session.clone()) {
                                    if old.edge_id() == registration.edge_id {
                                        warn!(
                                            "Duplicate registration for edge {}, replacing session",
                                            registration.edge_id
                                        );
                                        old.request_shutdown();
                                    }
                                }
                                shutdown_notify = Some(session.shutdown_notify());

                                edge_id = Some(registration.edge_id.clone());

                                // Persist edge registration to database (fire-and-forget)
                                if let Some(pool) = state.db_pool() {
                                    let pool = pool.clone();
                                    let edge_node = EdgeNode {
                                        id: registration.edge_id.clone(),
                                        name: registration.name.clone(),
                                        hostname: registration.hostname,
                                        ip_address: registration.ip_address,
                                        last_seen: Some(chrono::Utc::now()),
                                        status: EdgeStatus::Online,
                                    };
                                    tokio::spawn(async move {
                                        let repo = EdgeRepository::new(&pool);
                                        if let Err(e) = repo.upsert(&edge_node).await {
                                            tracing::error!("Failed to upsert edge node: {}", e);
                                        }
                                    });
                                }

                                // Push desired-state config for this edge (when configured)
                                let desired = state.edges_config.resolve(&registration.edge_id);
                                if let Some(update) = desired_to_config_update(&desired) {
                                    let _ = session.send(WsMessage::config_update(update));
                                }

                                let ack = WsMessage::ack(ws_msg.msg_id, true, None);
                                send_or_break!(ack);
                            }
                            WsMessageType::DeviceStatus => {
                                let status = match ws_msg.payload::<nimon_core::actor::messages::DeviceStatusUpdate>() {
                                    Ok(s) => s,
                                    Err(e) => {
                                        warn!("Failed to parse DeviceStatus payload: {}", e);
                                        continue;
                                    }
                                };

                                debug!(
                                    "Device status from {}: device={}, status={:?}",
                                    status.edge_id, status.device_id, status.status
                                );

                                // Track latest device state on the edge session
                                if let Some(session) = state.sessions().get(&status.edge_id) {
                                    session
                                        .update_device(
                                            status.device_id.clone(),
                                            status.status,
                                            status.metrics.clone(),
                                            status.is_simulated,
                                        )
                                        .await;
                                }

                                if let Some(alert_manager) = state.alert_manager().await {
                                    alert_manager.do_send(status);
                                }
                            }
                            WsMessageType::Prediction => {
                                let prediction = match ws_msg.payload::<nimon_core::actor::messages::PredictionResult>() {
                                    Ok(p) => p,
                                    Err(e) => {
                                        warn!("Failed to parse Prediction payload: {}", e);
                                        continue;
                                    }
                                };

                                debug!(
                                    "Prediction from {}: device={}, type={}, prob={:.2}",
                                    prediction.edge_id,
                                    prediction.device_id,
                                    prediction.prediction_type,
                                    prediction.probability
                                );

                                if let Some(alert_manager) = state.alert_manager().await {
                                    alert_manager.do_send(prediction);
                                }
                            }
                            WsMessageType::DeviceAlert => {
                                match ws_msg.payload::<nimon_core::actor::messages::DeviceAlert>() {
                                    Ok(alert) => {
                                        info!("Device alert received: {}", alert.message);
                                        if let Some(alert_manager) = state.alert_manager().await {
                                            alert_manager.do_send(IngestDeviceAlert { alert });
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Failed to parse DeviceAlert payload: {}", e);
                                    }
                                }
                            }
                            WsMessageType::ActionResult => {
                                match ws_msg.payload::<nimon_core::actor::messages::ActionResult>() {
                                    Ok(result) => {
                                        debug!("Action result for {}: success={}", result.action_id, result.success);
                                        if let Some(executor) = state.action_executor().await {
                                            executor.do_send(CompleteAction {
                                                msg_id: ws_msg.msg_id.clone(),
                                                result,
                                            });
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Failed to parse ActionResult payload: {}", e);
                                    }
                                }
                            }
                            WsMessageType::Heartbeat => {
                                let payload_count = ws_msg
                                    .payload::<nimon_core::actor::messages::EdgeHeartbeat>()
                                    .ok()
                                    .map(|h| HeartbeatInfo {
                                        device_count: Some(h.device_count),
                                        status: Some(h.status),
                                    })
                                    .unwrap_or_default();

                                if let Some(ref eid) = edge_id {
                                    if let Some(session) = state.sessions().get(eid) {
                                        session.update_heartbeat(payload_count).await;
                                        debug!("Heartbeat updated for edge: {}", eid);

                                        // Update last_seen in database (fire-and-forget)
                                        if let Some(pool) = state.db_pool() {
                                            let pool = pool.clone();
                                            let eid_clone = eid.clone();
                                            tokio::spawn(async move {
                                                let repo = EdgeRepository::new(&pool);
                                                if let Err(e) = repo.update_status(&eid_clone, EdgeStatus::Online).await {
                                                    tracing::error!("Failed to update edge last_seen: {}", e);
                                                }
                                            });
                                        }
                                    }
                                }
                            }
                            WsMessageType::Ping => {
                                let ping_ts = match ws_msg.payload::<nimon_core::protocol::PingMessage>() {
                                    Ok(p) => p.timestamp,
                                    Err(_) => ws_msg.timestamp,
                                };
                                let pong = WsMessage::pong(ping_ts);
                                send_or_break!(pong);
                            }
                            _ => {
                                debug!("Unhandled message type: {:?}", ws_msg.msg_type);
                            }
                        }
                    }
                    Ok(AxumWsMessage::Close(_)) => {
                        info!("WebSocket close received");
                        break;
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

    // Remove the edge session on disconnect and mark offline in database.
    // A replaced socket (still-registered edge) must not unregister the new session.
    if !replaced {
        if let Some(ref eid) = edge_id {
            state.sessions().remove(eid);
            info!("Edge disconnected: {}", eid);

            // Mark edge as offline in database (fire-and-forget)
            if let Some(pool) = state.db_pool() {
                let pool = pool.clone();
                let eid_clone = eid.clone();
                tokio::spawn(async move {
                    let repo = EdgeRepository::new(&pool);
                    if let Err(e) = repo.update_status(&eid_clone, EdgeStatus::Offline).await {
                        tracing::error!("Failed to update edge status to offline: {}", e);
                    }
                });
            }
        }
    }

    info!("WebSocket connection closed");
}

/// Health check handler
async fn health_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
) -> impl IntoResponse {
    let sessions = state.sessions();
    let connected = sessions.len();
    debug!("health_handler: sessions.len() = {}", connected);

    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        connected_edges: connected,
    })
}

/// Status handler
async fn status_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
) -> impl IntoResponse {
    let sessions = state.sessions();
    let mut edges = Vec::new();

    for entry in sessions.iter() {
        let (edge_id, session) = entry.pair();
        let device_count = session.device_count().await;
        edges.push(serde_json::json!({
            "edge_id": edge_id,
            "name": session.name(),
            "connected_at": session.connected_at().to_rfc3339(),
            "device_count": device_count,
        }));
    }

    Json(serde_json::json!({
        "edges": edges,
        "total": sessions.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hub_state_creation() {
        let state = HubState::new();
        assert_eq!(state.sessions().len(), 0);
    }

    #[test]
    fn test_health_response_serialization() {
        let response = HealthResponse {
            status: "healthy".to_string(),
            version: "0.1.0".to_string(),
            connected_edges: 3,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("healthy"));
        assert!(json.contains("3"));
    }

    #[test]
    fn test_hub_state_default() {
        let state = HubState::default();
        assert_eq!(state.sessions().len(), 0);
    }

    #[tokio::test]
    async fn test_hub_state_alert_manager_initially_none() {
        let state = HubState::new();
        assert!(state.alert_manager().await.is_none());
        assert!(state.action_executor().await.is_none());
    }

    #[test]
    fn test_alerts_response_serialization() {
        let response = AlertsResponse {
            alerts: vec![],
            total: 0,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"alerts\""));
        assert!(json.contains("\"total\":0"));
    }

    #[test]
    fn test_desired_to_config_update() {
        assert!(desired_to_config_update(&EdgeDesiredConfig::default()).is_none());
        let update = desired_to_config_update(&EdgeDesiredConfig {
            poll_interval_secs: Some(30),
            temperature_warning: Some(65.0),
            temperature_critical: None,
        })
        .unwrap();
        assert_eq!(update.poll_interval_secs, Some(30));
        assert_eq!(update.thresholds.temperature_warning, 65.0);
        assert_eq!(update.thresholds.temperature_critical, 75.0);
    }
}
