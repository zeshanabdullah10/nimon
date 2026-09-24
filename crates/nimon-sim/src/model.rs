//! Pure simulation model: device layout, temperature generators and the
//! per-edge state machine that turns one sweep ("tick") into protocol
//! messages. No I/O here, so everything is unit-testable with a seed.

use chrono::Utc;
use nimon_core::actor::messages::{
    ConfigUpdate, DeviceAlert, DeviceRemoved, DeviceStatusUpdate, EdgeHeartbeat, EdgeRegister,
    PredictionResult,
};
use nimon_core::protocol::WsMessage;
use nimon_core::{
    HealthStatus, MetricValue, PredictionType, Severity, DEFAULT_TEMP_CRITICAL_C,
    DEFAULT_TEMP_WARNING_C,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::time::Duration;

use crate::cli::Scenario;

/// Overheat: temperature gain per tick (C)
pub const OVERHEAT_RATE_C_PER_TICK: f64 = 1.0;
/// Overheat: plateau this far above the critical threshold (C)
pub const OVERHEAT_PLATEAU_ABOVE_CRITICAL_C: f64 = 5.0;
/// Overheat/flap/offline: healthy ticks before the scenario kicks in
pub const WARMUP_TICKS: u64 = 3;
/// Overheat: send a prediction every N ticks while ramping
pub const PREDICTION_EVERY_TICKS: u64 = 3;
/// Flap: oscillation amplitude around the warning threshold (C)
pub const FLAP_AMPLITUDE_C: f64 = 2.5;
/// Flap: oscillation period (ticks)
pub const FLAP_PERIOD_TICKS: f64 = 6.0;
/// Offline: the target device goes silent after this many ticks
pub const OFFLINE_AFTER_TICKS: u64 = 6;
/// Offline: `device_removed` is sent this many ticks after going silent
pub const REMOVE_AFTER_SILENT_TICKS: u64 = 6;
/// Reconnect: drop the socket after this many ticks in a session
pub const RECONNECT_EVERY_TICKS: u64 = 6;

/// Model version reported on simulated predictions
pub const MODEL_VERSION: &str = "sim-ramp-v1";

/// Round to two decimals (readable dashboards, stable JSON)
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ============================================================================
// Temperature generators
// ============================================================================

/// Mean-reverting random walk clamped to `[min, max]`.
#[derive(Debug, Clone)]
pub struct RandomWalk {
    pub value: f64,
    pub baseline: f64,
    pub min: f64,
    pub max: f64,
    /// Maximum random change per step
    pub step: f64,
    /// Fraction of the distance to the baseline recovered per step
    pub reversion: f64,
}

impl RandomWalk {
    pub fn new(baseline: f64, half_range: f64, step: f64) -> Self {
        Self {
            value: baseline,
            baseline,
            min: baseline - half_range,
            max: baseline + half_range,
            step,
            reversion: 0.1,
        }
    }

    /// Never exceed `ceiling` (keeps a "steady" device below warning).
    pub fn with_ceiling(mut self, ceiling: f64) -> Self {
        self.max = self.max.min(ceiling);
        self.min = self.min.min(self.max);
        self.baseline = self.baseline.clamp(self.min, self.max);
        self.value = self.value.clamp(self.min, self.max);
        self
    }

    pub fn next<R: Rng>(&mut self, rng: &mut R) -> f64 {
        let drift = self.reversion * (self.baseline - self.value);
        let noise = rng.gen_range(-self.step..=self.step);
        self.value = (self.value + drift + noise).clamp(self.min, self.max);
        self.value
    }
}

/// Linear ramp from `start` that plateaus above the critical threshold.
#[derive(Debug, Clone)]
pub struct Ramp {
    pub start: f64,
    pub rate: f64,
}

impl Ramp {
    fn plateau(&self, critical: f64) -> f64 {
        (critical + OVERHEAT_PLATEAU_ABOVE_CRITICAL_C).max(self.start)
    }

    /// Noise-free temperature `ramp_tick` ticks into the ramp.
    pub fn temperature(&self, ramp_tick: u64, critical: f64) -> f64 {
        (self.start + self.rate * ramp_tick as f64).min(self.plateau(critical))
    }

    /// True while the ramp has not reached its plateau.
    pub fn is_rising(&self, ramp_tick: u64, critical: f64) -> bool {
        self.start + self.rate * (ramp_tick as f64) < self.plateau(critical)
    }
}

/// Noise-free flap temperature: a sine around the warning threshold.
/// The half-tick phase offset avoids samples landing exactly on it.
pub fn flap_temperature(warning: f64, tick: u64) -> f64 {
    let phase = 2.0 * std::f64::consts::PI * (tick as f64 + 0.5) / FLAP_PERIOD_TICKS;
    warning + FLAP_AMPLITUDE_C * phase.sin()
}

/// Minutes until `current` reaches `target` rising at `rate_per_tick`
/// with one tick every `interval`. `None` when not rising.
pub fn eta_minutes(
    current: f64,
    target: f64,
    rate_per_tick: f64,
    interval: Duration,
) -> Option<i32> {
    if rate_per_tick <= 0.0 || !current.is_finite() || !target.is_finite() {
        return None;
    }
    let remaining = (target - current).max(0.0);
    let secs = remaining / rate_per_tick * interval.as_secs_f64();
    Some((secs / 60.0).ceil() as i32)
}

/// Health from temperature, mirroring the edge's NI-SysCfg mapping.
pub fn classify(temperature: f64, warning: f64, critical: f64) -> HealthStatus {
    if temperature > critical {
        HealthStatus::Error
    } else if temperature > warning {
        HealthStatus::Warning
    } else {
        HealthStatus::Healthy
    }
}

fn status_rank(s: HealthStatus) -> u8 {
    match s {
        HealthStatus::Healthy => 0,
        HealthStatus::Warning => 1,
        HealthStatus::Error => 2,
        HealthStatus::Offline => 3,
    }
}

#[derive(Debug, Clone)]
pub enum TempModel {
    Walk(RandomWalk),
    Ramp(Ramp),
    Flap,
}

// ============================================================================
// Device layout
// ============================================================================

#[derive(Debug, Clone, Copy)]
enum Kind {
    Pxi(u32),
    Usb,
    CDaq,
    Controller,
}

/// Hardware a simulated edge "discovers", in enumeration order.
const CATALOG: &[(&str, Kind)] = &[
    ("PXIe-6368", Kind::Pxi(2)),
    ("PXIe-4081", Kind::Pxi(3)),
    ("USB-6343", Kind::Usb),
    ("PXIe-8880", Kind::Controller),
    ("PXIe-5171", Kind::Pxi(4)),
    ("cDAQ-9189", Kind::CDaq),
    ("PXIe-4139", Kind::Pxi(5)),
    ("PXIe-6570", Kind::Pxi(6)),
];

/// Product shared by the two devices in the `duplicate` scenario
pub const DUPLICATE_PRODUCT: &str = "PXIe-6368";

/// A device as discovered: id key (the part after `<edge_id>:`), product
/// and slot. `controller` devices carry the station memory/disk metrics
/// (the edge attaches them to the resource aliased as the hostname).
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceSpec {
    pub key: String,
    pub product: String,
    pub slot: Option<u32>,
    pub controller: bool,
}

/// Device id as the real edge builds it: `<edge_id>:<key>`.
pub fn device_id(edge_id: &str, key: &str) -> String {
    format!("{edge_id}:{key}")
}

/// Id key for an un-aliased device without serial: `<product>#<index+1>`.
pub fn indexed_key(product: &str, index: usize) -> String {
    format!("{product}#{}", index + 1)
}

/// Build `count` devices. Normally keys are NI MAX aliases (`PXI1Slot2`,
/// `Dev1`, the hostname for the controller). With `duplicate`, devices 0
/// and 1 are both [`DUPLICATE_PRODUCT`] without alias, so they get the
/// `#1`/`#2` suffixed keys.
pub fn device_layout(count: usize, hostname: &str, duplicate: bool) -> Vec<DeviceSpec> {
    (0..count)
        .map(|i| {
            let (product, kind) = CATALOG[i % CATALOG.len()];
            let chassis = i / CATALOG.len() + 1;
            let (alias, slot) = match kind {
                Kind::Pxi(slot) => (format!("PXI{chassis}Slot{slot}"), Some(slot)),
                Kind::Usb => (format!("Dev{chassis}"), None),
                Kind::CDaq => (format!("cDAQ{chassis}"), None),
                Kind::Controller if chassis == 1 => (hostname.to_string(), Some(1)),
                Kind::Controller => (format!("{hostname}-{chassis}"), Some(1)),
            };
            if duplicate && i < 2 {
                DeviceSpec {
                    key: indexed_key(DUPLICATE_PRODUCT, i),
                    product: DUPLICATE_PRODUCT.to_string(),
                    slot,
                    controller: false,
                }
            } else {
                DeviceSpec {
                    key: alias,
                    product: product.to_string(),
                    slot,
                    controller: matches!(kind, Kind::Controller),
                }
            }
        })
        .collect()
}

// ============================================================================
// Devices and edges
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Active,
    Silent { since_tick: u64 },
    Removed,
}

