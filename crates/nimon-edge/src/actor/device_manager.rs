//! Device manager actor

use actix::prelude::*;
use std::collections::HashMap;
use std::time::Duration;

use nimon_core::{
    actor::{DevicePoll, DeviceStatusUpdate},
    Device, DeviceType,
};

use super::device_actor::{DeviceActor, HealthUpdate, StopDevice};
use super::hub_connector::{HubConnectorActor, UpdateDeviceCount};
use super::prediction_actor::PredictionActor;

/// Attempt to classify a device product name into a DeviceType
fn classify_device(product_name: &str) -> DeviceType {
    let name = product_name.to_uppercase();
    // Check more specific patterns first to avoid false matches
    if name.contains("CDAQ") || name.contains("COMPACTDAQ") {
        DeviceType::CDaq
    } else if name.contains("XNET") || name.contains("NI-XNET") {
        DeviceType::Xnet
    } else if name.contains("GPIB") {
        DeviceType::Gpib
    } else if name.contains("VISA") {
        DeviceType::Visa
    } else if name.contains("PS") || name.contains("POWER") {
        DeviceType::PowerSupply
    } else if name.contains("PXI") || name.contains("PXIE") {
        DeviceType::Pxi
    } else if name.contains("DAQ") || name.contains("USB-") {
        DeviceType::Daq
    } else {
        DeviceType::Daq
    }
}

/// Actor that manages all device actors
pub struct DeviceManagerActor {
    /// Map of device ID to device actor address
    device_actors: HashMap<String, Addr<DeviceActor>>,
    /// Device IDs in NI-SysCfg enumeration order (for sweep matching)
    device_order: Vec<String>,
    /// Product names in the same order (topology change detection)
    product_order: Vec<String>,
    /// True when devices come from real NI-SysCfg discovery and are
    /// driven by the shared sweep instead of per-actor polling
    sweep_mode: bool,
    /// Prediction actor address
    prediction_actor: Option<Addr<PredictionActor>>,
    /// Hub connector actor address for forwarding messages
    hub_connector: Option<Addr<HubConnectorActor>>,
    /// Edge ID
    edge_id: String,
    /// Default poll interval
    default_poll_interval: u64,
    /// Notified whenever the managed device count changes
    count_subscriber: Option<Recipient<UpdateDeviceCount>>,
}

impl DeviceManagerActor {
    pub fn new(edge_id: String) -> Self {
        Self {
            device_actors: HashMap::new(),
            device_order: Vec::new(),
            product_order: Vec::new(),
            sweep_mode: false,
            prediction_actor: None,
            hub_connector: None,
            edge_id,
            default_poll_interval: 10,
            count_subscriber: None,
        }
    }

    /// Set default poll interval
    pub fn with_poll_interval(mut self, interval_secs: u64) -> Self {
        self.default_poll_interval = interval_secs;
        self
    }

    /// Subscribe an actor to device count changes (heartbeats)
    pub fn with_count_subscriber(mut self, recipient: Recipient<UpdateDeviceCount>) -> Self {
        self.count_subscriber = Some(recipient);
        self
    }

    /// Notify the subscriber of the current device count
    fn notify_count(&self) {
        if let Some(ref subscriber) = self.count_subscriber {
            subscriber.do_send(UpdateDeviceCount {
                count: self.device_actors.len(),
            });
        }
    }

    /// Set the hub connector for forwarding predictions and status updates
    pub fn with_hub_connector(mut self, addr: Addr<HubConnectorActor>) -> Self {
        self.hub_connector = Some(addr);
        self
    }

    /// Start the prediction actor
    fn start_prediction_actor(&mut self, _ctx: &mut Context<Self>) {
        let mut actor = PredictionActor::new(10).with_thresholds(65.0, 75.0);
        if let Some(ref hub) = self.hub_connector {
            actor = actor.with_hub_connector(hub.clone());
        }
        self.prediction_actor = Some(actor.start());
        tracing::info!("Prediction actor started");
    }

