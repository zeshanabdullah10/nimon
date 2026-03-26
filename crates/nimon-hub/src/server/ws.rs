//! WebSocket handler utilities for the hub server

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tracing::{debug, error, info, warn};

use crate::session::SessionStore;

/// Handle a WebSocket connection from an edge node
pub async fn handle_edge_connection(
    mut socket: WebSocket,
    edge_id: String,
    sessions: SessionStore,
) {
    info!("Edge {} connected", edge_id);

    // Split the socket
    let (mut sender, mut receiver) = socket.split();

    // TODO: Send welcome message
    // TODO: Register session

    // Message loop
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                debug!("Received text from edge {}: {}", edge_id, text);

                // TODO: Parse and handle message
                // For now, echo back
                if sender.send(Message::Text(text)).await.is_err() {
                    warn!("Failed to send message to edge {}", edge_id);
                    break;
                }
            }
            Ok(Message::Close(_)) => {
                info!("Edge {} requested close", edge_id);
                break;
            }
            Ok(Message::Ping(data)) => {
                // Respond to ping
                if sender.send(Message::Pong(data)).await.is_err() {
                    warn!("Failed to send pong to edge {}", edge_id);
                    break;
                }
            }
            Err(e) => {
                error!("WebSocket error for edge {}: {}", edge_id, e);
                break;
            }
            _ => {
                debug!("Ignoring non-text message from edge {}", edge_id);
            }
        }
    }

    // TODO: Unregister session
    info!("Edge {} disconnected", edge_id);
}

/// Send a message to a specific edge
pub async fn send_to_edge(
    edge_id: &str,
    message: String,
    sessions: &SessionStore,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // TODO: Implement sending to specific edge session
    debug!("Sending message to edge {}: {}", edge_id, message);
    Ok(())
}

/// Broadcast a message to all connected edges
pub async fn broadcast_to_edges(
    message: String,
    sessions: &SessionStore,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let mut sent_count = 0;

    for entry in sessions.iter() {
        let (edge_id, _session) = entry.pair();
        match send_to_edge(edge_id, message.clone(), sessions).await {
            Ok(_) => sent_count += 1,
            Err(e) => {
                warn!("Failed to send to edge {}: {}", edge_id, e);
            }
        }
    }

    debug!("Broadcast message to {} edges", sent_count);
    Ok(sent_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_broadcast_to_empty_sessions() {
        let sessions = SessionStore::new();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(broadcast_to_edges("test".to_string(), &sessions));

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }
}
