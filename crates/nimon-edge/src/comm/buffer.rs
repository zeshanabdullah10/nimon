//! Bounded offline buffer for outgoing hub messages
//!
//! Messages are serialized once (JSON) before they enter the buffer.
//! When a limit is exceeded the oldest *droppable* message (device
//! status, superseded by the next sweep anyway) is evicted first; alerts,
//! predictions, action results and removals are only evicted when no
//! droppable message is left.

use std::collections::VecDeque;

/// Delivery class of an outgoing message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundClass {
    /// Must survive outages (alerts, predictions, action results, removals)
    Priority,
    /// May be dropped first when the buffer is full (device status)
    Droppable,
    /// Only meaningful right now; never buffered (heartbeat, pong,
    /// registration)
    Ephemeral,
}

/// A serialized message waiting to be written to the socket
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbound {
    pub json: String,
    pub class: OutboundClass,
}

impl Outbound {
    pub fn new(json: String, class: OutboundClass) -> Self {
        Self { json, class }
    }
}

/// FIFO buffer bounded by message count and total bytes
#[derive(Debug)]
pub struct OutboundBuffer {
    items: VecDeque<Outbound>,
    bytes: usize,
    droppable: usize,
    max_messages: usize,
    max_bytes: usize,
    dropped: u64,
}

impl OutboundBuffer {
    /// `max_messages == 0` disables buffering (everything is dropped)
    pub fn new(max_messages: usize, max_bytes: usize) -> Self {
        Self {
            items: VecDeque::new(),
            bytes: 0,
            droppable: 0,
            max_messages,
            max_bytes: max_bytes.max(1),
            dropped: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Total buffered JSON bytes
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Messages dropped since the last call
    pub fn take_dropped(&mut self) -> u64 {
        std::mem::take(&mut self.dropped)
    }

    /// Append a message (ephemeral messages are discarded)
    pub fn push_back(&mut self, item: Outbound) {
        if item.class == OutboundClass::Ephemeral {
            return;
        }
        self.account_add(&item);
        self.items.push_back(item);
        self.enforce();
    }

    /// Put a message back at the front (it is older than anything queued)
    pub fn push_front(&mut self, item: Outbound) {
        if item.class == OutboundClass::Ephemeral {
            return;
        }
        self.account_add(&item);
        self.items.push_front(item);
        self.enforce();
    }

    /// Return unsent messages of a dropped connection to the front,
    /// preserving their order
    pub fn requeue_front(&mut self, unsent: Vec<Outbound>) {
        for item in unsent.into_iter().rev() {
            if item.class == OutboundClass::Ephemeral {
                continue;
            }
            self.account_add(&item);
            self.items.push_front(item);
        }
        self.enforce();
    }

    pub fn pop_front(&mut self) -> Option<Outbound> {
        let item = self.items.pop_front()?;
        self.account_remove(&item);
        Some(item)
    }

    fn account_add(&mut self, item: &Outbound) {
        self.bytes += item.json.len();
        if item.class == OutboundClass::Droppable {
            self.droppable += 1;
        }
    }

    fn account_remove(&mut self, item: &Outbound) {
        self.bytes -= item.json.len();
        if item.class == OutboundClass::Droppable {
            self.droppable -= 1;
        }
    }

    fn over_limit(&self) -> bool {
        self.items.len() > self.max_messages || self.bytes > self.max_bytes
    }

    fn enforce(&mut self) {
        while self.over_limit() {
            let index = if self.droppable > 0 {
                self.items
                    .iter()
                    .position(|i| i.class == OutboundClass::Droppable)
                    .unwrap_or(0)
            } else {
                0
            };
            match self.items.remove(index) {
                Some(item) => {
                    self.account_remove(&item);
                    self.dropped += 1;
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(n: u32) -> Outbound {
        Outbound::new(format!("s{n}"), OutboundClass::Droppable)
    }

    fn alert(n: u32) -> Outbound {
        Outbound::new(format!("a{n}"), OutboundClass::Priority)
    }

    fn contents(b: &mut OutboundBuffer) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(i) = b.pop_front() {
            out.push(i.json);
        }
        out
    }

    #[test]
    fn test_fifo_order() {
        let mut b = OutboundBuffer::new(10, 1 << 20);
        b.push_back(status(1));
        b.push_back(alert(1));
        b.push_back(status(2));
        assert_eq!(b.len(), 3);
        assert_eq!(contents(&mut b), vec!["s1", "a1", "s2"]);
        assert_eq!(b.bytes(), 0);
    }

    #[test]
    fn test_full_drops_oldest_status_first() {
        let mut b = OutboundBuffer::new(3, 1 << 20);
        b.push_back(alert(1));
        b.push_back(status(1));
        b.push_back(status(2));
        b.push_back(alert(2)); // evicts s1, keeps both alerts
        assert_eq!(b.take_dropped(), 1);
        assert_eq!(contents(&mut b), vec!["a1", "s2", "a2"]);
    }

    #[test]
    fn test_full_of_priority_drops_oldest() {
        let mut b = OutboundBuffer::new(2, 1 << 20);
        b.push_back(alert(1));
        b.push_back(alert(2));
        b.push_back(alert(3));
        assert_eq!(contents(&mut b), vec!["a2", "a3"]);
    }

    #[test]
    fn test_byte_limit() {
        let mut b = OutboundBuffer::new(100, 4);
        b.push_back(status(1)); // 2 bytes
        b.push_back(status(2)); // 4 bytes
        b.push_back(alert(1)); // 6 bytes -> evict s1
        assert_eq!(b.bytes(), 4);
        assert_eq!(contents(&mut b), vec!["s2", "a1"]);
    }

    #[test]
    fn test_ephemeral_never_buffered() {
        let mut b = OutboundBuffer::new(10, 1 << 20);
        b.push_back(Outbound::new("hb".into(), OutboundClass::Ephemeral));
        b.requeue_front(vec![Outbound::new("hb".into(), OutboundClass::Ephemeral)]);
        assert!(b.is_empty());
    }

    #[test]
    fn test_requeue_front_preserves_order() {
        let mut b = OutboundBuffer::new(10, 1 << 20);
        b.push_back(status(9)); // queued after the connection dropped
        b.requeue_front(vec![alert(1), status(1), alert(2)]);
        assert_eq!(contents(&mut b), vec!["a1", "s1", "a2", "s9"]);
    }

    #[test]
    fn test_disabled_buffer_drops_everything() {
        let mut b = OutboundBuffer::new(0, 1 << 20);
        b.push_back(alert(1));
        assert!(b.is_empty());
        assert_eq!(b.take_dropped(), 1);
        assert_eq!(b.take_dropped(), 0);
    }
}
