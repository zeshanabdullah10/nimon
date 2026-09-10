//! Device monitoring actor

use actix::prelude::*;
use chrono::Utc;
use std::collections::HashMap;
use std::time::Duration;

use nimon_core::{
    actor::{DevicePoll, DevicePollResult, DeviceStatusUpdate},
    Device, HealthStatus, MetricValue,
};

/// Actor that monitors a single NI device
pub struct DeviceActor {
    /// Device information
    device: Device,
    /// Polling interval
    poll_interval: Duration,
    /// Recipient for status updates
    status_recipient: Option<Recipient<DeviceStatusUpdate>>,
    /// Last known status
    last_status: HealthStatus,
    /// Consecutive error count
    error_count: u32,
    /// When true the device manager drives updates via HealthUpdate
    /// (shared single-enumeration sweep) and this actor does not poll.
    managed: bool,
}

impl DeviceActor {
    pub fn new(device: Device, poll_interval_secs: u64) -> Self {
        Self {
            device,
            poll_interval: Duration::from_secs(poll_interval_secs),
            status_recipient: None,
            last_status: HealthStatus::Offline,
            error_count: 0,
            managed: false,
        }
    }

    /// Set recipient for status updates
    pub fn with_status_recipient(mut self, recipient: Recipient<DeviceStatusUpdate>) -> Self {
        self.status_recipient = Some(recipient);
        self
    }

    /// Mark this actor as externally driven (no self-polling)
    pub fn with_managed_polling(mut self) -> Self {
        self.managed = true;
        self
    }

    /// Convert a health report into a poll result and emit it downstream
    fn apply_health(&mut self, health: nimon_ni::syscfg::DeviceHealth) -> DevicePollResult {
        let (status, mut metrics) = health.to_status_and_metrics();

        // static identity metrics that the downstream UIs render
        if let Some(slot) = self.device.slot {
            metrics.insert("slot".to_string(), MetricValue::Integer(slot as i64));
        }
        if let Some(ref model) = self.device.model {
            metrics.insert("product".to_string(), MetricValue::String(model.clone()));
        }

        self.last_status = status;
        DevicePollResult {
            success: true,
            status,
            metrics,
            error: None,
            timestamp: Utc::now(),
        }
    }

    /// Forward a poll result to the status recipient
    fn emit(&self, result: DevicePollResult) {
        if let Some(recipient) = &self.status_recipient {
            let update = DeviceStatusUpdate {
                device_id: self.device.id.clone(),
                edge_id: self.device.edge_id.clone(),
                status: result.status,
                metrics: result.metrics,
                timestamp: result.timestamp,
            };
            let _ = recipient.do_send(update);
        }
    }

    /// Poll device health via NI-SysCfg, falling back to simulated data
    fn poll_device(&mut self) -> DevicePollResult {
        // Try real NI-SysCfg health polling first
        match nimon_ni::syscfg::NiSysCfg::load() {
            Ok(api) => match api.create_session() {
                Ok(session) => {
                    // Use device_name (product name) as the lookup key for NI-SysCfg
                    match session.get_device_health(&self.device.device_name) {
                        Ok(health) => {
                            let result = self.apply_health(health);
                            tracing::trace!(
                                "Polled {} via NI-SysCfg: status={:?}",
                                self.device.device_name,
                                result.status
                            );
                            return result;
                        }
                        Err(e) => {
                            tracing::debug!(
                                "NI-SysCfg health query failed for {}: {}, using simulated",
                                self.device.device_name,
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        "NI-SysCfg session failed: {}, using simulated polling",
                        e
                    );
                }
            },
            Err(_) => {
                tracing::trace!("NI-SysCfg not available, using simulated polling");
            }
        }

        // Simulated fallback
        self.simulated_metrics()
    }

    /// Generate simulated device metrics for development/testing
    fn simulated_metrics(&mut self) -> DevicePollResult {
        let mut metrics = HashMap::new();
        let temperature = 40.0 + (rand_factor() * 30.0);
        metrics.insert("temperature".to_string(), MetricValue::Float(temperature));
        metrics.insert(
            "error_count".to_string(),
            MetricValue::Integer(self.error_count as i64),
        );

        let status = if temperature > 75.0 {
            HealthStatus::Error
        } else if temperature > 65.0 {
            HealthStatus::Warning
        } else {
            HealthStatus::Healthy
        };

        self.last_status = status;

        DevicePollResult {
            success: true,
            status,
            metrics,
            error: None,
            timestamp: Utc::now(),
        }
    }

    /// Start periodic polling
    fn start_polling(&self, ctx: &mut Context<Self>) {
        if self.managed {
            // The device manager sweeps NI-SysCfg once per interval and
            // pushes HealthUpdate messages; nothing to do here.
            return;
        }

        let interval_duration = self.poll_interval;

        ctx.run_interval(interval_duration, |act, _ctx| {
            let result = act.poll_device();
            act.emit(result);
        });
    }
}

impl Actor for DeviceActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        tracing::info!("DeviceActor started for {}", self.device.device_name);
        self.start_polling(ctx);
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        tracing::info!("DeviceActor stopped for {}", self.device.device_name);
    }
}