    /// Add a device to be monitored; returns the actor address
    fn add_device(&mut self, device: Device) -> Addr<DeviceActor> {
        let device_id = device.id.clone();
        // topology fingerprint uses the product name (stable across
        // NI MAX renames); device_name may now be the user alias
        let product = device
            .model
            .clone()
            .unwrap_or_else(|| device.device_name.clone());

        let mut actor = DeviceActor::new(device, self.default_poll_interval);
        // Route status updates through the prediction actor, which forwards
        // them (plus any predictions) to the hub connector
        if let Some(ref prediction) = self.prediction_actor {
            actor =
                actor.with_status_recipient(prediction.clone().recipient::<DeviceStatusUpdate>());
        }
        // Real SysCfg devices are driven by the shared sweep
        if self.sweep_mode {
            actor = actor.with_managed_polling();
        }
        let addr = actor.start();

        self.device_actors.insert(device_id.clone(), addr.clone());
        self.device_order.push(device_id.clone());
        self.product_order.push(product);
        tracing::info!("Added device: {}", device_id);
        self.notify_count();
        addr
    }

    /// Remove a device from monitoring
    fn remove_device(&mut self, device_id: &str) {
        if let Some(pos) = self.device_order.iter().position(|id| id == device_id) {
            self.device_order.remove(pos);
            self.product_order.remove(pos);
        }
        if let Some(addr) = self.device_actors.remove(device_id) {
            addr.do_send(StopDevice);
            tracing::info!("Removed device: {}", device_id);
            self.notify_count();
        }
    }

    /// Stop all device actors and forget the current topology
    fn clear_devices(&mut self) {
        for addr in self.device_actors.values() {
            addr.do_send(StopDevice);
        }
        self.device_actors.clear();
        self.device_order.clear();
        self.product_order.clear();
        self.notify_count();
    }

    /// Build a Device from a discovery result, keeping IDs stable:
    /// prefer the NI MAX name (DAQmx alias), then serial, then product#index
    fn device_from_discovered(&self, i: usize, d: &nimon_ni::syscfg::DiscoveredDevice) -> Device {
        let alias = d.alias.clone().filter(|a| !a.is_empty());
        let id_key = alias
            .clone()
            .or_else(|| {
                if d.serial_number.is_empty() {
                    None
                } else {
                    Some(d.serial_number.clone())
                }
            })
            .unwrap_or_else(|| format!("{}#{}", d.product_name, i + 1));
        Device {
            id: format!("{}:{}", self.edge_id, id_key),
            edge_id: self.edge_id.clone(),
            device_name: alias.unwrap_or_else(|| d.product_name.clone()),
            device_type: classify_device(&d.product_name),
            model: Some(d.product_name.clone()),
            serial_number: Some(id_key),
            firmware_version: d.firmware_version.clone(),
            driver_version: d.driver_version.clone(),
            ip_address: d.ip_address.clone(),
            slot: d.slot,
            chassis: d.parent_link.clone(),
            is_simulated: false,
        }
    }

