//! Device manager actor
//!
//! Owns every monitored device and drives all data collection from ONE
//! timer:
//!
//! - **NI-SysCfg sweep**: one shared, reused [`SysCfgSession`] enumerates
//!   all hardware with health in a single pass, on the blocking thread
//!   pool (never on the actor thread). Sweeps never overlap. Results are
//!   matched to devices by identity (alias / serial / resource name), new
//!   devices are added, and devices missing for `removal_sweeps`
//!   consecutive sweeps are removed and reported to the hub.
//! - **NI-VISA discovery** (optional): resource listing without I/O
//!   (`*IDN?` probing only when enabled), driven by the same timer but run
//!   as its own blocking job, so a stuck probe cannot starve SysCfg.
//! - **Watchdog**: a SysCfg sweep or VISA scan that has not returned after
//!   3 of its intervals (at least 60 s) is reported once (warning log,
//!   edge-level alert, degraded heartbeat); recovery is reported when the
//!   job finally returns.
//! - **Simulation**: when NI-SysCfg is unavailable, simulated devices with
//!   a slow random-walk temperature.
//!
//! Device health status is computed with the edge's *current*
//! thresholds. Status updates go through the prediction actor to the hub
//! connector; edge-side alerts (driver/session failures, devices becoming
//! unreachable) go straight to the hub connector.

use actix::prelude::*;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nimon_core::actor::messages::{DeviceAlert, DeviceRemoved, ThresholdConfig};
use nimon_core::actor::DeviceStatusUpdate;
use nimon_core::{Device, DeviceType, HealthStatus, MetricValue, Severity};
use nimon_ni::syscfg::{DeviceHealth, DiscoveredDevice, SysCfgSession, SystemInfo};
use nimon_ni::visa::VisaInstrument;
use tracing::{debug, info, warn};

use super::hub_connector::{HubConnectorActor, UpdateDeviceCount};
use super::prediction_actor::{PredictionActor, PredictionSettings, UpdateThresholds};
use crate::config::{EdgeConfig, VisaSettings};
use crate::devices::{
    classify_health, device_from_discovered, device_id, identity_key, local_from_discovered,
    status_from_health, visa_device, visa_interface, DeviceRegistry, DeviceSource, LocalDevice,
    SimWalk, Thresholds,
};

pub use crate::devices::classify_device;

/// Station memory/disk are read at most this often
const SYSTEM_INFO_INTERVAL: Duration = Duration::from_secs(60);
/// Repeated-failure and offline-device summaries are logged this often
const SUMMARY_INTERVAL: Duration = Duration::from_secs(600);

/// Settings of the device manager
#[derive(Debug, Clone)]
pub struct DeviceManagerSettings {
    /// Sweep interval (seconds)
    pub poll_interval_secs: u64,
    /// Use NI-SysCfg for discovery + health
    pub syscfg_enabled: bool,
    /// Use simulated devices when NI-SysCfg cannot be used
    pub simulate_fallback: bool,
    /// NI-VISA discovery
    pub visa: VisaSettings,
    /// Consecutive missed sweeps before a device is removed
    pub removal_sweeps: u32,
    /// Prediction engine settings (also holds the initial thresholds)
    pub prediction: PredictionSettings,
}

impl Default for DeviceManagerSettings {
    fn default() -> Self {
        Self {
            poll_interval_secs: 10,
            syscfg_enabled: true,
            simulate_fallback: true,
            visa: VisaSettings::default(),
            removal_sweeps: 3,
            prediction: PredictionSettings::default(),
        }
    }
}

impl DeviceManagerSettings {
    /// Settings from the edge YAML. Simulated devices are only used when
    /// NI-SysCfg is enabled but unavailable (development machines).
    pub fn from_config(config: &EdgeConfig) -> Self {
        Self {
            poll_interval_secs: config.api.syscfg.poll_interval_secs.max(1),
            syscfg_enabled: config.api.syscfg.enabled,
            simulate_fallback: config.api.syscfg.enabled,
            visa: config.api.visa.clone(),
            removal_sweeps: config.api.removal_sweeps.max(1),
            prediction: PredictionSettings::from_config(&config.prediction),
        }
    }
}

/// How devices are currently sourced
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Waiting for the first NI-SysCfg sweep
    Pending,
    /// Real hardware via NI-SysCfg
    Hardware,
    /// Simulated devices
    Simulated,
    /// No SysCfg, no simulation (VISA / manual devices only)
    Idle,
}

impl Mode {
    fn as_str(&self) -> &'static str {
        match self {
            Mode::Pending => "pending",
            Mode::Hardware => "hardware",
            Mode::Simulated => "simulated",
            Mode::Idle => "idle",
        }
    }
}

struct TrackedDevice {
    device: Device,
    source: DeviceSource,
    last_status: Option<DeviceStatusUpdate>,
    last_seen_sweep: u64,
    missed_sweeps: u32,
    ever_reachable: bool,
    unreachable_alerted: bool,
    error_total: u64,
    sim: Option<SimWalk>,
}

/// Failure bookkeeping for one data source (log + alert dedupe)
#[derive(Debug, Default)]
struct SourceHealth {
    failures: u32,
    alerted: bool,
    last_summary: Option<Instant>,
}

impl SourceHealth {
    /// Record a failure; returns true when an alert should be raised
    fn on_failure(&mut self, what: &str, error: &str) -> bool {
        self.failures += 1;
        let summary_due = self
            .last_summary
            .is_none_or(|t| t.elapsed() >= SUMMARY_INTERVAL);
        if self.failures == 1 {
            warn!("{what} failed: {error}");
            self.last_summary = Some(Instant::now());
        } else if summary_due {
            info!(
                "{what} still failing ({} consecutive): {error}",
                self.failures
            );
            self.last_summary = Some(Instant::now());
        } else {
            debug!("{what} failed again ({}): {error}", self.failures);
        }
        !std::mem::replace(&mut self.alerted, true)
    }

    /// Record a success; returns true when a recovery should be reported
    fn on_success(&mut self, what: &str) -> bool {
        if self.failures > 0 {
            info!("{what} recovered after {} failure(s)", self.failures);
        }
        self.failures = 0;
        self.last_summary = None;
        std::mem::take(&mut self.alerted)
    }
}

/// Work for one blocking NI-SysCfg sweep
struct SweepJob {
    session: Option<Arc<SysCfgSession>>,
    system_info: bool,
}

enum SysCfgError {
    /// Library missing or session could not be opened
    Unavailable(String),
    /// Session open, enumeration failed
    Sweep(String),
}

struct SweepOutput {
    session: Option<Arc<SysCfgSession>>,
    syscfg: Result<Vec<(DiscoveredDevice, DeviceHealth)>, SysCfgError>,
    system_info: Option<SystemInfo>,
}