impl Handler<DevicePoll> for DeviceActor {
    type Result = MessageResult<DevicePoll>;

    fn handle(&mut self, _msg: DevicePoll, _ctx: &mut Self::Context) -> Self::Result {
        MessageResult(self.poll_device())
    }
}

/// Health report pushed by the device manager's shared sweep
/// (one NI-SysCfg enumeration per cycle for all devices)
#[derive(Message)]
#[rtype(result = "()")]
pub struct HealthUpdate {
    pub health: nimon_ni::syscfg::DeviceHealth,
}

impl Handler<HealthUpdate> for DeviceActor {
    type Result = ();

    fn handle(&mut self, msg: HealthUpdate, _ctx: &mut Self::Context) -> Self::Result {
        let result = self.apply_health(msg.health);
        self.emit(result);
    }
}

/// Ask the actor to stop (used when hardware topology changes)
#[derive(Message)]
#[rtype(result = "()")]
pub struct StopDevice;

impl Handler<StopDevice> for DeviceActor {
    type Result = ();

    fn handle(&mut self, _msg: StopDevice, ctx: &mut Self::Context) -> Self::Result {
        ctx.stop();
    }
}

/// Simple random factor for simulation (0.0 to 1.0)
fn rand_factor() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    ns as f64 / u32::MAX as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_core::DeviceType;

    fn make_test_device() -> Device {
        Device {
            id: "test-1".to_string(),
            edge_id: "edge-1".to_string(),
            device_name: "TestDevice".to_string(),
            device_type: DeviceType::Daq,
            model: None,
            serial_number: None,
            firmware_version: None,
            driver_version: None,
            ip_address: None,
            slot: None,
            chassis: None,
        }
    }

    #[test]
    fn test_device_actor_creation() {
        let device = make_test_device();
        let actor = DeviceActor::new(device.clone(), 10);
        assert_eq!(actor.device.id, "test-1");
        assert_eq!(actor.poll_interval, Duration::from_secs(10));
    }

    #[test]
    fn test_poll_device() {
        let device = make_test_device();
        let mut actor = DeviceActor::new(device, 10);
        let result = actor.poll_device();
        assert!(result.success);
        // With real NI-SysCfg available an unknown device name resolves to an
        // offline status without simulated metrics; without NI software the
        // simulated fallback always reports a temperature metric.
        if nimon_ni::syscfg::NiSysCfg::is_available() {
            assert_eq!(result.status, HealthStatus::Offline);
        } else {
            assert!(result.metrics.contains_key("temperature"));
        }
    }

    #[test]
    fn test_simulated_metrics_fallback() {
        let device = make_test_device();
        let mut actor = DeviceActor::new(device, 10);
        let result = actor.simulated_metrics();
        assert!(result.success);
        assert!(result.metrics.contains_key("temperature"));
        assert!(result.error.is_none());
    }

    #[test]
    fn test_poll_device_temperature_status() {
        let device = make_test_device();
        let mut actor = DeviceActor::new(device, 10);

        // Poll multiple times to verify status determination
        for _ in 0..10 {
            let result = actor.poll_device();
            if let Some(MetricValue::Float(temp)) = result.metrics.get("temperature") {
                if *temp > 75.0 {
                    assert_eq!(result.status, HealthStatus::Error);
                } else if *temp > 65.0 {
                    assert_eq!(result.status, HealthStatus::Warning);
                } else {
                    assert_eq!(result.status, HealthStatus::Healthy);
                }
            }
        }
    }
}