    /// Discover devices using NI-SysCfg, falling back to simulated data
    fn discover_devices(&mut self) -> Vec<Device> {
        // Try real NI-SysCfg discovery first
        match nimon_ni::syscfg::NiSysCfg::load() {
            Ok(api) => match api.create_session() {
                Ok(session) => match session.discover_devices() {
                    Ok(discovered) => {
                        if !discovered.is_empty() {
                            tracing::info!(
                                "Discovered {} real NI device(s) via NI-SysCfg",
                                discovered.len()
                            );
                            self.sweep_mode = true;
                            return discovered
                                .iter()
                                .enumerate()
                                .map(|(i, d)| self.device_from_discovered(i, d))
                                .collect();
                        }
                        tracing::info!("NI-SysCfg returned no devices, using simulated fallback");
                    }
                    Err(e) => {
                        tracing::warn!(
                            "NI-SysCfg discovery failed: {}, using simulated fallback",
                            e
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        "NI-SysCfg session creation failed: {}, using simulated fallback",
                        e
                    );
                }
            },
            Err(_) => {
                tracing::info!("NI-SysCfg not available, using simulated devices");
            }
        }

        // Simulated fallback for development/testing
        self.simulated_devices()
    }

    /// Generate simulated devices for development/testing
    fn simulated_devices(&self) -> Vec<Device> {
        vec![
            Device {
                id: format!("{}:daq-1", self.edge_id),
                edge_id: self.edge_id.clone(),
                device_name: "DAQ-1".to_string(),
                device_type: DeviceType::Daq,
                model: Some("USB-6343".to_string()),
                serial_number: Some("12345678".to_string()),
                firmware_version: None,
                driver_version: None,
                ip_address: None,
                slot: None,
                chassis: None,
                is_simulated: true,
            },
            Device {
                id: format!("{}:pxi-1", self.edge_id),
                edge_id: self.edge_id.clone(),
                device_name: "PXI-1".to_string(),
                device_type: DeviceType::Pxi,
                model: Some("PXIe-8880".to_string()),
                serial_number: Some("87654321".to_string()),
                firmware_version: None,
                driver_version: None,
                ip_address: Some("192.168.1.100".to_string()),
                slot: Some(1),
                chassis: Some("PXI1".to_string()),
                is_simulated: true,
            },
        ]
    }

    /// Get count of managed devices
    pub fn device_count(&self) -> usize {
        self.device_actors.len()
    }

    /// Shared NI-SysCfg sweep: ONE enumeration produces health for ALL
    /// devices each cycle (instead of one full enumeration per device).
    fn start_sweep(&self, ctx: &mut Context<Self>) {
        let interval = Duration::from_secs(self.default_poll_interval.max(1));
        ctx.run_interval(interval, |act, ctx| act.spawn_sweep(ctx));
    }

    /// Run the blocking FFI sweep off the actor thread
    fn spawn_sweep(&self, ctx: &mut Context<Self>) {
        let fut = actix_rt::task::spawn_blocking(|| {
            let api = nimon_ni::syscfg::NiSysCfg::load()?;
            let session = api.create_session()?;
            let sweep = session.discover_with_health();
            // system info rides along: plain session reads, no enumeration
            let system = session.get_system_info();
            sweep.map(|items| (items, system))
        });
        ctx.spawn(fut.into_actor(self).map(|res, _act, _ctx| match res {
            Ok(Ok((items, system))) => _act.apply_sweep(items, system),
            Ok(Err(e)) => tracing::debug!("NI-SysCfg sweep failed: {e}"),
            Err(e) => tracing::debug!("sweep task failed: {e}"),
        }));
    }

    /// Distribute sweep results; resync actors if the topology changed.
    /// Station-level system metrics attach to the host resource (the
    /// device whose alias is the machine hostname).
    fn apply_sweep(
        &mut self,
        mut items: Vec<(
            nimon_ni::syscfg::DiscoveredDevice,
            nimon_ni::syscfg::DeviceHealth,
        )>,
        system: nimon_ni::syscfg::SystemInfo,
    ) {
        if let Some(ref hostname) = system.hostname {
            for (d, health) in items.iter_mut() {
                if d.alias.as_deref() == Some(hostname.as_str()) {
                    if let Some(v) = system.memory_total_mb {
                        health.metrics.insert(
                            "mem_total_mb".to_string(),
                            nimon_core::MetricValue::Float(v),
                        );
                    }
                    if let Some(v) = system.memory_free_mb {
                        health
                            .metrics
                            .insert("mem_free_mb".to_string(), nimon_core::MetricValue::Float(v));
                    }
                    if let Some(v) = system.disk_total_mb {
                        health.metrics.insert(
                            "disk_total_mb".to_string(),
                            nimon_core::MetricValue::Float(v),
                        );
                    }
                    if let Some(v) = system.disk_free_mb {
                        health.metrics.insert(
                            "disk_free_mb".to_string(),
                            nimon_core::MetricValue::Float(v),
                        );
                    }
                    break;
                }
            }
        }

        let topology_changed = items.len() != self.product_order.len()
            || items
                .iter()
                .zip(self.product_order.iter())
                .any(|((d, _), known)| &d.product_name != known);

        if topology_changed {
            tracing::info!(
                "NI topology changed ({} devices), resyncing actors",
                items.len()
            );
            self.clear_devices();
            for (i, (d, health)) in items.into_iter().enumerate() {
                let device = self.device_from_discovered(i, &d);
                let addr = self.add_device(device);
                addr.do_send(HealthUpdate { health });
            }
            return;
        }

        for (i, (_, health)) in items.into_iter().enumerate() {
            if let Some(addr) = self.device_actors.get(&self.device_order[i]) {
                addr.do_send(HealthUpdate { health });
            }
        }
    }
}

impl Actor for DeviceManagerActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        tracing::info!("DeviceManagerActor started for edge {}", self.edge_id);