/// Runs on the blocking pool: every NI-SysCfg call of a sweep
fn run_sweep_job(job: SweepJob) -> SweepOutput {
    let session = match job.session {
        Some(s) => Ok(s),
        None => SysCfgSession::open()
            .map(Arc::new)
            .map_err(|e| e.to_string()),
    };
    match session {
        Ok(session) => SweepOutput {
            syscfg: session
                .discover_with_health()
                .map_err(|e| SysCfgError::Sweep(e.to_string())),
            system_info: job.system_info.then(|| session.system_info()),
            session: Some(session),
        },
        Err(e) => SweepOutput {
            session: None,
            syscfg: Err(SysCfgError::Unavailable(e)),
            system_info: None,
        },
    }
}

/// A blocking NI job (SysCfg sweep or VISA scan) that may be running.
/// A job that never returns (hung driver call, `*IDN?` probe without a
/// timeout) must not stop monitoring silently: past the stall limit it is
/// reported once, and recovery is reported when it finally completes.
#[derive(Debug, Default)]
struct InFlight {
    started: Option<Instant>,
    stalled: bool,
}

impl InFlight {
    fn is_running(&self) -> bool {
        self.started.is_some()
    }

    fn start(&mut self, now: Instant) {
        self.started = Some(now);
    }

    /// Returns true exactly once when the job exceeds `limit`
    fn check_stalled(&mut self, now: Instant, limit: Duration) -> bool {
        match self.started {
            Some(t) if !self.stalled && now.saturating_duration_since(t) >= limit => {
                self.stalled = true;
                true
            }
            _ => false,
        }
    }

    /// Job completed; returns how long it ran when it had been reported
    /// as stalled
    fn finish(&mut self) -> Option<Duration> {
        let started = self.started.take();
        std::mem::take(&mut self.stalled).then(|| started.map(|t| t.elapsed()).unwrap_or_default())
    }
}

/// A job is stalled after this many of its intervals
const STALL_INTERVALS: u32 = 3;
/// ...but never sooner than this (short intervals, slow enumerations)
const MIN_STALL_LIMIT: Duration = Duration::from_secs(60);

/// List VISA resources; `*IDN?` only when `probe_idn` is set
fn visa_scan(settings: &VisaSettings) -> Result<Vec<VisaInstrument>, String> {
    let api = nimon_ni::visa::NiVisa::load().map_err(|e| e.to_string())?;
    let session = api.create_session().map_err(|e| e.to_string())?;
    let names = session
        .find_resources(&settings.expression)
        .map_err(|e| e.to_string())?;
    Ok(names
        .into_iter()
        .map(|name| {
            if settings.probe_idn {
                session.probe_instrument(&name)
            } else {
                let interface = visa_interface(&name).to_string();
                VisaInstrument::new(name, interface)
            }
        })
        .collect())
}

