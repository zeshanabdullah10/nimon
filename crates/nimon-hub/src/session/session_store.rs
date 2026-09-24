//! Session store for managing connected edge nodes

use dashmap::DashMap;
use tracing::{info, warn};

use crate::session::EdgeSession;

/// Thread-safe session store for edge connections.
///
/// Never hold a map guard across `.await`: use [`SessionStore::snapshot`]
/// to iterate with async work.
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

    /// Remove a session unconditionally
    pub fn remove(&self, edge_id: &str) -> Option<EdgeSession> {
        info!("Removing session for edge: {}", edge_id);
        self.sessions.remove(edge_id).map(|(_, session)| session)
    }

    /// Remove the session for `edge_id` only if it still belongs to the
    /// connection `conn_id` (a replaced connection must not unregister its
    /// successor).
    pub fn remove_if_current(&self, edge_id: &str, conn_id: &str) -> Option<EdgeSession> {
        let removed = self
            .sessions
            .remove_if(edge_id, |_, session| session.conn_id() == conn_id)
            .map(|(_, session)| session);
        if removed.is_some() {
            info!("Removing session for edge: {}", edge_id);
        }
        removed
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

    /// Clone every session (sorted by edge id) so callers can await
    /// without holding map guards.
    pub fn snapshot(&self) -> Vec<EdgeSession> {
        let mut list: Vec<EdgeSession> = self
            .sessions
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        list.sort_by(|a, b| a.edge_id().cmp(b.edge_id()));
        list
    }

    /// Clear all sessions (their sockets are asked to close)
    pub fn clear(&self) {
        info!("Clearing all sessions");
        for session in self.snapshot() {
            session.request_shutdown();
        }
        self.sessions.clear();
    }

    /// Remove stale sessions and close their sockets. Returns the edge
    /// IDs that were removed so callers can mark them offline.
    pub async fn remove_stale(&self, timeout_secs: i64) -> Vec<String> {
        let mut stale_edges = Vec::new();
        for session in self.snapshot() {
            if session.is_stale(timeout_secs).await
                && self
                    .remove_if_current(session.edge_id(), session.conn_id())
                    .is_some()
            {
                warn!(
                    "Edge {} silent for more than {}s; closing its connection",
                    session.edge_id(),
                    timeout_secs
                );
                session.request_shutdown();
                stale_edges.push(session.edge_id().to_string());
            }
        }
        stale_edges
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
    fn test_remove_by_identity_keeps_replacement() {
        let store = SessionStore::new();
        let old = EdgeSession::new("edge-1".to_string(), "Old".to_string(), None, None);
        let new = EdgeSession::new("edge-1".to_string(), "New".to_string(), None, None);

        store.add(old.clone());
        let replaced = store.add(new.clone()).expect("old session replaced");
        assert!(replaced.same_connection(&old));
        replaced.request_shutdown();

        // The old connection exits: it must NOT remove the new session
        assert!(store.remove_if_current("edge-1", old.conn_id()).is_none());
        assert_eq!(store.get("edge-1").unwrap().name(), "New");

        // The new connection exits: removed
        assert!(store.remove_if_current("edge-1", new.conn_id()).is_some());
        assert!(store.is_empty());
    }

    #[test]
    fn test_remove_stale_cancels_connection() {
        let store = SessionStore::new();
        let session = EdgeSession::new("edge-1".to_string(), "Edge 1".to_string(), None, None);
        store.add(session.clone());
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            // Fresh: nothing removed
            assert!(store.remove_stale(60).await.is_empty());
            // Negative timeout: everything is stale
            let removed = store.remove_stale(-1).await;
            assert_eq!(removed, vec!["edge-1".to_string()]);
        });
        assert!(store.is_empty());
        assert!(session.is_shutdown_requested());
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
    fn test_edge_ids_and_snapshot() {
        let store = SessionStore::new();

        store.add(EdgeSession::new(
            "edge-2".to_string(),
            "Edge 2".to_string(),
            None,
            None,
        ));
        store.add(EdgeSession::new(
            "edge-1".to_string(),
            "Edge 1".to_string(),
            None,
            None,
        ));

        let ids = store.edge_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"edge-1".to_string()));
        assert!(ids.contains(&"edge-2".to_string()));
        let snapshot = store.snapshot();
        assert_eq!(snapshot[0].edge_id(), "edge-1");
        assert_eq!(snapshot[1].edge_id(), "edge-2");
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
