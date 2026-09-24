//! Run-wide counters and the end-of-run summary.

use nimon_core::protocol::WsMessageType;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Wire name of a message type (`device_status`, ...)
pub fn type_name(t: WsMessageType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{t:?}"))
}

#[derive(Default)]
pub struct Stats {
    sent: Mutex<BTreeMap<String, u64>>,
    received: Mutex<BTreeMap<String, u64>>,
    pub actions_received: AtomicU64,
    pub actions_replied: AtomicU64,
    pub actions_withheld: AtomicU64,
    pub hub_errors: AtomicU64,
    pub parse_errors: AtomicU64,
    pub connects: AtomicU64,
    pub connect_failures: AtomicU64,
    pub disconnects: AtomicU64,
    pub planned_reconnects: AtomicU64,
    pub connected: AtomicU64,
}

fn bump(map: &Mutex<BTreeMap<String, u64>>, key: String) {
    if let Ok(mut m) = map.lock() {
        *m.entry(key).or_default() += 1;
    }
}

fn total(map: &Mutex<BTreeMap<String, u64>>) -> u64 {
    map.lock().map(|m| m.values().sum()).unwrap_or(0)
}

pub fn inc(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

pub fn dec(c: &AtomicU64) {
    let _ = c.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1));
}

pub fn get(c: &AtomicU64) -> u64 {
    c.load(Ordering::Relaxed)
}

impl Stats {
    pub fn record_sent(&self, t: WsMessageType) {
        bump(&self.sent, type_name(t));
    }

    pub fn record_received(&self, t: WsMessageType) {
        bump(&self.received, type_name(t));
    }

    pub fn total_sent(&self) -> u64 {
        total(&self.sent)
    }

    pub fn total_received(&self) -> u64 {
        total(&self.received)
    }

    #[cfg(test)]
    pub fn sent_count(&self, t: WsMessageType) -> u64 {
        self.sent
            .lock()
            .ok()
            .and_then(|m| m.get(&type_name(t)).copied())
            .unwrap_or(0)
    }

    /// Human-readable summary.
    pub fn summary(&self, elapsed: Duration, fatal: Option<&str>) -> String {
        let secs = elapsed.as_secs_f64().max(0.001);
        let mut s = String::new();
        let mut line = |l: String| {
            s.push_str(&l);
            s.push('\n');
        };
        line("==================== nimon-sim summary ====================".into());
        line(format!("elapsed            {secs:.1}s"));
        let table =
            |name: &str, map: &Mutex<BTreeMap<String, u64>>, out: &mut dyn FnMut(String)| {
                let m = map.lock().map(|m| m.clone()).unwrap_or_default();
                let sum: u64 = m.values().sum();
                out(format!(
                    "{name:<18} {sum} total ({:.1} msg/s)",
                    sum as f64 / secs
                ));
                for (k, v) in m {
                    out(format!("  {k:<16} {v}"));
                }
            };
        table("messages sent", &self.sent, &mut line);
        table("messages received", &self.received, &mut line);
        line(format!(
            "actions            received {} / replied {} / withheld {}",
            get(&self.actions_received),
            get(&self.actions_replied),
            get(&self.actions_withheld)
        ));
        line(format!(
            "connections        {} ok, {} failed, {} dropped, {} planned reconnects",
            get(&self.connects),
            get(&self.connect_failures),
            get(&self.disconnects),
            get(&self.planned_reconnects)
        ));
        line(format!(
            "errors             {} hub error msgs, {} unparseable msgs",
            get(&self.hub_errors),
            get(&self.parse_errors)
        ));
        match fatal {
            Some(reason) => line(format!("result             FATAL: {reason}")),
            None => line("result             ok".into()),
        }
        line("===========================================================".into());
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_by_type() {
        let s = Stats::default();
        s.record_sent(WsMessageType::DeviceStatus);
        s.record_sent(WsMessageType::DeviceStatus);
        s.record_sent(WsMessageType::Heartbeat);
        s.record_received(WsMessageType::Ack);
        assert_eq!(s.total_sent(), 3);
        assert_eq!(s.total_received(), 1);
        assert_eq!(s.sent_count(WsMessageType::DeviceStatus), 2);
        assert_eq!(type_name(WsMessageType::DeviceRemoved), "device_removed");
        dec(&s.connected);
        assert_eq!(get(&s.connected), 0);
        let text = s.summary(Duration::from_secs(2), None);
        assert!(text.contains("device_status    2"));
        assert!(text.contains("1.5 msg/s"));
        assert!(s
            .summary(Duration::from_secs(1), Some("x"))
            .contains("FATAL: x"));
    }
}
