//! Device manager actor

use actix::prelude::*;
use std::collections::HashMap;

use nimon_core::{
    actor::DevicePoll,
    Device, DeviceType,
};

use super::device_actor::DeviceActor;
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
    /// Prediction actor address
    prediction_actor: Option<Addr<PredictionActor>>,
    /// Edge ID
    edge_id: String,
    /// Default poll interval
    default_poll_interval: u64,
}

impl DeviceManagerActor {
    pub fn new(edge_id: String) -> Self {
        Self {
            device_actors: HashMap::new(),
            prediction_actor: None,
            edge_id,
            default_poll_interval: 10,
        }
    }

    /// Set default poll interval
    pub fn with_poll_interval(mut self, interval_secs: u64) -> Self {
        self.default_poll_interval = interval_secs;
        self
    }

    /// Start the prediction actor
    fn start_prediction_actor(&mut self, _ctx: &mut Context<Self>) {
        let actor = PredictionActor::new(10).with_thresholds(65.0, 75.0);
        self.prediction_actor = Some(actor.start());
        tracing::info!("Prediction actor started");
    }

    /// Add a device to be monitored
    fn add_device(&mut self, device: Device) {
        let device_id = device.id.clone();

        let actor = DeviceActor::new(device, self.default_poll_interval);
        let addr = actor.start();

        self.device_actors.insert(device_id.clone(), addr);
        tracing::info!("Added device: {}", device_id);
    }

    /// Remove a device from monitoring
    fn remove_device(&mut self, device_id: &str) {
        if let Some(_addr) = self.device_actors.remove(device_id) {
            // Actor will be stopped when its address is dropped
            tracing::info!("Removed device: {}", device_id);
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
                            return discovered
                                .into_iter()
                                .map(|d| {
                                    let device_type = classify_device(&d.product_name);
                                    Device {
                                        id: format!("{}:{}", self.edge_id, d.serial_number),
                                        edge_id: self.edge_id.clone(),
                                        device_name: d.product_name.clone(),
                                        device_type,
                                        model: Some(d.product_name),
                                        serial_number: Some(d.serial_number),
                                        firmware_version: d.firmware_version,
                                        driver_version: d.driver_version,
                                        ip_address: d.ip_address,
                                        slot: None,
                                        chassis: None,
                                    }
                                })
                                .collect();
                        }
                        tracing::info!(
                            "NI-SysCfg returned no devices, using simulated fallback"
                        );
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
            },
        ]
    }

    /// Get count of managed devices
    pub fn device_count(&self) -> usize {
        self.device_actors.len()
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

    fn handle(&mut self, _msg: PollAllDevices, _ctx: &mut Self::Context) -> Self::Result {
        for (device_id, addr) in &self.device_actors {
            let _ = addr.send(DevicePoll {
                device_id: device_id.clone(),
                force: false,
            });
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
        assert!(!devices.is_empty());
        assert!(devices.contains(&"test-edge:daq-1".to_string()));
        assert!(devices.contains(&"test-edge:pxi-1".to_string()));
    }

    #[actix::test]
    async fn test_get_device_count() {
        let manager = DeviceManagerActor::new("test-edge".to_string());
        let addr = manager.start();

        // Wait for discovery
        actix_rt::time::sleep(Duration::from_millis(100)).await;

        let count = addr.send(GetDeviceCount).await.unwrap();
        assert_eq!(count, 2);
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

        // Remove a device
        let removed = addr
            .send(RemoveDevice {
                device_id: "test-edge:daq-1".to_string(),
            })
            .await
            .unwrap();
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
        assert_eq!(devices.len(), 2);
    }
}
