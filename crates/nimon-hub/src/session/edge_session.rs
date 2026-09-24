//! Edge session representation

use chrono::{DateTime, Utc};
use nimon_core::{HealthStatus, MetricValue};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use nimon_core::protocol::WsMessage;

/// Capacity of the per-connection outbound queue. A consumer that falls
/// this far behind is disconnected.
pub const OUTBOUND_CAPACITY: usize = 256;

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

/// Session for a connected edge node.
///
/// Clones share state. Each WebSocket connection gets a unique
/// `conn_id`, so a stale connection can never unregister the session of
/// a newer connection for the same edge.
#[derive(Clone)]
pub struct EdgeSession {
    /// Edge node ID
    edge_id: String,
    /// Unique id of the WebSocket connection owning this session
    conn_id: String,
    /// Edge node name
    name: String,
    /// Hostname
    hostname: Option<String>,
    /// IP address
    ip_address: Option<String>,
    /// Protocol version the edge spoke when registering
    protocol_version: String,
    /// Connection timestamp
    connected_at: DateTime<Utc>,
    /// Latest device states reported by this edge
    devices: Arc<RwLock<HashMap<String, DeviceSnapshot>>>,
    /// Last time anything (heartbeat or other message) arrived
    last_heartbeat: Arc<RwLock<DateTime<Utc>>>,
    /// Device count and status reported in the last heartbeat payload
    heartbeat_info: Arc<RwLock<HeartbeatInfo>>,
    /// Outbound message queue consumed by the socket write loop
    outbound: mpsc::Sender<WsMessage>,
    /// Cancels the owning socket loop (replacement, staleness, slow consumer)
    shutdown: CancellationToken,
}

/// Information carried by the most recent heartbeat
#[derive(Debug, Clone, Default)]
pub struct HeartbeatInfo {
    pub device_count: Option<usize>,
    pub status: Option<String>,
    pub uptime_secs: Option<u64>,
    pub version: Option<String>,
}

impl EdgeSession {
    /// Create a new edge session not attached to a socket (tests)
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
            mpsc::channel(OUTBOUND_CAPACITY).0,
        )
    }

    /// Create a new edge session wired to a socket write loop
    pub fn with_outbound(
        edge_id: String,
        name: String,
        hostname: Option<String>,
        ip_address: Option<String>,
        outbound: mpsc::Sender<WsMessage>,
    ) -> Self {
        Self::for_connection(
            edge_id,
            name,
            hostname,
            ip_address,
            nimon_core::protocol::PROTOCOL_VERSION.to_string(),
            outbound,
            CancellationToken::new(),
        )
    }

    /// Create the session of a specific connection
    pub fn for_connection(
        edge_id: String,
        name: String,
        hostname: Option<String>,
        ip_address: Option<String>,
        protocol_version: String,
        outbound: mpsc::Sender<WsMessage>,
        shutdown: CancellationToken,
    ) -> Self {
        let now = Utc::now();
        Self {
            edge_id,
            conn_id: ulid::Ulid::new().to_string(),
            name,
            hostname,
            ip_address,
            protocol_version,
            connected_at: now,
            devices: Arc::new(RwLock::new(HashMap::new())),
            last_heartbeat: Arc::new(RwLock::new(now)),
            heartbeat_info: Arc::new(RwLock::new(HeartbeatInfo::default())),
            outbound,
            shutdown,
        }
    }

    /// Get the edge ID
    pub fn edge_id(&self) -> &str {
        &self.edge_id
    }

    /// Unique id of the owning connection
    pub fn conn_id(&self) -> &str {
        &self.conn_id
    }

    /// True when both handles belong to the same connection
    pub fn same_connection(&self, other: &EdgeSession) -> bool {
        self.conn_id == other.conn_id
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

    /// Protocol version announced at registration
    pub fn protocol_version(&self) -> &str {
        &self.protocol_version
    }

    /// True for peers speaking protocol 1.0 (need full threshold pushes)
    pub fn is_legacy_protocol(&self) -> bool {
        is_legacy_protocol(&self.protocol_version)
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

    /// Forget a device (removed from discovery). Returns true when known.
    pub async fn remove_device(&self, device_id: &str) -> bool {
        self.devices.write().await.remove(device_id).is_some()
    }

    /// Get snapshots of all devices, sorted by device ID
    pub async fn devices(&self) -> Vec<DeviceSnapshot> {
        let map = self.devices.read().await;
        let mut list: Vec<DeviceSnapshot> = map.values().cloned().collect();
        list.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        list
    }

    /// Get one device snapshot
    pub async fn device(&self, device_id: &str) -> Option<DeviceSnapshot> {
        self.devices.read().await.get(device_id).cloned()
    }

    /// Last time the edge was heard from (heartbeat or any message)
    pub async fn last_heartbeat(&self) -> DateTime<Utc> {
        *self.last_heartbeat.read().await
    }

    /// Non-blocking read of [`Self::last_heartbeat`] (None while a writer holds the lock)
    pub fn try_last_heartbeat(&self) -> Option<DateTime<Utc>> {
        self.last_heartbeat.try_read().ok().map(|t| *t)
    }

    /// Record that a message arrived from the edge
    pub async fn touch(&self) {
        *self.last_heartbeat.write().await = Utc::now();
    }

    /// Update the last heartbeat timestamp and payload
    pub async fn update_heartbeat(&self, info: HeartbeatInfo) {
        *self.last_heartbeat.write().await = Utc::now();
        *self.heartbeat_info.write().await = info;
    }

    /// Payload of the last heartbeat
    pub async fn heartbeat_info(&self) -> HeartbeatInfo {
        self.heartbeat_info.read().await.clone()
    }

    /// Device count from the last heartbeat payload (None when never sent)
    pub async fn heartbeat_device_count(&self) -> Option<usize> {
        self.heartbeat_info.read().await.device_count
    }

    /// Connector status from the last heartbeat payload
    pub async fn heartbeat_status(&self) -> Option<String> {
        self.heartbeat_info.read().await.status.clone()
    }

    /// Check if the session is stale (nothing heard for a while)
    pub async fn is_stale(&self, timeout_secs: i64) -> bool {
        let last = *self.last_heartbeat.read().await;
        Utc::now().signed_duration_since(last).num_seconds() > timeout_secs
    }

    /// Enqueue a message for delivery to this edge. Fails when the socket
    /// is gone; a full queue (slow consumer) disconnects the edge.
    pub fn send(&self, msg: WsMessage) -> Result<(), nimon_core::NimonError> {
        match self.outbound.try_send(msg) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!(
                    "Outbound queue for edge {} is full ({} messages); disconnecting slow consumer",
                    self.edge_id, OUTBOUND_CAPACITY
                );
                self.shutdown.cancel();
                Err(nimon_core::NimonError::Connection(
                    "outbound queue full; edge disconnected".to_string(),
                ))
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(nimon_core::NimonError::Connection(
                "edge connection closed".to_string(),
            )),
        }
    }

    /// Token cancelled when the owning socket loop must close
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Ask the owning socket loop to close. Cannot be lost: the token
    /// stays cancelled even if the loop is not currently waiting on it.
    pub fn request_shutdown(&self) {
        self.shutdown.cancel();
    }

    /// True once shutdown was requested
    pub fn is_shutdown_requested(&self) -> bool {
        self.shutdown.is_cancelled()
    }
}

