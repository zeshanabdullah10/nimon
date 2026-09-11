//! REST API routes for the hub server

use axum::{extract::State, http::StatusCode, response::Json};
use serde_json::json;
use tracing::info;

use crate::server::HubState;

/// List all known edges: live sessions merged with the database
/// registry (edges that have connected before but are currently
/// offline appear with `status: "offline"`).
pub async fn list_edges(State(state): State<HubState>) -> Json<serde_json::Value> {
    info!("Listing all known edges");

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
            "status": "online",
        }));
    }

    // Merge in edges that exist only in the database (registered
    // previously, currently offline)
    if let Some(pool) = state.db_pool() {
        let repo = nimon_core::db::edge_repo::EdgeRepository::new(&pool);
        match repo.list().await {
            Ok(known) => {
                for edge in known {
                    let live = sessions.get(&edge.id).map(|s| s.name().to_string());
                    if live.is_none() {
                        edges.push(json!({
                            "edge_id": edge.id,
                            "name": edge.name,
                            "last_seen": edge.last_seen.as_ref().map(|t| t.to_rfc3339()),
                            "status": "offline",
                        }));
                    }
                }
            }
            Err(e) => tracing::warn!("Failed to list edges from database: {}", e),
        }
    }

    Json(json!({
        "edges": edges,
        "total": edges.len(),
    }))
}

/// Get details for a specific edge (live session preferred, database
/// record as fallback).
pub async fn get_edge(
    State(state): State<HubState>,
    axum::extract::Path(edge_id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    info!("Getting details for edge: {}", edge_id);

    if let Some(session) = state.sessions().get(&edge_id) {
        let device_count = session.device_count().await;
        return Ok(Json(json!({
            "edge_id": edge_id,
            "name": session.name(),
            "hostname": session.hostname(),
            "ip_address": session.ip_address(),
            "connected_at": session.connected_at().to_rfc3339(),
            "device_count": device_count,
            "status": "online",
        })));
    }

    // Fall back to the database record
    if let Some(pool) = state.db_pool() {
        let repo = nimon_core::db::edge_repo::EdgeRepository::new(&pool);
        match repo.get(&edge_id).await {
            Ok(edge) => {
                return Ok(Json(json!({
                    "edge_id": edge.id,
                    "name": edge.name,
                    "hostname": edge.hostname,
                    "ip_address": edge.ip_address,
                    "last_seen": edge.last_seen.as_ref().map(|t| t.to_rfc3339()),
                    "status": "offline",
                })));
            }
            Err(nimon_core::NimonError::EdgeNotFound(_)) => {}
            Err(e) => {
                tracing::warn!("Failed to load edge {} from database: {}", edge_id, e);
            }
        }
    }

    Err(StatusCode::NOT_FOUND)
}

/// Get device status for an edge
pub async fn get_edge_devices(
    State(state): State<HubState>,
    axum::extract::Path(edge_id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    info!("Getting devices for edge: {}", edge_id);

    // Live snapshot from the session first
    if let Some(session) = state.sessions().get(&edge_id) {
        let devices = session.devices().await;
        let total = devices.len();
        return Ok(Json(json!({
            "edge_id": edge_id,
            "devices": devices,
            "total": total,
        })));
    }

    // Offline edge: return last-known status from the database
    if let Some(pool) = state.db_pool() {
        let repo = nimon_core::db::device_repo::DeviceRepository::new(&pool);
        match repo.list_by_edge(&edge_id).await {
            Ok(devices) if !devices.is_empty() => {
                let mut list = Vec::new();
                for device in devices {
                    let status = repo
                        .get_status(&device.id)
                        .await
                        .ok()
                        .map(|s| {
                            json!({
                                "device_id": device.id,
                                "status": s.status.to_string(),
                                "last_poll": s.last_poll.to_rfc3339(),
                                "metrics": s.metrics,
                            })
                        })
                        .unwrap_or_else(|| json!({ "device_id": device.id, "status": "unknown" }));
                    list.push(status);
                }
                let total = list.len();
                return Ok(Json(json!({
                    "edge_id": edge_id,
                    "devices": list,
                    "total": total,
                    "live": false,
                })));
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("Failed to list devices for edge {}: {}", edge_id, e);
            }
        }
    }

    Err(StatusCode::NOT_FOUND)
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