        // Start prediction actor
        self.start_prediction_actor(ctx);

        // Discover and add devices
        let devices = self.discover_devices();
        for device in devices {
            self.add_device(device);
        }

        // Real hardware: drive health via a single shared sweep
        if self.sweep_mode {
            tracing::info!(
                "Sweep mode: one shared NI-SysCfg enumeration every {}s",
                self.default_poll_interval
            );
            self.start_sweep(ctx);
        }
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        tracing::info!("DeviceManagerActor stopped");
    }
}

/// Message to get list of devices
#[derive(Message)]
#[rtype(result = "Vec<String>")]
pub struct ListDevices;

impl Handler<ListDevices> for DeviceManagerActor {
    type Result = Vec<String>;

    fn handle(&mut self, _msg: ListDevices, _ctx: &mut Self::Context) -> Self::Result {
        self.device_actors.keys().cloned().collect()
    }
}

/// Message to poll all devices
#[derive(Message)]
#[rtype(result = "()")]
pub struct PollAllDevices;

impl Handler<PollAllDevices> for DeviceManagerActor {
    type Result = ();

    fn handle(&mut self, _msg: PollAllDevices, ctx: &mut Self::Context) -> Self::Result {
        // NOTE: `addr.send(...)` inside a sync handler enqueues nothing —
        // the response future is dropped before the envelope is written.
        // Fire-and-forget polls must use `do_send`, or be awaited inside
        // a spawned future.
        for (device_id, addr) in &self.device_actors {
            let device_id = device_id.clone();
            let addr = addr.clone();
            ctx.spawn(
                async move {
                    let _ = addr
                        .send(DevicePoll {
                            device_id,
                            force: false,
                        })
                        .await;
                }
                .into_actor(self),
            );
        }
    }
}

/// Message to add a device
#[derive(Message)]
#[rtype(result = "()")]
pub struct AddDevice {
    pub device: Device,
}

impl Handler<AddDevice> for DeviceManagerActor {
    type Result = ();

    fn handle(&mut self, msg: AddDevice, _ctx: &mut Self::Context) -> Self::Result {
        self.add_device(msg.device);
    }
}

/// Message to remove a device
#[derive(Message)]
#[rtype(result = "bool")]
pub struct RemoveDevice {
    pub device_id: String,
}

impl Handler<RemoveDevice> for DeviceManagerActor {
    type Result = bool;

    fn handle(&mut self, msg: RemoveDevice, _ctx: &mut Self::Context) -> Self::Result {
        if self.device_actors.contains_key(&msg.device_id) {
            self.remove_device(&msg.device_id);
            true
        } else {
            false
        }
    }
}

/// Message to poll a specific device (returns immediately, actual poll is async)
#[derive(Message)]
#[rtype(result = "()")]
pub struct PollDevice {
    pub device_id: String,
}

