//! Hub server implementation
//!
//! Provides WebSocket server for edge connections and REST API for status/health.

pub mod routes;
pub mod ws;

use std::net::SocketAddr;

use axum::{
    extract::ws::{WebSocket, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, Router},
};
use futures_util::{StreamExt, SinkExt};
use tokio::net::TcpListener;
use tower_http::{
    cors::CorsLayer,
    trace::TraceLayer,
};
use tracing::{debug, error, info};

use crate::session::SessionStore;

/// Hub server state
#[derive(Clone)]
pub struct HubState {
    /// Connected edge sessions
    sessions: SessionStore,
}

impl HubState {
    /// Create a new hub state
    pub fn new() -> Self {
        Self {
            sessions: SessionStore::new(),
        }
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

/// Start the hub server
pub async fn run() -> anyhow::Result<()> {
    let state = HubState::new();

    // Build our application with routes
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(health_handler))
        .route("/api/status", get(status_handler))
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
}
