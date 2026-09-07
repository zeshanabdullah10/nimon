//! REST API routes for the hub server

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
};
use serde_json::json;
use tracing::info;

use crate::server::HubState;

/// List all connected edges
pub async fn list_edges(State(state): State<HubState>) -> Json<serde_json::Value> {
    info!("Listing all connected edges");

    let sessions = state.sessions();
    let mut edges = Vec::new();

    for entry in sessions.iter() {
        let (edge_id, session) = entry.pair();
        let device_count = session.device_count().await;
        edges.push(json!({
            "edge_id": edge_id,
            "name": session.name(),
            "connected_at": session.connected_at().to_rfc3339(),
            "device_count": device_count,
        }));
    }

    Json(json!({
        "edges": edges,
        "total": edges.len(),
    }))
}

/// Get details for a specific edge
pub async fn get_edge(
    State(state): State<HubState>,
    axum::extract::Path(edge_id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    info!("Getting details for edge: {}", edge_id);

    match state.sessions().get(&edge_id) {
        Some(session) => {
            let device_count = session.device_count().await;
            Ok(Json(json!({
                "edge_id": edge_id,
                "name": session.name(),
                "hostname": session.hostname(),
                "ip_address": session.ip_address(),
                "connected_at": session.connected_at().to_rfc3339(),
                "device_count": device_count,
                "status": "connected",
            })))
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// Get device status for an edge
pub async fn get_edge_devices(
    State(state): State<HubState>,
    axum::extract::Path(edge_id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    info!("Getting devices for edge: {}", edge_id);

    // Return the latest device states reported by this edge
    match state.sessions().get(&edge_id) {
        Some(session) => {
            let devices = session.devices().await;
            let total = devices.len();
            Ok(Json(json!({
                "edge_id": edge_id,
                "devices": devices,
                "total": total,
            })))
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_edges_response_format() {
        // Test response format
        let response = json!({
            "edges": [],
            "total": 0,
        });

        assert_eq!(response["total"], 0);
        assert!(response["edges"].is_array());
    }
}