impl Handler<PollDevice> for DeviceManagerActor {
    type Result = ();

    fn handle(&mut self, msg: PollDevice, _ctx: &mut Self::Context) -> Self::Result {
        if let Some(addr) = self.device_actors.get(&msg.device_id) {
            // Send poll request (async, result handled by DeviceActor)
            let _ = addr.do_send(DevicePoll {
                device_id: msg.device_id.clone(),
                force: true,
            });
        }
    }
}

/// Message to get device count
#[derive(Message)]
#[rtype(result = "usize")]
pub struct GetDeviceCount;

impl Handler<GetDeviceCount> for DeviceManagerActor {
    type Result = usize;

    fn handle(&mut self, _msg: GetDeviceCount, _ctx: &mut Self::Context) -> Self::Result {
        self.device_count()
    }
}

/// Hub-pushed desired-state config (arrives via the hub connector).
/// Thresholds apply to the prediction engine immediately; the poll
/// interval takes effect on the next sweep cycle.
#[derive(Message)]
#[rtype(result = "()")]
pub struct ApplyConfig {
    pub poll_interval_secs: Option<u64>,
    pub temperature_warning: f64,
    pub temperature_critical: f64,
}

impl Handler<ApplyConfig> for DeviceManagerActor {
    type Result = ();

