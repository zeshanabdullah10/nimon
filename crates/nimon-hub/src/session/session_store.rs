//! Session store for managing connected edge nodes

use dashmap::DashMap;
use tracing::{info, warn};

use crate::session::EdgeSession;

/// Thread-safe session store for edge connections
#[derive(Clone)]
pub struct SessionStore {
    sessions: DashMap<String, EdgeSession>,
}

impl SessionStore {
    /// Create a new session store
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    /// Add a new session. Returns the session it replaced (duplicate
    /// edge_id registration), so the caller can close the old socket.
    pub fn add(&self, session: EdgeSession) -> Option<EdgeSession> {
        let edge_id = session.edge_id().to_string();
        info!("Adding session for edge: {}", edge_id);
        self.sessions.insert(edge_id, session)
    }

    /// Remove a session
    pub fn remove(&self, edge_id: &str) -> Option<EdgeSession> {
        info!("Removing session for edge: {}", edge_id);
        self.sessions.remove(edge_id).map(|(_, session)| session)
    }

    /// Get a session by edge ID
    pub fn get(&self, edge_id: &str) -> Option<EdgeSession> {
        self.sessions.get(edge_id).map(|session| session.clone())
    }

    /// Check if an edge is connected
    pub fn contains(&self, edge_id: &str) -> bool {
        self.sessions.contains_key(edge_id)
    }

    /// Get the number of connected edges
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Check if there are any connected edges
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Get all edge IDs
    pub fn edge_ids(&self) -> Vec<String> {
        self.sessions
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Clear all sessions
    pub fn clear(&self) {
        info!("Clearing all sessions");
        self.sessions.clear();
    }

    /// Remove stale sessions. Returns the edge IDs that were removed so
    /// callers can mark them offline in the database.
    pub async fn remove_stale(&self, timeout_secs: i64) -> Vec<String> {
        let mut stale_edges = Vec::new();

        for entry in self.sessions.iter() {
            let (edge_id, session) = entry.pair();
            if session.is_stale(timeout_secs).await {
                stale_edges.push(edge_id.clone());
            }
        }

        for edge_id in &stale_edges {
            warn!("Removing stale session for edge: {}", edge_id);
            self.remove(edge_id);
        }

        stale_edges
    }

    /// Iterate over all sessions
    pub fn iter(&self) -> dashmap::iter::Iter<'_, String, EdgeSession> {
        self.sessions.iter()
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_store_creation() {
        let store = SessionStore::new();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn test_add_and_remove_session() {
        let store = SessionStore::new();

        let session = EdgeSession::new("edge-1".to_string(), "Edge Node 1".to_string(), None, None);

        store.add(session);
        assert_eq!(store.len(), 1);
        assert!(store.contains("edge-1"));

        let removed = store.remove("edge-1");
        assert!(removed.is_some());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_get_session() {
        let store = SessionStore::new();

        let session = EdgeSession::new(
            "edge-1".to_string(),
            "Edge Node 1".to_string(),
            Some("localhost".to_string()),
            None,
        );

        store.add(session.clone());

        let retrieved = store.get("edge-1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name(), "Edge Node 1");
    }

    #[test]
    fn test_edge_ids() {
        let store = SessionStore::new();

        store.add(EdgeSession::new(
            "edge-1".to_string(),
            "Edge 1".to_string(),
            None,
            None,
        ));
        store.add(EdgeSession::new(
            "edge-2".to_string(),
            "Edge 2".to_string(),
            None,
            None,
        ));

        let ids = store.edge_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"edge-1".to_string()));
        assert!(ids.contains(&"edge-2".to_string()));
    }

    #[test]
    fn test_clear_sessions() {
        let store = SessionStore::new();

        store.add(EdgeSession::new(
            "edge-1".to_string(),
            "Edge 1".to_string(),
            None,
            None,
        ));
        store.add(EdgeSession::new(
            "edge-2".to_string(),
            "Edge 2".to_string(),
            None,
            None,
        ));

        assert_eq!(store.len(), 2);
        store.clear();
        assert_eq!(store.len(), 0);
    }
}
