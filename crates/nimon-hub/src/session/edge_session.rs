//! Edge session representation

use chrono::{DateTime, Utc};
use nimon_core::{HealthStatus, MetricValue};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Notify, RwLock};

use nimon_core::protocol::WsMessage;

/// Latest observed state of a device reported by an edge node
#[derive(Debug, Clone, Serialize)]
pub struct DeviceSnapshot {
    pub device_id: String,
    pub status: String,
    pub metrics: HashMap<String, MetricValue>,
    pub last_seen: DateTime<Utc>,
    #[serde(default)]
    pub is_simulated: bool,
}

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
    /// Latest device states reported by this edge
    devices: Arc<RwLock<HashMap<String, DeviceSnapshot>>>,
    /// Last heartbeat timestamp
    last_heartbeat: Arc<RwLock<DateTime<Utc>>>,
    /// Device count and status reported in the last heartbeat payload
    heartbeat_info: Arc<RwLock<HeartbeatInfo>>,
    /// Outbound message channel consumed by the socket write loop
    outbound: mpsc::UnboundedSender<WsMessage>,
    /// Signals the owning socket loop to close (duplicate registration)
    shutdown: Arc<Notify>,
}

/// Information carried by the most recent heartbeat
#[derive(Debug, Clone, Default)]
pub struct HeartbeatInfo {
    pub device_count: Option<usize>,
    pub status: Option<String>,
}

impl EdgeSession {
    /// Create a new edge session
    pub fn new(
        edge_id: String,
        name: String,
        hostname: Option<String>,
        ip_address: Option<String>,
    ) -> Self {
        Self::with_outbound(
            edge_id,
            name,
            hostname,
            ip_address,
            mpsc::unbounded_channel().0,
        )
    }

