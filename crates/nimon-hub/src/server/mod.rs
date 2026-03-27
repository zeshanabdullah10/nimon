//! Hub server implementation
//!
//! Provides WebSocket server for edge connections and REST API for status/health.

pub mod routes;
pub mod ws;

use std::net::SocketAddr;
use std::sync::Arc;

use actix::Actor;
use axum::{
    extract::ws::{WebSocket, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, Router},
};
use futures_util::{StreamExt, SinkExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tower_http::{
    cors::CorsLayer,
    trace::TraceLayer,
};
use tracing::{debug, error, info};

use crate::action::executor::ActionExecutor;
use crate::alert::manager::{AlertManager, AlertManagerConfig, GetActiveAlerts};
use crate::session::SessionStore;

/// Hub server state
#[derive(Clone)]
pub struct HubState {
    /// Connected edge sessions
    sessions: SessionStore,
    /// Alert manager actor address
    alert_manager: Arc<Mutex<Option<actix::Addr<AlertManager>>>>,
}

impl HubState {
    /// Create a new hub state
    pub fn new() -> Self {
        Self {
            sessions: SessionStore::new(),
            alert_manager: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a new hub state with an alert manager
    pub fn with_alert_manager(alert_manager: actix::Addr<AlertManager>) -> Self {
        Self {
            sessions: SessionStore::new(),
            alert_manager: Arc::new(Mutex::new(Some(alert_manager))),
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

    /// Get the session store
    pub fn sessions(&self) -> &SessionStore {
        &self.sessions
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

/// Handler to get active alerts from the AlertManager
async fn get_alerts_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
) -> impl IntoResponse {
    let empty_response = || Json(AlertsResponse { alerts: vec![], total: 0 });

    match state.alert_manager().await {
        Some(addr) => {
            let alerts = match addr.send(GetActiveAlerts).await {
                Ok(alerts) => alerts,
                Err(e) => {
                    error!("Failed to query AlertManager: {}", e);
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        empty_response(),
                    ).into_response();
                }
            };
            let total = alerts.len();
            Json(AlertsResponse { alerts, total }).into_response()
        }
        None => {
            empty_response().into_response()
        }
    }
}

/// Start the hub server
pub async fn run() -> anyhow::Result<()> {
    let state = HubState::new();

    // Create and start the ActionExecutor actor
    let action_executor = ActionExecutor::new().start();

    // Create and start the AlertManager actor with action executor for auto-remediation
    let alert_manager = AlertManager::new(
        AlertManagerConfig::default(),
        state.sessions().clone(),
    )
    .with_action_executor(action_executor);
    let alert_manager_addr = alert_manager.start();
    state.set_alert_manager(alert_manager_addr).await;

    info!("AlertManager actor started with auto-remediation enabled");

    // Build our application with routes
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(health_handler))
        .route("/api/status", get(status_handler))
        .route("/api/alerts", get(get_alerts_handler))
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    // Bind TCP listener
    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    let listener = TcpListener::bind(addr).await?;

    info!("Hub server listening on {}", addr);

    // Start the server
    axum::serve(listener, app).await?;

    Ok(())
}

/// WebSocket handler for edge connections
async fn ws_handler(
    ws: WebSocketUpgrade,
    axum::extract::State(state): axum::extract::State<HubState>,
) -> impl IntoResponse {
    info!("WebSocket connection request from edge");
    ws.on_upgrade(move |socket| ws_socket_handler(socket, state))
}

/// Handle WebSocket connection
async fn ws_socket_handler(socket: WebSocket, state: HubState) {
    info!("WebSocket connection established");

    // Split the socket into sender and receiver
    let (mut sender, mut receiver) = socket.split();

    // TODO: Implement proper session management and message handling
    // For now, just echo back messages

    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(axum::extract::ws::Message::Text(text)) => {
                debug!("Received text message: {}", text);

                // Echo back
                if sender
                    .send(axum::extract::ws::Message::Text(text))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(axum::extract::ws::Message::Close(_)) => {
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

    info!("WebSocket connection closed");
}

/// Health check handler
async fn health_handler(
    axum::extract::State(state): axum::extract::State<HubState>,
) -> impl IntoResponse {
    let connected = state.sessions().len();

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
}