    fn handle(&mut self, msg: ApplyConfig, _ctx: &mut Self::Context) -> Self::Result {
        tracing::info!(
            "Applying hub config: poll={:?}, thresholds={:?}/{}C",
            msg.poll_interval_secs,
            msg.temperature_warning,
            msg.temperature_critical
        );

        if let Some(secs) = msg.poll_interval_secs {
            self.default_poll_interval = secs.max(1);
        }
        if let Some(ref prediction) = self.prediction_actor {
            prediction.do_send(super::prediction_actor::UpdateThresholds {
                warning: msg.temperature_warning,
                critical: msg.temperature_critical,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_classify_device_pxi() {
        assert_eq!(classify_device("PXIe-8880"), DeviceType::Pxi);
        assert_eq!(classify_device("PXI-1042Q"), DeviceType::Pxi);
    }

    #[test]
    fn test_classify_device_cdaq() {
        assert_eq!(classify_device("cDAQ-9178"), DeviceType::CDaq);
        assert_eq!(classify_device("CompactDAQ-9189"), DeviceType::CDaq);
    }

    #[test]
    fn test_classify_device_daq() {
        assert_eq!(classify_device("USB-6343"), DeviceType::Daq);
        assert_eq!(classify_device("PCI-6221"), DeviceType::Daq);
    }

    #[test]
    fn test_classify_device_xnet() {
        assert_eq!(classify_device("NI-XNET"), DeviceType::Xnet);
        assert_eq!(classify_device("PXIe-8510"), DeviceType::Pxi); // PXIe prefix -> Pxi
    }

    #[test]
    fn test_classify_device_gpib() {
        assert_eq!(classify_device("GPIB-USB-HS"), DeviceType::Gpib);
    }

    #[test]
    fn test_classify_device_power() {
        assert_eq!(classify_device("NIPSPS-4010"), DeviceType::PowerSupply);
    }

    #[test]
    fn test_classify_device_unknown() {
        // Unknown product names default to Daq
        assert_eq!(classify_device("UNKNOWN-MODEL"), DeviceType::Daq);
    }

    #[test]
    fn test_simulated_devices() {
        let manager = DeviceManagerActor::new("test-edge".to_string());
        let devices = manager.simulated_devices();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].device_type, DeviceType::Daq);
        assert_eq!(devices[1].device_type, DeviceType::Pxi);
        assert!(devices[0].id.starts_with("test-edge:"));
        assert!(devices[1].id.starts_with("test-edge:"));
    }

    #[actix::test]
    async fn test_device_manager_starts() {
        let manager = DeviceManagerActor::new("test-edge".to_string());
        let _addr = manager.start();
        // Give actors time to start
        actix_rt::time::sleep(Duration::from_millis(100)).await;
    }

    #[actix::test]
    async fn test_list_devices() {
        let manager = DeviceManagerActor::new("test-edge".to_string());
        let addr = manager.start();

        // Wait for discovery
        actix_rt::time::sleep(Duration::from_millis(100)).await;

        let devices = addr.send(ListDevices).await.unwrap();
        // Real NI-SysCfg devices when available, simulated fallback otherwise
        assert!(!devices.is_empty());
    }

    #[actix::test]
    async fn test_get_device_count() {
        let manager = DeviceManagerActor::new("test-edge".to_string());
        let addr = manager.start();

        // Wait for discovery
        actix_rt::time::sleep(Duration::from_millis(100)).await;

        let count = addr.send(GetDeviceCount).await.unwrap();
        // Real NI-SysCfg devices when available, simulated fallback otherwise
        assert!(count >= 1);
    }

    #[actix::test]
    async fn test_add_device() {
        let manager = DeviceManagerActor::new("test-edge".to_string());
        let addr = manager.start();

        // Wait for initial discovery
        actix_rt::time::sleep(Duration::from_millis(100)).await;

        let initial_count = addr.send(GetDeviceCount).await.unwrap();

        // Add a new device
        let new_device = Device {
            id: "test-edge:new-device".to_string(),
            edge_id: "test-edge".to_string(),
            device_name: "NewDevice".to_string(),
            device_type: DeviceType::CDaq,
            model: Some("cDAQ-9178".to_string()),
            serial_number: None,
            firmware_version: None,
            driver_version: None,
            ip_address: None,
            slot: None,
            chassis: None,
            is_simulated: false,
        };

        addr.send(AddDevice { device: new_device }).await.unwrap();

        let new_count = addr.send(GetDeviceCount).await.unwrap();
        assert_eq!(new_count, initial_count + 1);
    }

    #[actix::test]
    async fn test_remove_device() {
        let manager = DeviceManagerActor::new("test-edge".to_string());
        let addr = manager.start();

        // Wait for discovery
        actix_rt::time::sleep(Duration::from_millis(100)).await;

        let initial_count = addr.send(GetDeviceCount).await.unwrap();

        // Remove an existing device (whatever discovery provided)
        let devices = addr.send(ListDevices).await.unwrap();
        let device_id = devices[0].clone();
        let removed = addr.send(RemoveDevice { device_id }).await.unwrap();
        assert!(removed);

        let new_count = addr.send(GetDeviceCount).await.unwrap();
        assert_eq!(new_count, initial_count - 1);

        // Try to remove non-existent device
        let removed = addr
            .send(RemoveDevice {
                device_id: "non-existent".to_string(),
            })
            .await
            .unwrap();
        assert!(!removed);
    }

    #[actix::test]
    async fn test_poll_specific_device() {
        let manager = DeviceManagerActor::new("test-edge".to_string());
        let addr = manager.start();

        // Wait for discovery
        actix_rt::time::sleep(Duration::from_millis(100)).await;

        // Poll existing device (async, no result)
        addr.send(PollDevice {
            device_id: "test-edge:daq-1".to_string(),
        })
        .await
        .unwrap();

        // Poll non-existent device (no-op)
        addr.send(PollDevice {
            device_id: "non-existent".to_string(),
        })
        .await
        .unwrap();
    }

    #[actix::test]
    async fn test_custom_poll_interval() {
        let manager = DeviceManagerActor::new("test-edge".to_string()).with_poll_interval(30);
        let addr = manager.start();

        // Wait for discovery
        actix_rt::time::sleep(Duration::from_millis(100)).await;

        let devices = addr.send(ListDevices).await.unwrap();
        // Real NI-SysCfg devices when available, simulated fallback otherwise
        assert!(!devices.is_empty());
    }
}