    /// Create a new edge session wired to a socket write loop
    pub fn with_outbound(
        edge_id: String,
        name: String,
        hostname: Option<String>,
        ip_address: Option<String>,
        outbound: mpsc::UnboundedSender<WsMessage>,
    ) -> Self {
        let now = Utc::now();
        Self {
            edge_id,
            name,
            hostname,
            ip_address,
            connected_at: now,
            devices: Arc::new(RwLock::new(HashMap::new())),
            last_heartbeat: Arc::new(RwLock::new(now)),
            heartbeat_info: Arc::new(RwLock::new(HeartbeatInfo::default())),
            outbound,
            shutdown: Arc::new(Notify::new()),
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
        self.devices.read().await.len()
    }

    /// Record the latest status for a device reported by this edge
    pub async fn update_device(
        &self,
        device_id: String,
        status: HealthStatus,
        metrics: HashMap<String, MetricValue>,
        is_simulated: bool,
    ) {
        let snapshot = DeviceSnapshot {
            device_id: device_id.clone(),
            status: status.to_string(),
            metrics,
            last_seen: Utc::now(),
            is_simulated,
        };
        self.devices.write().await.insert(device_id, snapshot);
    }

    /// Get snapshots of all devices, sorted by device ID
    pub async fn devices(&self) -> Vec<DeviceSnapshot> {
        let map = self.devices.read().await;
        let mut list: Vec<DeviceSnapshot> = map.values().cloned().collect();
        list.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        list
    }

    /// Get the last heartbeat timestamp
    pub async fn last_heartbeat(&self) -> DateTime<Utc> {
        *self.last_heartbeat.read().await
    }

    /// Update the last heartbeat timestamp and payload
    pub async fn update_heartbeat(&self, info: HeartbeatInfo) {
        *self.last_heartbeat.write().await = Utc::now();
        *self.heartbeat_info.write().await = info;
    }

    /// Device count from the last heartbeat payload (None when never sent)
    pub async fn heartbeat_device_count(&self) -> Option<usize> {
        self.heartbeat_info.read().await.device_count
    }

    /// Connector status from the last heartbeat payload
    pub async fn heartbeat_status(&self) -> Option<String> {
        self.heartbeat_info.read().await.status.clone()
    }

    /// Check if the session is stale (no heartbeat for a while)
    pub async fn is_stale(&self, timeout_secs: i64) -> bool {
        let last = *self.last_heartbeat.read().await;
        let now = Utc::now();
        let duration = now.signed_duration_since(last);
        duration.num_seconds() > timeout_secs
    }

    /// Enqueue a message for delivery to this edge. Fails when the socket
    /// is gone (receiver dropped).
    pub fn send(&self, msg: WsMessage) -> Result<(), nimon_core::NimonError> {
        self.outbound
            .send(msg)
            .map_err(|e| nimon_core::NimonError::Connection(e.to_string()))
    }

    /// Subscribe to the shutdown signal for the owning socket loop
    pub fn shutdown_notify(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }

    /// Ask the owning socket loop to close (used on duplicate registration)
    pub fn request_shutdown(&self) {
        self.shutdown.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> EdgeSession {
        EdgeSession::new(
            "edge-1".to_string(),
            "Edge Node 1".to_string(),
            Some("localhost".to_string()),
            Some("127.0.0.1".to_string()),
        )
    }

    #[test]
    fn test_edge_session_creation() {
        let session = session();
        assert_eq!(session.edge_id(), "edge-1");
        assert_eq!(session.name(), "Edge Node 1");
        assert_eq!(session.hostname(), Some("localhost"));
        assert_eq!(session.ip_address(), Some("127.0.0.1"));
    }

    #[test]
    fn test_device_count() {
        let session = EdgeSession::new("edge-1".to_string(), "Edge Node 1".to_string(), None, None);

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            assert_eq!(session.device_count().await, 0);

            let mut metrics = HashMap::new();
            metrics.insert("temperature".to_string(), MetricValue::Float(42.0));
            session
                .update_device("dev-1".to_string(), HealthStatus::Healthy, metrics, false)
                .await;
            session
                .update_device(
                    "dev-2".to_string(),
                    HealthStatus::Warning,
                    HashMap::new(),
                    true,
                )
                .await;
            assert_eq!(session.device_count().await, 2);

            // Re-reporting the same device must not grow the registry
            session
                .update_device(
                    "dev-1".to_string(),
                    HealthStatus::Healthy,
                    HashMap::new(),
                    false,
                )
                .await;
            assert_eq!(session.device_count().await, 2);

            let devices = session.devices().await;
            assert_eq!(devices[0].device_id, "dev-1");
            assert_eq!(devices[0].status, "healthy");
            assert!(!devices[0].is_simulated);
            assert!(devices[1].is_simulated);
        });
    }

    #[test]
    fn test_heartbeat() {
        let session = EdgeSession::new("edge-1".to_string(), "Edge Node 1".to_string(), None, None);

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            assert!(session.heartbeat_device_count().await.is_none());
            session
                .update_heartbeat(HeartbeatInfo {
                    device_count: Some(3),
                    status: Some("Connected".to_string()),
                })
                .await;
            assert_eq!(session.heartbeat_device_count().await, Some(3));
            assert_eq!(
                session.heartbeat_status().await.as_deref(),
                Some("Connected")
            );
            assert!(!session.is_stale(60).await);
        });
    }

    #[test]
    fn test_is_stale() {
        let session = EdgeSession::new("edge-1".to_string(), "Edge Node 1".to_string(), None, None);

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            // Not stale immediately
            assert!(!session.is_stale(10).await);

            // Set a very old heartbeat
            *session.last_heartbeat.write().await = Utc::now() - chrono::Duration::seconds(20);

            // Should be stale with 10 second timeout
            assert!(session.is_stale(10).await);
        });
    }

    #[test]
    fn test_outbound_send() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let session =
            EdgeSession::with_outbound("edge-1".to_string(), "Edge 1".to_string(), None, None, tx);

        assert!(session.send(WsMessage::ping()).is_ok());
        assert!(rx.try_recv().is_ok());

        drop(rx);
        assert!(session.send(WsMessage::ping()).is_err());
    }
}