#[derive(Debug, Clone)]
pub struct DeviceSim {
    pub id: String,
    pub spec: DeviceSpec,
    pub model: TempModel,
    pub temperature: f64,
    pub status: HealthStatus,
    pub lifecycle: Lifecycle,
    /// Tick at which the device goes silent (offline scenario)
    pub offline_at: Option<u64>,
    mem_free_mb: f64,
}

/// Result of applying a hub `config_update`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigChange {
    pub old_thresholds: (f64, f64),
    pub new_thresholds: (f64, f64),
    pub old_interval: Duration,
    pub new_interval: Duration,
}

impl ConfigChange {
    pub fn interval_changed(&self) -> bool {
        self.old_interval != self.new_interval
    }
}

/// One simulated edge node.
pub struct EdgeSim {
    pub edge_id: String,
    pub name: String,
    pub hostname: String,
    pub ip_address: String,
    pub devices: Vec<DeviceSim>,
    pub warning: f64,
    pub critical: f64,
    pub interval: Duration,
    /// Completed sweeps
    pub tick: u64,
    rng: StdRng,
}

/// Derive a per-edge seed from the run seed.
pub fn edge_seed(seed: u64, index: usize) -> u64 {
    seed ^ (index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

impl EdgeSim {
    /// Build edge `index`. Single-device scenarios (overheat/flap/offline)
    /// target device 0 of edge 0 only.
    pub fn new(
        index: usize,
        prefix: &str,
        device_count: usize,
        scenario: Scenario,
        seed: u64,
        interval: Duration,
    ) -> Self {
        let edge_id = format!("{prefix}-{:02}", index + 1);
        let hostname = format!("sim-host-{:02}", index + 1);
        let mut rng = StdRng::seed_from_u64(edge_seed(seed, index));
        let warning = DEFAULT_TEMP_WARNING_C;
        let critical = DEFAULT_TEMP_CRITICAL_C;

        let layout = device_layout(device_count, &hostname, scenario == Scenario::Duplicate);
        let devices = layout
            .into_iter()
            .enumerate()
            .map(|(i, spec)| {
                let baseline = round2(rng.gen_range(38.0..50.0));
                let target = index == 0 && i == 0;
                let model = match scenario {
                    Scenario::Overheat if target => TempModel::Ramp(Ramp {
                        start: baseline,
                        rate: OVERHEAT_RATE_C_PER_TICK,
                    }),
                    Scenario::Flap if target => TempModel::Flap,
                    _ => TempModel::Walk(
                        RandomWalk::new(baseline, 6.0, 0.4).with_ceiling(warning - 3.0),
                    ),
                };
                DeviceSim {
                    id: device_id(&edge_id, &spec.key),
                    spec,
                    model,
                    temperature: baseline,
                    status: HealthStatus::Healthy,
                    lifecycle: Lifecycle::Active,
                    offline_at: (scenario == Scenario::Offline && target)
                        .then_some(OFFLINE_AFTER_TICKS),
                    mem_free_mb: 9000.0,
                }
            })
            .collect();

        Self {
            name: format!("Simulated Edge {}", index + 1),
            ip_address: format!("192.0.2.{}", index % 250 + 1),
            edge_id,
            hostname,
            devices,
            warning,
            critical,
            interval,
            tick: 0,
            rng,
        }
    }

    pub fn register(&self) -> WsMessage {
        WsMessage::edge_register(EdgeRegister {
            edge_id: self.edge_id.clone(),
            name: self.name.clone(),
            hostname: Some(self.hostname.clone()),
            ip_address: Some(self.ip_address.clone()),
        })
    }

    /// Devices the edge still tracks (silent devices count until removed,
    /// like device actors on the real edge).
    pub fn device_count(&self) -> usize {
        self.devices
            .iter()
            .filter(|d| d.lifecycle != Lifecycle::Removed)
            .count()
    }

    pub fn has_device(&self, device_id: &str) -> bool {
        self.devices
            .iter()
            .any(|d| d.id == device_id && d.lifecycle != Lifecycle::Removed)
    }

    pub fn heartbeat(&self, uptime_secs: u64) -> WsMessage {
        WsMessage::heartbeat(EdgeHeartbeat {
            edge_id: self.edge_id.clone(),
            timestamp: Utc::now(),
            device_count: self.device_count(),
            // matches the edge's `format!("{:?}", ConnectionState::Connected)`
            status: "Connected".to_string(),
            uptime_secs: Some(uptime_secs),
            version: Some(format!("{}-sim", env!("CARGO_PKG_VERSION"))),
        })
    }

    /// Apply a hub config push (partial thresholds via `resolve`).
    pub fn apply_config(&mut self, update: &ConfigUpdate) -> ConfigChange {
        let old_thresholds = (self.warning, self.critical);
        let old_interval = self.interval;
        let (w, c) = update.thresholds.resolve(self.warning, self.critical);
        self.warning = w;
        self.critical = c;
        if let Some(secs) = update.poll_interval_secs.filter(|s| *s > 0) {
            self.interval = Duration::from_secs(secs);
        }
        ConfigChange {
            old_thresholds,
            new_thresholds: (w, c),
            old_interval,
            new_interval: self.interval,
        }
    }

    /// Messages that re-establish the full edge state on the hub
    /// (`hub_command: resend_state`): register + current device statuses.
    pub fn state_snapshot(&self) -> Vec<WsMessage> {
        let mut out = vec![self.register()];
        out.extend(
            self.devices
                .iter()
                .filter(|d| d.lifecycle == Lifecycle::Active)
                .map(|d| status_message(&self.edge_id, d)),
        );
        out
    }

    /// Run one sweep: sample every device and return the resulting
    /// `device_status` / `device_alert` / `prediction` / `device_removed`
    /// messages (heartbeat is separate).
    pub fn step(&mut self) -> Vec<WsMessage> {
        let t = self.tick;
        let (warning, critical, interval) = (self.warning, self.critical, self.interval);
        let mut out = Vec::new();

        for d in self.devices.iter_mut() {
            match d.lifecycle {
                Lifecycle::Removed => continue,
                Lifecycle::Silent { since_tick } => {
                    if t >= since_tick + REMOVE_AFTER_SILENT_TICKS {
                        d.lifecycle = Lifecycle::Removed;
                        out.push(WsMessage::device_removed(DeviceRemoved {
                            edge_id: self.edge_id.clone(),
                            device_id: d.id.clone(),
                            timestamp: Utc::now(),
                        }));
                    }
                    continue;
                }
                Lifecycle::Active => {
                    if d.offline_at.is_some_and(|at| t >= at) {
                        d.lifecycle = Lifecycle::Silent { since_tick: t };
                        continue;
                    }
                }
            }

            let ramp_tick = t.saturating_sub(WARMUP_TICKS);
            let raw = match &mut d.model {
                TempModel::Walk(w) => w.next(&mut self.rng),
                TempModel::Ramp(r) => {
                    r.temperature(ramp_tick, critical) + self.rng.gen_range(-0.15..=0.15)
                }
                TempModel::Flap if t < WARMUP_TICKS => {
                    warning - 8.0 + self.rng.gen_range(-0.3..=0.3)
                }
                TempModel::Flap => flap_temperature(warning, t) + self.rng.gen_range(-0.1..=0.1),
            };
            d.temperature = round2(raw);
            if d.spec.controller {
                d.mem_free_mb =
                    (d.mem_free_mb + self.rng.gen_range(-150.0..=150.0)).clamp(6000.0, 12000.0);
            }

            let status = classify(d.temperature, warning, critical);
            if status_rank(status) > status_rank(d.status) {
                let (severity, threshold, label) = if status == HealthStatus::Error {
                    (Severity::Critical, critical, "critical")
                } else {
                    (Severity::Warning, warning, "warning")
                };
                out.push(WsMessage::device_alert(DeviceAlert {
                    device_id: d.id.clone(),
                    edge_id: self.edge_id.clone(),
                    severity,
                    message: format!(
                        "{} temperature {:.1} C exceeds {label} threshold {:.1} C",
                        d.spec.product, d.temperature, threshold
                    ),
                    metric_name: Some("temperature".to_string()),
                    metric_value: Some(d.temperature),
                    timestamp: Utc::now(),
                }));
            }
            d.status = status;
            out.push(status_message(&self.edge_id, d));

            if let TempModel::Ramp(r) = &d.model {
                let due = t >= WARMUP_TICKS
                    && ramp_tick >= PREDICTION_EVERY_TICKS
                    && ramp_tick.is_multiple_of(PREDICTION_EVERY_TICKS);
                if due && r.is_rising(ramp_tick, critical) && d.temperature < critical {
                    out.push(WsMessage::prediction(ramp_prediction(
                        &self.edge_id,
                        d,
                        r,
                        critical,
                        interval,
                    )));
                }
            }
        }

        self.tick += 1;
        out
    }
}

fn ramp_prediction(
    edge_id: &str,
    d: &DeviceSim,
    ramp: &Ramp,
    critical: f64,
    interval: Duration,
) -> PredictionResult {
    let span = (critical - ramp.start).max(1.0);
    let progress = ((d.temperature - ramp.start) / span).clamp(0.0, 1.0);
    PredictionResult {
        device_id: d.id.clone(),
        edge_id: edge_id.to_string(),
        prediction_type: PredictionType::Overheating,
        probability: round2(0.5 + 0.45 * progress),
        eta_minutes: eta_minutes(d.temperature, critical, ramp.rate, interval),
        confidence: 0.85,
        reason: Some(format!(
            "temperature rising {:.1} C per {:.1}s sweep; projected to reach critical {:.1} C",
            ramp.rate,
            interval.as_secs_f64(),
            critical
        )),
        model_version: Some(MODEL_VERSION.to_string()),
        timestamp: Utc::now(),
    }
}

/// Status message with the metric keys the real edge sends via NI-SysCfg.
fn status_message(edge_id: &str, d: &DeviceSim) -> WsMessage {
    let mut metrics = HashMap::new();
    metrics.insert("temperature".to_string(), MetricValue::Float(d.temperature));
    metrics.insert("is_reachable".to_string(), MetricValue::Boolean(true));
    metrics.insert("self_test_passed".to_string(), MetricValue::Boolean(true));
    metrics.insert(
        "product".to_string(),
        MetricValue::String(d.spec.product.clone()),
    );
    if let Some(slot) = d.spec.slot {
        metrics.insert("slot".to_string(), MetricValue::Integer(slot as i64));
    }
    if d.spec.controller {
        for (k, v) in [
            ("mem_total_mb", 16384.0),
            ("mem_free_mb", round2(d.mem_free_mb)),
            ("disk_total_mb", 475_000.0),
            ("disk_free_mb", 212_000.0),
        ] {
            metrics.insert(k.to_string(), MetricValue::Float(v));
        }
    }
    WsMessage::device_status(DeviceStatusUpdate {
        device_id: d.id.clone(),
        edge_id: edge_id.to_string(),
        status: d.status,
        metrics,
        timestamp: Utc::now(),
        is_simulated: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_core::actor::messages::ThresholdConfig;
    use nimon_core::protocol::WsMessageType;

    const SECS5: Duration = Duration::from_secs(5);

    fn temps(sim: &mut EdgeSim, device: usize, ticks: usize) -> Vec<f64> {
        (0..ticks)
            .map(|_| {
                sim.step();
                sim.devices[device].temperature
            })
            .collect()
    }

    fn count(msgs: &[WsMessage], t: WsMessageType) -> usize {
        msgs.iter().filter(|m| m.msg_type == t).count()
    }

    #[test]
    fn random_walk_stays_in_bounds_and_is_deterministic() {
        let run = |seed| {
            let mut rng = StdRng::seed_from_u64(seed);
            let mut w = RandomWalk::new(45.0, 6.0, 0.4).with_ceiling(62.0);
            (0..5000).map(|_| w.next(&mut rng)).collect::<Vec<_>>()
        };
        let a = run(42);
        assert!(a.iter().all(|v| (39.0..=51.0).contains(v)), "out of bounds");
        assert_eq!(a, run(42), "same seed must reproduce");
        assert_ne!(a, run(43));
        // consecutive samples move by a bounded amount (a walk, not noise)
        assert!(a
            .windows(2)
            .all(|p| (p[1] - p[0]).abs() <= 0.4 + 0.1 * 6.0 + 1e-9));
    }

    #[test]
    fn random_walk_ceiling_clamps() {
        let mut rng = StdRng::seed_from_u64(1);
        let mut w = RandomWalk::new(60.0, 10.0, 2.0).with_ceiling(55.0);
        assert!(w.value <= 55.0 && w.baseline <= 55.0);
        for _ in 0..1000 {
            assert!(w.next(&mut rng) <= 55.0);
        }
    }

    #[test]
    fn ramp_rises_then_plateaus() {
        let r = Ramp {
            start: 45.0,
            rate: 1.0,
        };
        assert_eq!(r.temperature(0, 75.0), 45.0);
        assert_eq!(r.temperature(10, 75.0), 55.0);
        assert!(r.is_rising(34, 75.0));
        assert_eq!(r.temperature(35, 75.0), 80.0);
        assert!(!r.is_rising(35, 75.0));
        assert_eq!(r.temperature(500, 75.0), 80.0);
    }

    #[test]
    fn flap_crosses_warning_both_ways() {
        let v: Vec<f64> = (0..12).map(|t| flap_temperature(65.0, t)).collect();
        assert!(v.iter().any(|t| *t > 65.0 + 1.0));
        assert!(v.iter().any(|t| *t < 65.0 - 1.0));
        assert!(v
            .iter()
            .all(|t| (t - 65.0).abs() <= FLAP_AMPLITUDE_C + 1e-9));
        assert!(v.iter().all(|t| *t != 65.0));
    }

    #[test]
    fn eta_from_ramp() {
        // 20 C to go at 1 C per 5 s -> 100 s -> 2 min (ceil)
        assert_eq!(eta_minutes(55.0, 75.0, 1.0, SECS5), Some(2));
        // 30 C at 1 C per 60 s -> 30 min
        assert_eq!(
            eta_minutes(45.0, 75.0, 1.0, Duration::from_secs(60)),
            Some(30)
        );
        assert_eq!(eta_minutes(80.0, 75.0, 1.0, SECS5), Some(0));
        assert_eq!(eta_minutes(50.0, 75.0, 0.0, SECS5), None);
    }

    #[test]
    fn classify_mirrors_edge() {
        assert_eq!(classify(65.0, 65.0, 75.0), HealthStatus::Healthy);
        assert_eq!(classify(65.1, 65.0, 75.0), HealthStatus::Warning);
        assert_eq!(classify(75.1, 65.0, 75.0), HealthStatus::Error);
    }

    #[test]
    fn device_ids_match_edge_format() {
        assert_eq!(device_id("e1", "PXI1Slot2"), "e1:PXI1Slot2");
        assert_eq!(indexed_key("PXIe-6368", 0), "PXIe-6368#1");

        let layout = device_layout(20, "host", false);
        let mut keys: Vec<_> = layout.iter().map(|d| d.key.clone()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 20, "keys must be unique");
        assert_eq!(layout[0].key, "PXI1Slot2");
        assert_eq!(layout[3].key, "host");
        assert!(layout[3].controller);

        let dup = device_layout(4, "host", true);
        assert_eq!(dup[0].key, "PXIe-6368#1");
        assert_eq!(dup[1].key, "PXIe-6368#2");
        assert_eq!(dup[0].product, dup[1].product);

        let sim = EdgeSim::new(1, "sim-edge", 2, Scenario::Duplicate, 1, SECS5);
        assert_eq!(sim.edge_id, "sim-edge-02");
        assert_eq!(sim.devices[1].id, "sim-edge-02:PXIe-6368#2");
    }

    #[test]
    fn steady_never_warns_with_defaults() {
        let mut sim = EdgeSim::new(0, "e", 8, Scenario::Steady, 7, SECS5);
        for _ in 0..2000 {
            let msgs = sim.step();
            assert_eq!(count(&msgs, WsMessageType::DeviceAlert), 0);
            assert_eq!(count(&msgs, WsMessageType::Prediction), 0);
            assert_eq!(count(&msgs, WsMessageType::DeviceStatus), 8);
        }
        assert!(sim
            .devices
            .iter()
            .all(|d| d.status == HealthStatus::Healthy));
    }

    #[test]
    fn same_seed_same_temperatures() {
        let mut a = EdgeSim::new(0, "e", 4, Scenario::Steady, 99, SECS5);
        let mut b = EdgeSim::new(0, "e", 4, Scenario::Steady, 99, SECS5);
        assert_eq!(temps(&mut a, 2, 50), temps(&mut b, 2, 50));
    }

    #[test]
    fn overheat_ramps_alerts_and_predicts() {
        let mut sim = EdgeSim::new(0, "e", 4, Scenario::Overheat, 3, SECS5);
        let mut alerts = Vec::new();
        let mut predictions = Vec::new();
        let mut series = Vec::new();
        for _ in 0..60 {
            for m in sim.step() {
                match m.msg_type {
                    WsMessageType::DeviceAlert => alerts.push(m.payload::<DeviceAlert>().unwrap()),
                    WsMessageType::Prediction => {
                        predictions.push(m.payload::<PredictionResult>().unwrap())
                    }
                    _ => {}
                }
            }
            series.push(sim.devices[0].temperature);
        }
        // Monotone-ish rise, then plateau near critical + 5
        assert!(series[20] > series[5] + 10.0);
        assert!((series[59] - 80.0).abs() < 0.5);
        assert_eq!(sim.devices[0].status, HealthStatus::Error);
        // exactly one warning then one critical alert, both on device 0
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0].severity, Severity::Warning);
        assert_eq!(alerts[1].severity, Severity::Critical);
        assert!(alerts.iter().all(|a| a.device_id == sim.devices[0].id));
        // predictions: sane, shrinking ETA, rising probability
        assert!(predictions.len() >= 5);
        let etas: Vec<i32> = predictions.iter().map(|p| p.eta_minutes.unwrap()).collect();
        assert!(etas.windows(2).all(|w| w[1] <= w[0]));
        assert!(etas[0] <= 3 && *etas.last().unwrap() >= 0);
        assert!(predictions
            .windows(2)
            .all(|w| w[1].probability >= w[0].probability));
        assert!(predictions
            .iter()
            .all(|p| p.prediction_type == PredictionType::Overheating
                && (0.0..=1.0).contains(&p.probability)));
        // other devices stay healthy
        assert!(sim.devices[1..]
            .iter()
            .all(|d| d.status == HealthStatus::Healthy));
    }

    #[test]
    fn overheat_only_on_first_edge() {
        let mut sim = EdgeSim::new(1, "e", 2, Scenario::Overheat, 3, SECS5);
        for _ in 0..60 {
            assert_eq!(count(&sim.step(), WsMessageType::DeviceAlert), 0);
        }
    }

    #[test]
    fn flap_oscillates_around_warning() {
        let mut sim = EdgeSim::new(0, "e", 1, Scenario::Flap, 5, SECS5);
        let mut warn_alerts = 0;
        let mut statuses = Vec::new();
        for _ in 0..40 {
            warn_alerts += count(&sim.step(), WsMessageType::DeviceAlert);
            statuses.push(sim.devices[0].status);
        }
        assert!(statuses.contains(&HealthStatus::Warning));
        assert!(statuses[5..].contains(&HealthStatus::Healthy));
        assert!(!statuses.contains(&HealthStatus::Error));
        // one warning alert per upward crossing: ~40/6 cycles
        assert!((5..=8).contains(&warn_alerts), "alerts={warn_alerts}");
    }

    #[test]
    fn flap_follows_threshold_updates() {
        let mut sim = EdgeSim::new(0, "e", 1, Scenario::Flap, 5, SECS5);
        sim.apply_config(&ConfigUpdate {
            poll_interval_secs: None,
            thresholds: ThresholdConfig {
                temperature_warning: Some(50.0),
                temperature_critical: None,
            },
        });
        let series = temps(&mut sim, 0, 30);
        assert!(series[WARMUP_TICKS as usize..]
            .iter()
            .all(|t| (t - 50.0).abs() <= FLAP_AMPLITUDE_C + 0.2));
    }

    #[test]
    fn offline_goes_silent_then_removed() {
        let mut sim = EdgeSim::new(0, "e", 3, Scenario::Offline, 1, SECS5);
        let target = sim.devices[0].id.clone();
        let mut removed_at = None;
        let mut last_status_tick = None;
        for tick in 0..30u64 {
            for m in sim.step() {
                match m.msg_type {
                    WsMessageType::DeviceStatus => {
                        let s: DeviceStatusUpdate = m.payload().unwrap();
                        if s.device_id == target {
                            last_status_tick = Some(tick);
                        }
                    }
                    WsMessageType::DeviceRemoved => {
                        let r: DeviceRemoved = m.payload().unwrap();
                        assert_eq!(r.device_id, target);
                        assert!(removed_at.is_none(), "removed twice");
                        removed_at = Some(tick);
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(last_status_tick, Some(OFFLINE_AFTER_TICKS - 1));
        assert_eq!(
            removed_at,
            Some(OFFLINE_AFTER_TICKS + REMOVE_AFTER_SILENT_TICKS)
        );
        assert_eq!(sim.device_count(), 2);
        assert!(!sim.has_device(&target));
        let hb: EdgeHeartbeat = sim.heartbeat(1).payload().unwrap();
        assert_eq!(hb.device_count, 2);
    }

    #[test]
    fn config_update_resolves_partial_thresholds_and_interval() {
        let mut sim = EdgeSim::new(0, "e", 1, Scenario::Steady, 1, SECS5);
        let change = sim.apply_config(&ConfigUpdate {
            poll_interval_secs: Some(2),
            thresholds: ThresholdConfig {
                temperature_warning: None,
                temperature_critical: Some(90.0),
            },
        });
        assert_eq!(change.old_thresholds, (65.0, 75.0));
        assert_eq!(change.new_thresholds, (65.0, 90.0));
        assert!(change.interval_changed());
        assert_eq!(sim.interval, Duration::from_secs(2));
        // poll_interval 0 is ignored
        let change = sim.apply_config(&ConfigUpdate {
            poll_interval_secs: Some(0),
            thresholds: ThresholdConfig::default(),
        });
        assert!(!change.interval_changed());
    }

    #[test]
    fn status_and_heartbeat_payloads() {
        let mut sim = EdgeSim::new(0, "e", 4, Scenario::Steady, 1, SECS5);
        let msgs = sim.step();
        let s: DeviceStatusUpdate = msgs[0].payload().unwrap();
        assert!(s.is_simulated);
        for k in [
            "temperature",
            "is_reachable",
            "self_test_passed",
            "product",
            "slot",
        ] {
            assert!(s.metrics.contains_key(k), "missing {k}");
        }
        let controller: DeviceStatusUpdate = msgs[3].payload().unwrap();
        assert!(controller.metrics.contains_key("mem_free_mb"));

        let hb: EdgeHeartbeat = sim.heartbeat(12).payload().unwrap();
        assert_eq!(hb.status, "Connected");
        assert_eq!(hb.uptime_secs, Some(12));
        assert!(hb.version.is_some());
        assert_eq!(hb.device_count, 4);

        let snap = sim.state_snapshot();
        assert_eq!(snap[0].msg_type, WsMessageType::EdgeRegister);
        assert_eq!(count(&snap, WsMessageType::DeviceStatus), 4);
    }
}
