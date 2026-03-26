//! Edge session representation

use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Session for a connected edge node
#[derive(Clone)]
pub struct EdgeSession {
    /// Edge node ID
    edge_id: String,
    /// Edge node name
    name: String,
    /// Hostname
    hostname: Option<String>,
    /// IP address
    ip_address: Option<String>,
    /// Connection timestamp
    connected_at: DateTime<Utc>,
    /// Number of devices being monitored
    device_count: Arc<RwLock<usize>>,
    /// Last heartbeat timestamp
    last_heartbeat: Arc<RwLock<DateTime<Utc>>>,
}

impl EdgeSession {
    /// Create a new edge session
    pub fn new(
        edge_id: String,
        name: String,
        hostname: Option<String>,
        ip_address: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            edge_id,
            name,
            hostname,
            ip_address,
            connected_at: now,
            device_count: Arc::new(RwLock::new(0)),
            last_heartbeat: Arc::new(RwLock::new(now)),
        }
    }

    /// Get the edge ID
    pub fn edge_id(&self) -> &str {
        &self.edge_id
    }

    /// Get the edge name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the hostname
    pub fn hostname(&self) -> Option<&str> {
        self.hostname.as_deref()
    }

    /// Get the IP address
    pub fn ip_address(&self) -> Option<&str> {
        self.ip_address.as_deref()
    }

    /// Get the connection timestamp
    pub fn connected_at(&self) -> DateTime<Utc> {
        self.connected_at
    }

    /// Get the device count
    pub async fn device_count(&self) -> usize {
        *self.device_count.read().await
    }

    /// Set the device count
    pub async fn set_device_count(&self, count: usize) {
        *self.device_count.write().await = count;
    }

    /// Get the last heartbeat timestamp
    pub async fn last_heartbeat(&self) -> DateTime<Utc> {
        *self.last_heartbeat.read().await
    }

    /// Update the last heartbeat timestamp
    pub async fn update_heartbeat(&self) {
        *self.last_heartbeat.write().await = Utc::now();
    }

    /// Check if the session is stale (no heartbeat for a while)
    pub async fn is_stale(&self, timeout_secs: i64) -> bool {
        let last = *self.last_heartbeat.read().await;
        let now = Utc::now();
        let duration = now.signed_duration_since(last);
        duration.num_seconds() > timeout_secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edge_session_creation() {
        let session = EdgeSession::new(
            "edge-1".to_string(),
            "Edge Node 1".to_string(),
            Some("localhost".to_string()),
            Some("127.0.0.1".to_string()),
        );

        assert_eq!(session.edge_id(), "edge-1");
        assert_eq!(session.name(), "Edge Node 1");
        assert_eq!(session.hostname(), Some("localhost"));
        assert_eq!(session.ip_address(), Some("127.0.0.1"));
    }

    #[test]
    fn test_device_count() {
        let session = EdgeSession::new(
            "edge-1".to_string(),
            "Edge Node 1".to_string(),
            None,
            None,
        );

        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                assert_eq!(session.device_count().await, 0);

                session.set_device_count(5).await;
                assert_eq!(session.device_count().await, 5);
            });
    }

    #[test]
    fn test_heartbeat() {
        let session = EdgeSession::new(
            "edge-1".to_string(),
            "Edge Node 1".to_string(),
            None,
            None,
        );

        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                let first = session.last_heartbeat().await;
                session.update_heartbeat().await;
                let second = session.last_heartbeat().await;

                assert!(second > first);
                assert!(!session.is_stale(60).await);
            });
    }

    #[test]
    fn test_is_stale() {
        let session = EdgeSession::new(
            "edge-1".to_string(),
            "Edge Node 1".to_string(),
            None,
            None,
        );

        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                // Not stale immediately
                assert!(!session.is_stale(10).await);

                // Set a very old heartbeat
                *session.last_heartbeat.write().await =
                    Utc::now() - chrono::Duration::seconds(20);

                // Should be stale with 10 second timeout
                assert!(session.is_stale(10).await);
            });
    }
}