/// Actor that manages all devices of this edge
pub struct DeviceManagerActor {
    edge_id: String,
    settings: DeviceManagerSettings,
    devices: HashMap<String, TrackedDevice>,
    registry: DeviceRegistry,
    thresholds: Thresholds,
    mode: Mode,
    prediction_actor: Option<Addr<PredictionActor>>,
    hub_connector: Option<Addr<HubConnectorActor>>,
    sweep_handle: Option<SpawnHandle>,
    /// NI-SysCfg sweep and VISA scan run as separate blocking jobs, so a
    /// stuck VISA probe cannot starve SysCfg (and vice versa)
    syscfg_job: InFlight,
    visa_job: InFlight,
    session: Option<Arc<SysCfgSession>>,
    syscfg_health: SourceHealth,
    visa_health: SourceHealth,
    syscfg_seq: u64,
    visa_seq: u64,
    last_visa_scan: Option<Instant>,
    last_system_info: Option<Instant>,
    /// (hostname, metrics) of the station, attached to the host device
    system_metrics: Option<(String, Vec<(&'static str, f64)>)>,
    last_offline_summary: Instant,
    last_reported: Option<(usize, Option<String>)>,
    /// Timer ticks handled (diagnostics)
    ticks: u64,
    // per-sweep scratch buffers (reused, no per-sweep allocation)
    fallback_counts: HashMap<String, u32>,
    key_buf: String,
    id_buf: String,
}

impl DeviceManagerActor {
    pub fn new(edge_id: String) -> Self {
        Self {
            edge_id,
            settings: DeviceManagerSettings::default(),
            devices: HashMap::new(),
            registry: DeviceRegistry::new(),
            thresholds: Thresholds::default(),
            mode: Mode::Pending,
            prediction_actor: None,
            hub_connector: None,
            sweep_handle: None,
            syscfg_job: InFlight::default(),
            visa_job: InFlight::default(),
            session: None,
            syscfg_health: SourceHealth::default(),
            visa_health: SourceHealth::default(),
            syscfg_seq: 0,
            visa_seq: 0,
            last_visa_scan: None,
            last_system_info: None,
            system_metrics: None,
            last_offline_summary: Instant::now(),
            last_reported: None,
            ticks: 0,
            fallback_counts: HashMap::new(),
            key_buf: String::new(),
            id_buf: String::new(),
        }
    }

    /// Full settings (thresholds come from `settings.prediction`)
    pub fn with_settings(mut self, settings: DeviceManagerSettings) -> Self {
        self.thresholds = Thresholds {
            warning: settings.prediction.temperature_warning,
            critical: settings.prediction.temperature_critical,
        };
        self.settings = settings;
        self
    }

    /// Set the sweep interval
    pub fn with_poll_interval(mut self, interval_secs: u64) -> Self {
        self.settings.poll_interval_secs = interval_secs.max(1);
        self
    }

    /// Share the registry of managed devices (used by the action runtime)
    pub fn with_registry(mut self, registry: DeviceRegistry) -> Self {
        self.registry = registry;
        self
    }

    /// Set the hub connector for status, alerts and removals
    pub fn with_hub_connector(mut self, addr: Addr<HubConnectorActor>) -> Self {
        self.hub_connector = Some(addr);
        self
    }

    /// Get count of managed devices
    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    fn start_prediction_actor(&mut self) {
        let mut settings = self.settings.prediction.clone();
        settings.temperature_warning = self.thresholds.warning;
        settings.temperature_critical = self.thresholds.critical;
        let mut actor = PredictionActor::new(settings);
        if let Some(ref hub) = self.hub_connector {
            actor = actor.with_hub_connector(hub.clone());
        }
        self.prediction_actor = Some(actor.start());
    }

    // ------------------------------------------------------------------
    // Timer
    // ------------------------------------------------------------------

    fn tick_interval(&self) -> Option<Duration> {
        let base = match self.mode {
            Mode::Pending | Mode::Hardware | Mode::Simulated => self.settings.poll_interval_secs,
            Mode::Idle if self.settings.visa.enabled => self.settings.visa.poll_interval_secs,
            Mode::Idle => return None,
        };
        Some(Duration::from_secs(base.max(1)))
    }

    /// (Re-)arm the single sweep timer with the current interval
    fn arm_timer(&mut self, ctx: &mut Context<Self>) {
        if let Some(handle) = self.sweep_handle.take() {
            ctx.cancel_future(handle);
        }
        if let Some(interval) = self.tick_interval() {
            self.sweep_handle = Some(ctx.run_interval(interval, |act, ctx| {
                act.tick(ctx);
            }));
        }
    }

    fn visa_due(&self) -> bool {
        self.settings.visa.enabled
            && self.last_visa_scan.is_none_or(|t| {
                t.elapsed() + Duration::from_millis(500)
                    >= Duration::from_secs(self.settings.visa.poll_interval_secs.max(1))
            })
    }

    fn stall_limit(interval_secs: u64) -> Duration {
        (Duration::from_secs(interval_secs.max(1)) * STALL_INTERVALS).max(MIN_STALL_LIMIT)
    }

    /// Watchdog for blocking NI jobs that never return: warn + one
    /// edge-level alert + degraded heartbeat. Returns true when a new stall
    /// was detected.
    fn check_stalls(&mut self, now: Instant) -> bool {
        let mut changed = false;
        let limit = Self::stall_limit(self.settings.poll_interval_secs);
        if self.syscfg_job.check_stalled(now, limit) {
            warn!(
                "NI-SysCfg sweep has not returned for {}s; device data is stale",
                limit.as_secs()
            );
            self.edge_alert(
                Severity::Warning,
                format!(
                    "NI-SysCfg sweep hung: no result for {}s (driver call not returning); \
                     device status is stale",
                    limit.as_secs()
                ),
            );
            changed = true;
        }
        let limit = Self::stall_limit(self.settings.visa.poll_interval_secs);
        if self.visa_job.check_stalled(now, limit) {
            warn!(
                "NI-VISA discovery has not returned for {}s",
                limit.as_secs()
            );
            self.edge_alert(
                Severity::Warning,
                format!(
                    "NI-VISA discovery hung: no result for {}s (resource listing or *IDN? probe \
                     not returning)",
                    limit.as_secs()
                ),
            );
            changed = true;
        }
        if changed {
            self.notify_edge_status();
        }
        changed
    }

    /// One timer tick: simulate, and/or start the blocking NI jobs that are
    /// due and not already running. Returns true when a job was started.
    fn tick(&mut self, ctx: &mut Context<Self>) -> bool {
        self.ticks += 1;
        let now = Instant::now();
        self.check_stalls(now);
        if self.mode == Mode::Simulated {
            self.simulate_tick();
        }
        let (syscfg, visa) = self.jobs_to_start();
        if syscfg {
            self.start_syscfg_job(now, ctx);
        }
        if visa {
            self.start_visa_job(now, ctx);
        }
        if !(syscfg || visa) {
            self.maybe_log_offline_summary();
        }
        syscfg || visa
    }

    /// (start a SysCfg sweep, start a VISA scan) for this tick. Each job
    /// only waits for its own previous run.
    fn jobs_to_start(&self) -> (bool, bool) {
        let want_syscfg = matches!(self.mode, Mode::Pending | Mode::Hardware);
        if want_syscfg && self.syscfg_job.is_running() {
            debug!("Previous NI-SysCfg sweep still running; skipping this tick");
        }
        let visa_due = self.visa_due();
        if visa_due && self.visa_job.is_running() {
            debug!("Previous NI-VISA scan still running; skipping");
        }
        (
            want_syscfg && !self.syscfg_job.is_running(),
            visa_due && !self.visa_job.is_running(),
        )
    }

    fn start_syscfg_job(&mut self, now: Instant, ctx: &mut Context<Self>) {
        let system_info = self
            .last_system_info
            .is_none_or(|t| t.elapsed() >= SYSTEM_INFO_INTERVAL);
        if system_info {
            self.last_system_info = Some(now);
        }
        let job = SweepJob {
            session: self.session.clone(),
            system_info,
        };
        self.syscfg_job.start(now);
        let fut = actix_rt::task::spawn_blocking(move || run_sweep_job(job));
        ctx.spawn(fut.into_actor(self).map(|res, act, ctx| {
            act.finish_syscfg_job();
            match res {
                Ok(out) => act.apply_sweep(out, ctx),
                Err(e) => warn!("NI-SysCfg sweep task failed: {e}"),
            }
            act.maybe_log_offline_summary();
            act.notify_edge_status();
        }));
    }

    fn start_visa_job(&mut self, now: Instant, ctx: &mut Context<Self>) {
        self.last_visa_scan = Some(now);
        let settings = self.settings.visa.clone();
        self.visa_job.start(now);
        let fut = actix_rt::task::spawn_blocking(move || visa_scan(&settings));
        ctx.spawn(fut.into_actor(self).map(|res, act, _ctx| {
            act.finish_visa_job();
            match res {
                Ok(out) => act.apply_visa_result(out),
                Err(e) => warn!("NI-VISA scan task failed: {e}"),
            }
            act.maybe_log_offline_summary();
            act.notify_edge_status();
        }));
    }

    fn finish_syscfg_job(&mut self) {
        if let Some(took) = self.syscfg_job.finish() {
            info!(
                "NI-SysCfg sweep returned after {}s; monitoring resumed",
                took.as_secs()
            );
            self.edge_alert(
                Severity::Info,
                format!(
                    "NI-SysCfg sweep recovered (a sweep took {}s)",
                    took.as_secs()
                ),
            );
        }
    }

    fn finish_visa_job(&mut self) {
        if let Some(took) = self.visa_job.finish() {
            info!("NI-VISA discovery returned after {}s", took.as_secs());
            self.edge_alert(
                Severity::Info,
                format!(
                    "NI-VISA discovery recovered (a scan took {}s)",
                    took.as_secs()
                ),
            );
        }
    }

    fn apply_visa_result(&mut self, result: Result<Vec<VisaInstrument>, String>) {
        match result {
            Ok(instruments) => {
                if self.visa_health.on_success("NI-VISA discovery") {
                    self.edge_alert(Severity::Info, "NI-VISA discovery recovered".to_string());
                }
                self.apply_visa(instruments);
            }
            Err(e) => {
                if self.visa_health.on_failure("NI-VISA discovery", &e) {
                    self.edge_alert(
                        Severity::Warning,
                        format!("NI-VISA driver/discovery failing: {e}"),
                    );
                }
            }
        }
    }

    fn apply_sweep(&mut self, out: SweepOutput, ctx: &mut Context<Self>) {
        if let Some(session) = out.session {
            self.session = Some(session);
        }
        if let Some(info) = out.system_info {
            self.system_metrics = system_metrics(&info);
        }

        match out.syscfg {
            Ok(items) => {
                if self.mode == Mode::Pending {
                    info!(
                        "NI-SysCfg sweep mode: {} device(s), one shared enumeration every {}s",
                        items.len(),
                        self.settings.poll_interval_secs
                    );
                    self.mode = Mode::Hardware;
                }
                if self.syscfg_health.on_success("NI-SysCfg sweep") {
                    self.edge_alert(Severity::Info, "NI-SysCfg sweep recovered".to_string());
                }
                self.apply_syscfg(items);
            }
            Err(SysCfgError::Unavailable(e)) => {
                self.session = None;
                if self.mode == Mode::Pending && self.settings.simulate_fallback {
                    info!("NI-SysCfg unavailable ({e}); using simulated devices");
                    self.syscfg_health.failures = 1;
                    self.syscfg_health.alerted = true;
                    self.edge_alert(
                        Severity::Warning,
                        format!("NI-SysCfg driver unavailable: {e} (simulated devices in use)"),
                    );
                    self.mode = Mode::Simulated;
                    self.add_simulated_devices();
                    self.arm_timer(ctx);
                } else if self.syscfg_health.on_failure("NI-SysCfg session", &e) {
                    self.edge_alert(
                        Severity::Warning,
                        format!("NI-SysCfg driver/session unavailable: {e}"),
                    );
                }
            }
            Err(SysCfgError::Sweep(e)) => {
                if self.syscfg_health.on_failure("NI-SysCfg enumeration", &e) {
                    self.edge_alert(
                        Severity::Warning,
                        format!("NI-SysCfg enumeration failing: {e}"),
                    );
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Source handling
    // ------------------------------------------------------------------

    fn is_host(&self, d: &DiscoveredDevice) -> bool {
        match (&self.system_metrics, d.alias.as_deref()) {
            (Some((hostname, _)), Some(alias)) => alias.eq_ignore_ascii_case(hostname),
            _ => false,
        }
    }

    fn apply_syscfg(&mut self, items: Vec<(DiscoveredDevice, DeviceHealth)>) {
        self.syscfg_seq += 1;
        let seq = self.syscfg_seq;
        let mut counts = std::mem::take(&mut self.fallback_counts);
        let mut key = std::mem::take(&mut self.key_buf);
        let mut id = std::mem::take(&mut self.id_buf);
        counts.clear();

        for (d, mut health) in items {
            identity_key(&d, &mut counts, &mut key);
            device_id(&self.edge_id, &key, &mut id);

            // station memory/disk ride along on the host resource
            if self.is_host(&d) {
                if let Some((_, metrics)) = &self.system_metrics {
                    for (name, value) in metrics {
                        health
                            .metrics
                            .insert((*name).to_string(), MetricValue::Float(*value));
                    }
                }
            }

            if !self.devices.contains_key(id.as_str()) {
                let device = device_from_discovered(&self.edge_id, &id, &d);
                self.add_tracked(device, DeviceSource::SysCfg, local_from_discovered(&d));
            }
            if let Some(t) = self.devices.get_mut(id.as_str()) {
                t.last_seen_sweep = seq;
                t.missed_sweeps = 0;
            }
            let (status, metrics) = status_from_health(health, self.thresholds);
            self.record_status(&id, status, metrics);
        }

        self.fallback_counts = counts;
        self.key_buf = key;
        self.id_buf = id;
        self.expire_missing(DeviceSource::SysCfg, seq);
    }

    fn apply_visa(&mut self, instruments: Vec<VisaInstrument>) {
        self.visa_seq += 1;
        let seq = self.visa_seq;
        let probed = self.settings.visa.probe_idn;
        let mut id = std::mem::take(&mut self.id_buf);
        for instr in instruments {
            device_id(&self.edge_id, &instr.resource_name, &mut id);
            if !self.devices.contains_key(id.as_str()) {
                let device = visa_device(
                    &self.edge_id,
                    &id,
                    &instr.resource_name,
                    instr.description.as_deref(),
                );
                self.add_tracked(
                    device,
                    DeviceSource::Visa,
                    LocalDevice {
                        daqmx_name: None,
                        is_simulated: false,
                        source: DeviceSource::Visa,
                    },
                );
            }
            if let Some(t) = self.devices.get_mut(id.as_str()) {
                t.last_seen_sweep = seq;
                t.missed_sweeps = 0;
            }
            let mut metrics = HashMap::new();
            metrics.insert(
                "interface".to_string(),
                MetricValue::String(instr.interface_type),
            );
            metrics.insert("probed".to_string(), MetricValue::Boolean(probed));
            let status = if probed {
                metrics.insert(
                    "is_reachable".to_string(),
                    MetricValue::Boolean(instr.is_reachable),
                );
                if let Some(ms) = instr.response_time_ms {
                    metrics.insert("response_time_ms".to_string(), MetricValue::Float(ms));
                }
                if instr.is_reachable {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Offline
                }
            } else {
                // listed by the resource manager; not contacted
                HealthStatus::Healthy
            };
            self.record_status(&id, status, metrics);
        }
        self.id_buf = id;
        self.expire_missing(DeviceSource::Visa, seq);
    }

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

    fn add_simulated_devices(&mut self) {
        for device in self.simulated_devices() {
            if !self.devices.contains_key(&device.id) {
                self.add_tracked(
                    device,
                    DeviceSource::Simulated,
                    LocalDevice {
                        daqmx_name: None,
                        is_simulated: true,
                        source: DeviceSource::Simulated,
                    },
                );
            }
        }
    }

    fn simulate_tick(&mut self) {
        let ids: Vec<String> = self
            .devices
            .iter()
            .filter(|(_, t)| t.sim.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let Some(t) = self.devices.get_mut(&id) else {
                continue;
            };
            let Some(walk) = t.sim.as_mut() else {
                continue;
            };
            let temperature = walk.next_value();
            let mut metrics = HashMap::new();
            metrics.insert("temperature".to_string(), MetricValue::Float(temperature));
            let status = classify_health(true, false, false, Some(temperature), self.thresholds);
            self.record_status(&id, status, metrics);
        }
    }

    // ------------------------------------------------------------------
    // Device bookkeeping
    // ------------------------------------------------------------------

    fn add_tracked(&mut self, device: Device, source: DeviceSource, local: LocalDevice) {
        let id = device.id.clone();
        let sim = match source {
            DeviceSource::Simulated => Some(SimWalk::seeded(&id)),
            DeviceSource::Manual if device.is_simulated => Some(SimWalk::seeded(&id)),
            _ => None,
        };
        info!(
            "Added device {} ({}, {:?}, {:?})",
            id,
            device.model.as_deref().unwrap_or(&device.device_name),
            device.device_type,
            source
        );
        self.registry.insert(id.clone(), local);
        self.devices.insert(
            id,
            TrackedDevice {
                device,
                source,
                last_status: None,
                last_seen_sweep: 0,
                missed_sweeps: 0,
                ever_reachable: false,
                unreachable_alerted: false,
                error_total: 0,
                sim,
            },
        );
    }

    /// Remove a device; the hub is told via `device_removed` and its
    /// prediction state is dropped
    fn remove_tracked(&mut self, device_id: &str) -> bool {
        if self.devices.remove(device_id).is_none() {
            return false;
        }
        self.registry.remove(device_id);
        let removed = DeviceRemoved {
            edge_id: self.edge_id.clone(),
            device_id: device_id.to_string(),
            timestamp: Utc::now(),
        };
        // through the prediction actor: drops state, keeps ordering
        if let Some(ref prediction) = self.prediction_actor {
            prediction.do_send(removed);
        } else if let Some(ref hub) = self.hub_connector {
            hub.do_send(removed);
        }
        info!("Removed device {}", device_id);
        true
    }

    fn expire_missing(&mut self, source: DeviceSource, seq: u64) {
        let limit = self.settings.removal_sweeps.max(1);
        let mut gone = Vec::new();
        for (id, t) in self.devices.iter_mut() {
            if t.source == source && t.last_seen_sweep != seq {
                t.missed_sweeps += 1;
                if t.missed_sweeps >= limit {
                    gone.push(id.clone());
                } else {
                    debug!(
                        "Device {} missing from discovery ({}/{})",
                        id, t.missed_sweeps, limit
                    );
                }
            }
        }
        for id in gone {
            info!("Device {} missing for {} sweeps", id, limit);
            self.remove_tracked(&id);
        }
    }

    /// Finish a status (identity metrics, error count, sanitize), raise
    /// reachability alerts, cache it and send it downstream
    fn record_status(
        &mut self,
        id: &str,
        status: HealthStatus,
        mut metrics: HashMap<String, MetricValue>,
    ) {
        let Some(t) = self.devices.get_mut(id) else {
            return;
        };
        if let Some(slot) = t.device.slot {
            metrics.insert("slot".to_string(), MetricValue::Integer(slot as i64));
        }
        if let Some(ref model) = t.device.model {
            metrics.insert("product".to_string(), MetricValue::String(model.clone()));
        }
        if matches!(status, HealthStatus::Offline | HealthStatus::Error) {
            t.error_total += 1;
        }
        metrics.insert(
            "error_count".to_string(),
            MetricValue::Integer(t.error_total.min(i64::MAX as u64) as i64),
        );
        nimon_core::sanitize_metrics(&mut metrics);

        let mut alert = None;
        if status == HealthStatus::Offline {
            if t.ever_reachable && !t.unreachable_alerted {
                t.unreachable_alerted = true;
                warn!("Device {} became unreachable", id);
                alert = Some((
                    Severity::Warning,
                    format!(
                        "Device {} ({}) became unreachable",
                        t.device.device_name, id
                    ),
                ));
            } else {
                debug!("Device {} unreachable", id);
            }
        } else {
            if t.unreachable_alerted {
                t.unreachable_alerted = false;
                info!("Device {} is reachable again", id);
                alert = Some((
                    Severity::Info,
                    format!(
                        "Device {} ({}) is reachable again",
                        t.device.device_name, id
                    ),
                ));
            }
            t.ever_reachable = true;
        }

        let update = DeviceStatusUpdate {
            device_id: id.to_string(),
            edge_id: self.edge_id.clone(),
            status,
            metrics,
            timestamp: Utc::now(),
            is_simulated: t.device.is_simulated,
        };
        t.last_status = Some(update.clone());

        if let Some((severity, message)) = alert {
            self.alert(id, severity, message, None);
        }
        if let Some(ref prediction) = self.prediction_actor {
            prediction.do_send(update);
        } else if let Some(ref hub) = self.hub_connector {
            hub.do_send(update);
        }
    }

    fn maybe_log_offline_summary(&mut self) {
        if self.last_offline_summary.elapsed() < SUMMARY_INTERVAL {
            return;
        }
        self.last_offline_summary = Instant::now();
        let offline: Vec<&str> = self
            .devices
            .iter()
            .filter(|(_, t)| {
                t.last_status
                    .as_ref()
                    .is_some_and(|s| s.status == HealthStatus::Offline)
            })
            .map(|(id, _)| id.as_str())
            .collect();
        if !offline.is_empty() {
            info!(
                "{} of {} device(s) offline: {}",
                offline.len(),
                self.devices.len(),
                offline.join(", ")
            );
        }
    }

    // ------------------------------------------------------------------
    // Alerts and edge status
    // ------------------------------------------------------------------

    fn alert(&self, device_id: &str, severity: Severity, message: String, metric: Option<&str>) {
        if let Some(ref hub) = self.hub_connector {
            hub.do_send(DeviceAlert {
                device_id: device_id.to_string(),
                edge_id: self.edge_id.clone(),
                severity,
                message,
                metric_name: metric.map(str::to_string),
                metric_value: None,
                timestamp: Utc::now(),
            });
        }
    }

    /// Edge-scoped alert (device_id == edge id)
    fn edge_alert(&self, severity: Severity, message: String) {
        self.alert(&self.edge_id, severity, message, None);
    }

    fn degraded_reason(&self) -> Option<String> {
        let mut reasons = Vec::new();
        if self.syscfg_job.stalled {
            reasons.push("NI-SysCfg sweep hung");
        } else if self.mode == Mode::Simulated {
            reasons.push("NI-SysCfg unavailable, simulated devices");
        } else if self.syscfg_health.failures > 0 {
            reasons.push("NI-SysCfg failing");
        }
        if self.visa_job.stalled {
            reasons.push("NI-VISA discovery hung");
        } else if self.visa_health.failures > 0 {
            reasons.push("NI-VISA failing");
        }
        (!reasons.is_empty()).then(|| reasons.join("; "))
    }

    fn notify_edge_status(&mut self) {
        let current = (self.devices.len(), self.degraded_reason());
        if self.last_reported.as_ref() == Some(&current) {
            return;
        }
        if let Some(ref hub) = self.hub_connector {
            hub.do_send(UpdateDeviceCount {
                count: current.0,
                degraded: current.1.clone(),
            });
        }
        self.last_reported = Some(current);
    }
}

/// Station metrics (MB) from NI-SysCfg system info, keyed by hostname
fn system_metrics(info: &SystemInfo) -> Option<(String, Vec<(&'static str, f64)>)> {
    let hostname = info.hostname.clone()?;
    let metrics = [
        ("mem_total_mb", info.memory_total_mb),
        ("mem_free_mb", info.memory_free_mb),
        ("disk_total_mb", info.disk_total_mb),
        ("disk_free_mb", info.disk_free_mb),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.filter(|v| v.is_finite()).map(|v| (k, v)))
    .collect();
    Some((hostname, metrics))
}

impl Actor for DeviceManagerActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("DeviceManagerActor started for edge {}", self.edge_id);
        self.start_prediction_actor();

        self.mode = if self.settings.syscfg_enabled {
            Mode::Pending
        } else if self.settings.simulate_fallback {
            Mode::Simulated
        } else {
            Mode::Idle
        };
        if self.mode == Mode::Simulated {
            info!("NI-SysCfg disabled; using simulated devices");
            self.add_simulated_devices();
        }
        self.arm_timer(ctx);
        // first sweep right away (off the actor thread)
        self.tick(ctx);
        self.notify_edge_status();
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        info!("DeviceManagerActor stopped");
    }
}

// ============================================================================
// Messages
// ============================================================================

/// Message to get list of device ids
#[derive(Message)]
#[rtype(result = "Vec<String>")]
pub struct ListDevices;

impl Handler<ListDevices> for DeviceManagerActor {
    type Result = Vec<String>;

    fn handle(&mut self, _msg: ListDevices, _ctx: &mut Self::Context) -> Self::Result {
        self.devices.keys().cloned().collect()
    }
}

/// Trigger an immediate sweep (skipped when one is already running).
/// Returns true when a blocking NI sweep was started.
#[derive(Message)]
#[rtype(result = "bool")]
pub struct PollAllDevices;

impl Handler<PollAllDevices> for DeviceManagerActor {
    type Result = bool;

    fn handle(&mut self, _msg: PollAllDevices, ctx: &mut Self::Context) -> Self::Result {
        self.tick(ctx)
    }
}

/// Message to add a device manually (never auto-removed; simulated
/// devices get simulated readings)
#[derive(Message)]
#[rtype(result = "()")]
pub struct AddDevice {
    pub device: Device,
}

impl Handler<AddDevice> for DeviceManagerActor {
    type Result = ();

    fn handle(&mut self, msg: AddDevice, _ctx: &mut Self::Context) -> Self::Result {
        if self.devices.contains_key(&msg.device.id) {
            return;
        }
        let local = LocalDevice {
            daqmx_name: None,
            is_simulated: msg.device.is_simulated,
            source: DeviceSource::Manual,
        };
        self.add_tracked(msg.device, DeviceSource::Manual, local);
        self.notify_edge_status();
    }
}

/// Message to remove a device (reported to the hub as `device_removed`)
#[derive(Message)]
#[rtype(result = "bool")]
pub struct RemoveDevice {
    pub device_id: String,
}

impl Handler<RemoveDevice> for DeviceManagerActor {
    type Result = bool;

    fn handle(&mut self, msg: RemoveDevice, _ctx: &mut Self::Context) -> Self::Result {
        let removed = self.remove_tracked(&msg.device_id);
        self.notify_edge_status();
        removed
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

/// Desired-state config (hub push or local reload). `None` / empty
/// thresholds mean "no change". Thresholds are applied in place; a new
/// poll interval re-arms the sweep timer immediately.
#[derive(Message, Debug, Clone, Default)]
#[rtype(result = "()")]
pub struct ApplyConfig {
    pub poll_interval_secs: Option<u64>,
    pub thresholds: ThresholdConfig,
}

impl Handler<ApplyConfig> for DeviceManagerActor {
    type Result = ();

    fn handle(&mut self, msg: ApplyConfig, ctx: &mut Self::Context) -> Self::Result {
        if let Some(secs) = msg.poll_interval_secs {
            let secs = secs.max(1);
            if secs != self.settings.poll_interval_secs {
                info!(
                    "Sweep interval {}s -> {}s",
                    self.settings.poll_interval_secs, secs
                );
                self.settings.poll_interval_secs = secs;
                self.arm_timer(ctx);
            }
        }

        if !msg.thresholds.is_empty() {
            let (warning, critical) = msg
                .thresholds
                .resolve(self.thresholds.warning, self.thresholds.critical);
            if !(warning.is_finite() && critical.is_finite()) || warning >= critical {
                warn!(
                    "Ignoring thresholds warning={warning} critical={critical}: warning must be below critical"
                );
            } else if (warning, critical) != (self.thresholds.warning, self.thresholds.critical) {
                info!(
                    "Temperature thresholds {:.1}/{:.1} -> {:.1}/{:.1} C",
                    self.thresholds.warning, self.thresholds.critical, warning, critical
                );
                self.thresholds = Thresholds { warning, critical };
                if let Some(ref prediction) = self.prediction_actor {
                    prediction.do_send(UpdateThresholds { warning, critical });
                }
            } else {
                debug!("Thresholds unchanged ({warning}/{critical})");
            }
        }
    }
}

/// Re-send the latest status of every device straight to the hub
/// (hub `resend_state` command). Returns the number of snapshots sent.
#[derive(Message)]
#[rtype(result = "usize")]
pub struct ResendState;

impl Handler<ResendState> for DeviceManagerActor {
    type Result = usize;

    fn handle(&mut self, _msg: ResendState, _ctx: &mut Self::Context) -> Self::Result {
        let Some(ref hub) = self.hub_connector else {
            return 0;
        };
        let mut sent = 0;
        for t in self.devices.values() {
            if let Some(ref update) = t.last_status {
                hub.do_send(update.clone());
                sent += 1;
            }
        }
        // the connector re-learns count/degraded state too
        hub.do_send(UpdateDeviceCount {
            count: self.devices.len(),
            degraded: self.degraded_reason(),
        });
        sent
    }
}

/// Stop the sweep timer and the actor (graceful shutdown)
#[derive(Message)]
#[rtype(result = "()")]
pub struct StopDeviceManager;

impl Handler<StopDeviceManager> for DeviceManagerActor {
    type Result = ();

    fn handle(&mut self, _msg: StopDeviceManager, ctx: &mut Self::Context) -> Self::Result {
        if let Some(handle) = self.sweep_handle.take() {
            ctx.cancel_future(handle);
        }
        ctx.stop();
    }
}

/// Snapshot of the manager (diagnostics / tests)
#[derive(Debug, Clone, PartialEq)]
pub struct ManagerSnapshot {
    pub device_count: usize,
    pub poll_interval_secs: u64,
    pub temperature_warning: f64,
    pub temperature_critical: f64,
    pub mode: &'static str,
    pub timer_armed: bool,
    pub degraded: Option<String>,
    pub ticks: u64,
}

/// Get a [`ManagerSnapshot`]
#[derive(Message)]
#[rtype(result = "ManagerSnapshot")]
pub struct GetManagerSnapshot;

impl Handler<GetManagerSnapshot> for DeviceManagerActor {
    type Result = MessageResult<GetManagerSnapshot>;

    fn handle(&mut self, _msg: GetManagerSnapshot, _ctx: &mut Self::Context) -> Self::Result {
        MessageResult(ManagerSnapshot {
            device_count: self.devices.len(),
            poll_interval_secs: self.settings.poll_interval_secs,
            temperature_warning: self.thresholds.warning,
            temperature_critical: self.thresholds.critical,
            mode: self.mode.as_str(),
            timer_armed: self.sweep_handle.is_some(),
            degraded: self.degraded_reason(),
            ticks: self.ticks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimon_ni::syscfg::DeviceHealth;

    /// Simulation only: never touches NI drivers
    fn sim_settings() -> DeviceManagerSettings {
        DeviceManagerSettings {
            syscfg_enabled: false,
            simulate_fallback: true,
            ..DeviceManagerSettings::default()
        }
    }

    fn sim_manager() -> DeviceManagerActor {
        DeviceManagerActor::new("test-edge".to_string()).with_settings(sim_settings())
    }

    fn discovered(product: &str, serial: &str, alias: Option<&str>) -> DiscoveredDevice {
        let mut d = DiscoveredDevice::new(product.to_string(), serial.to_string());
        d.alias = alias.map(str::to_string);
        d
    }

    fn healthy(temp: f64) -> DeviceHealth {
        DeviceHealth {
            is_reachable: true,
            temperature: Some(temp),
            sensors: Vec::new(),
            self_test_passed: None,
            error_message: None,
            metrics: HashMap::new(),
        }
    }

    #[test]
    fn test_settings_from_config() {
        let mut cfg = EdgeConfig::default();
        cfg.api.syscfg.poll_interval_secs = 30;
        cfg.prediction.temperature_warning = 55.0;
        cfg.prediction.temperature_critical = 60.0;
        let s = DeviceManagerSettings::from_config(&cfg);
        assert_eq!(s.poll_interval_secs, 30);
        assert!(s.syscfg_enabled && s.simulate_fallback);
        let m = DeviceManagerActor::new("e".into()).with_settings(s);
        assert_eq!(
            m.thresholds,
            Thresholds {
                warning: 55.0,
                critical: 60.0
            }
        );
        cfg.api.syscfg.enabled = false;
        let s = DeviceManagerSettings::from_config(&cfg);
        assert!(!s.syscfg_enabled && !s.simulate_fallback);
    }

    #[test]
    fn test_sweep_matching_by_identity_not_position() {
        let mut m = DeviceManagerActor::new("e".into());
        m.apply_syscfg(vec![
            (discovered("NI 9205", "S1", Some("ModA")), healthy(40.0)),
            (discovered("NI 9205", "S2", Some("ModB")), healthy(70.0)),
        ]);
        assert_eq!(m.devices.len(), 2);
        // reversed enumeration order: temperatures must follow the device
        m.apply_syscfg(vec![
            (discovered("NI 9205", "S2", Some("ModB")), healthy(71.0)),
            (discovered("NI 9205", "S1", Some("ModA")), healthy(41.0)),
        ]);
        let temp = |id: &str| {
            m.devices[id].last_status.as_ref().unwrap().metrics["temperature"].as_f64_finite()
        };
        assert_eq!(temp("e:ModA"), Some(41.0));
        assert_eq!(temp("e:ModB"), Some(71.0));
        assert_eq!(
            m.devices["e:ModB"].last_status.as_ref().unwrap().status,
            HealthStatus::Warning
        );
        // registry knows the DAQmx names
        assert_eq!(
            m.registry.get("e:ModA").unwrap().daqmx_name.as_deref(),
            Some("ModA")
        );
    }

    #[test]
    fn test_device_removed_after_n_missed_sweeps_and_readded() {
        let mut m = DeviceManagerActor::new("e".into());
        m.settings.removal_sweeps = 3;
        let a = || (discovered("NI 9205", "S1", Some("ModA")), healthy(40.0));
        let b = || (discovered("NI 9205", "S2", Some("ModB")), healthy(40.0));
        m.apply_syscfg(vec![a(), b()]);
        m.apply_syscfg(vec![a()]);
        m.apply_syscfg(vec![a()]);
        assert!(
            m.devices.contains_key("e:ModB"),
            "not removed before 3 misses"
        );
        // flapping back resets the counter
        m.apply_syscfg(vec![a(), b()]);
        assert_eq!(m.devices["e:ModB"].missed_sweeps, 0);
        for _ in 0..3 {
            m.apply_syscfg(vec![a()]);
        }
        assert!(!m.devices.contains_key("e:ModB"));
        assert!(m.registry.get("e:ModB").is_none());
        m.apply_syscfg(vec![a(), b()]);
        assert!(m.devices.contains_key("e:ModB"));
    }

    #[test]
    fn test_status_uses_current_thresholds_and_counts_errors() {
        let mut m = DeviceManagerActor::new("e".into());
        m.thresholds = Thresholds {
            warning: 80.0,
            critical: 90.0,
        };
        m.apply_syscfg(vec![(discovered("X", "S1", None), healthy(70.0))]);
        let st = m.devices["e:S1"].last_status.clone().unwrap();
        assert_eq!(st.status, HealthStatus::Healthy);
        assert_eq!(st.metrics["error_count"], MetricValue::Integer(0));

        m.apply_syscfg(vec![(
            discovered("X", "S1", None),
            DeviceHealth::unreachable(),
        )]);
        m.apply_syscfg(vec![(
            discovered("X", "S1", None),
            DeviceHealth::unreachable(),
        )]);
        let t = &m.devices["e:S1"];
        assert_eq!(t.error_total, 2);
        assert!(
            t.unreachable_alerted,
            "reachable -> unreachable alerts once"
        );
        m.apply_syscfg(vec![(discovered("X", "S1", None), healthy(70.0))]);
        assert!(!m.devices["e:S1"].unreachable_alerted, "recovery clears");
    }

    #[test]
    fn test_never_reachable_device_does_not_alert() {
        let mut m = DeviceManagerActor::new("e".into());
        m.apply_syscfg(vec![(
            discovered("X", "S1", None),
            DeviceHealth::unreachable(),
        )]);
        assert!(!m.devices["e:S1"].unreachable_alerted);
    }

    #[test]
    fn test_host_device_gets_system_metrics() {
        let mut m = DeviceManagerActor::new("e".into());
        m.system_metrics = Some(("STATION1".into(), vec![("mem_free_mb", 1024.0)]));
        m.apply_syscfg(vec![
            (
                discovered("PXIe-8880", "S0", Some("station1")),
                healthy(40.0),
            ),
            (discovered("NI 9205", "S1", Some("ModA")), healthy(40.0)),
        ]);
        let host = m.devices["e:station1"].last_status.as_ref().unwrap();
        assert_eq!(host.metrics["mem_free_mb"], MetricValue::Float(1024.0));
        let other = m.devices["e:ModA"].last_status.as_ref().unwrap();
        assert!(!other.metrics.contains_key("mem_free_mb"));
    }

    #[test]
    fn test_system_metrics_filters_missing() {
        let info = SystemInfo {
            hostname: Some("h".into()),
            memory_total_mb: Some(10.0),
            memory_free_mb: None,
            disk_free_mb: Some(f64::NAN),
            ..SystemInfo::default()
        };
        let (host, metrics) = system_metrics(&info).unwrap();
        assert_eq!(host, "h");
        assert_eq!(metrics, vec![("mem_total_mb", 10.0)]);
        assert!(system_metrics(&SystemInfo::default()).is_none());
    }

    #[test]
    fn test_visa_devices_tracked_and_expired() {
        let mut m = DeviceManagerActor::new("e".into());
        m.settings.removal_sweeps = 1;
        let instr = VisaInstrument::new("GPIB0::1::INSTR".into(), "GPIB".into());
        m.apply_visa(vec![instr]);
        let t = &m.devices["e:GPIB0::1::INSTR"];
        assert_eq!(t.device.device_type, DeviceType::Gpib);
        let st = t.last_status.as_ref().unwrap();
        assert_eq!(st.status, HealthStatus::Healthy);
        assert_eq!(st.metrics["probed"], MetricValue::Boolean(false));
        assert!(m
            .registry
            .get("e:GPIB0::1::INSTR")
            .unwrap()
            .daqmx_name
            .is_none());
        m.apply_visa(vec![]);
        assert!(m.devices.is_empty());
    }

    #[test]
    fn test_source_health_dedupes_alerts() {
        let mut h = SourceHealth::default();
        assert!(h.on_failure("x", "e1"));
        assert!(!h.on_failure("x", "e2"));
        assert!(h.on_success("x"));
        assert!(!h.on_success("x"));
        assert!(h.on_failure("x", "e3"));
    }

    #[test]
    fn test_hung_sweep_detected_once_and_recovers() {
        let mut m = DeviceManagerActor::new("e".into());
        m.mode = Mode::Hardware;
        m.settings.poll_interval_secs = 30; // stall limit 90 s
        let t0 = Instant::now();
        m.syscfg_job.start(t0);
        assert!(!m.check_stalls(t0 + Duration::from_secs(60)));
        assert_eq!(m.degraded_reason(), None);
        // past 3 intervals: reported once
        assert!(m.check_stalls(t0 + Duration::from_secs(91)));
        assert!(!m.check_stalls(t0 + Duration::from_secs(200)));
        assert_eq!(m.degraded_reason().as_deref(), Some("NI-SysCfg sweep hung"));
        assert!(
            m.last_reported
                .as_ref()
                .is_some_and(|(_, d)| d.as_deref() == Some("NI-SysCfg sweep hung")),
            "heartbeat state updated"
        );
        // the sweep never returned: no new one is started
        assert_eq!(m.jobs_to_start(), (false, false));
        m.finish_syscfg_job();
        assert!(!m.syscfg_job.is_running() && !m.syscfg_job.stalled);
        assert_eq!(m.degraded_reason(), None);
        assert_eq!(m.jobs_to_start(), (true, false));
    }

    #[test]
    fn test_stuck_visa_scan_does_not_starve_syscfg() {
        let mut m = DeviceManagerActor::new("e".into());
        m.mode = Mode::Hardware;
        m.settings.visa.enabled = true;
        m.settings.visa.poll_interval_secs = 60;
        let t0 = Instant::now();
        m.visa_job.start(t0);
        m.last_visa_scan = Some(t0);
        // SysCfg still sweeps while the VISA scan hangs
        assert_eq!(m.jobs_to_start(), (true, false));
        assert!(m.check_stalls(t0 + Duration::from_secs(181)));
        assert_eq!(
            m.degraded_reason().as_deref(),
            Some("NI-VISA discovery hung")
        );
        assert!(m.jobs_to_start().0);
        // minimum stall limit for short intervals
        assert_eq!(
            DeviceManagerActor::stall_limit(1),
            MIN_STALL_LIMIT,
            "1 s polls do not flag a 3 s enumeration"
        );
    }

    #[test]
    fn test_simulated_devices() {
        let manager = DeviceManagerActor::new("test-edge".to_string());
        let devices = manager.simulated_devices();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].device_type, DeviceType::Daq);
        assert_eq!(devices[1].device_type, DeviceType::Pxi);
        assert!(devices.iter().all(|d| d.id.starts_with("test-edge:")));
    }

    #[actix::test]
    async fn test_sim_manager_lists_devices() {
        let addr = sim_manager().start();
        let devices = addr.send(ListDevices).await.unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(addr.send(GetDeviceCount).await.unwrap(), 2);
        let snap = addr.send(GetManagerSnapshot).await.unwrap();
        assert_eq!(snap.mode, "simulated");
        assert!(snap.timer_armed);
        // simulation never starts blocking NI sweeps
        assert!(!addr.send(PollAllDevices).await.unwrap());
    }

    #[actix::test]
    async fn test_add_and_remove_device() {
        let addr = sim_manager().start();
        let initial = addr.send(GetDeviceCount).await.unwrap();
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
        assert_eq!(addr.send(GetDeviceCount).await.unwrap(), initial + 1);

        let removed = addr
            .send(RemoveDevice {
                device_id: "test-edge:new-device".to_string(),
            })
            .await
            .unwrap();
        assert!(removed);
        assert_eq!(addr.send(GetDeviceCount).await.unwrap(), initial);
        let removed = addr
            .send(RemoveDevice {
                device_id: "non-existent".to_string(),
            })
            .await
            .unwrap();
        assert!(!removed);
    }

    #[actix::test]
    async fn test_apply_config_rearms_timer_and_resolves_partial_thresholds() {
        let addr = sim_manager().start();
        addr.send(ApplyConfig {
            poll_interval_secs: Some(42),
            thresholds: ThresholdConfig {
                temperature_warning: None,
                temperature_critical: Some(90.0),
            },
        })
        .await
        .unwrap();
        let snap = addr.send(GetManagerSnapshot).await.unwrap();
        assert_eq!(snap.poll_interval_secs, 42);
        assert!(snap.timer_armed);
        assert_eq!(snap.temperature_warning, 65.0, "omitted value kept");
        assert_eq!(snap.temperature_critical, 90.0);

        // invalid combination is ignored
        addr.send(ApplyConfig {
            poll_interval_secs: None,
            thresholds: ThresholdConfig::full(95.0, 90.0),
        })
        .await
        .unwrap();
        let snap = addr.send(GetManagerSnapshot).await.unwrap();
        assert_eq!(
            (snap.temperature_warning, snap.temperature_critical),
            (65.0, 90.0)
        );
    }

    #[actix::test]
    async fn test_rearmed_timer_uses_new_interval() {
        let settings = DeviceManagerSettings {
            poll_interval_secs: 3600,
            ..sim_settings()
        };
        let addr = DeviceManagerActor::new("e".into())
            .with_settings(settings)
            .start();
        // only the immediate startup tick within the hour-long interval
        assert_eq!(addr.send(GetManagerSnapshot).await.unwrap().ticks, 1);
        addr.send(ApplyConfig {
            poll_interval_secs: Some(1),
            thresholds: ThresholdConfig::default(),
        })
        .await
        .unwrap();
        actix_rt::time::sleep(Duration::from_millis(1300)).await;
        let snap = addr.send(GetManagerSnapshot).await.unwrap();
        assert_eq!(snap.poll_interval_secs, 1);
        assert!(
            snap.ticks >= 2,
            "re-armed timer fired ({} ticks)",
            snap.ticks
        );
        // no hub attached -> nothing re-sent
        assert_eq!(addr.send(ResendState).await.unwrap(), 0);
    }

    #[actix::test]
    async fn test_stop_device_manager() {
        let addr = sim_manager().start();
        addr.send(StopDeviceManager).await.unwrap();
        actix_rt::time::sleep(Duration::from_millis(50)).await;
        assert!(!addr.connected());
    }
}