/// True for protocol "1.0" (or "1"): such peers require full thresholds.
pub fn is_legacy_protocol(version: &str) -> bool {
    let mut parts = version.trim().split('.');
    let major = parts.next().and_then(|m| m.parse::<u64>().ok());
    let minor = parts
        .next()
        .and_then(|m| m.parse::<u64>().ok())
        .unwrap_or(0);
    major == Some(1) && minor == 0
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
        assert!(!session.is_legacy_protocol());
        // Each session has its own connection identity
        assert!(!session.same_connection(&self::session()));
        assert!(session.same_connection(&session.clone()));
    }

    #[test]
    fn test_legacy_protocol_detection() {
        assert!(is_legacy_protocol("1.0"));
        assert!(is_legacy_protocol("1"));
        assert!(!is_legacy_protocol("1.1"));
        assert!(!is_legacy_protocol("1.7"));
        assert!(!is_legacy_protocol("2.0"));
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

            assert!(session.remove_device("dev-2").await);
            assert!(!session.remove_device("dev-2").await);
            assert_eq!(session.device_count().await, 1);
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
                    uptime_secs: Some(10),
                    version: Some("0.2.0".to_string()),
                })
                .await;
            assert_eq!(session.heartbeat_device_count().await, Some(3));
            assert_eq!(
                session.heartbeat_status().await.as_deref(),
                Some("Connected")
            );
            assert_eq!(session.heartbeat_info().await.uptime_secs, Some(10));
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

            // Any message refreshes liveness
            session.touch().await;
            assert!(!session.is_stale(10).await);
        });
    }

    #[test]
    fn test_outbound_send() {
        let (tx, mut rx) = mpsc::channel(4);
        let session =
            EdgeSession::with_outbound("edge-1".to_string(), "Edge 1".to_string(), None, None, tx);

        assert!(session.send(WsMessage::ping()).is_ok());
        assert!(rx.try_recv().is_ok());

        drop(rx);
        assert!(session.send(WsMessage::ping()).is_err());
    }

    #[test]
    fn test_slow_consumer_is_disconnected() {
        let (tx, _rx) = mpsc::channel(2);
        let session =
            EdgeSession::with_outbound("edge-1".to_string(), "Edge 1".to_string(), None, None, tx);
        assert!(session.send(WsMessage::ping()).is_ok());
        assert!(session.send(WsMessage::ping()).is_ok());
        assert!(!session.is_shutdown_requested());
        assert!(session.send(WsMessage::ping()).is_err());
        assert!(session.is_shutdown_requested());
    }

    #[test]
    fn test_shutdown_request_is_not_lost() {
        // Requested before anyone waits: the waiter still observes it
        let session = session();
        session.request_shutdown();
        let token = session.shutdown_token();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                tokio::time::timeout(std::time::Duration::from_secs(1), token.cancelled())
                    .await
                    .expect("shutdown must be observed");
            });
    }
}
