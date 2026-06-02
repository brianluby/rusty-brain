//! A bounded, lossy ring of change events shared between a background daemon
//! subscriber and the `poll_changes` tool. Bounded so a flood of writes (or a
//! client that never polls) can never grow memory without limit; lossy so the
//! newest events win and the count of dropped events is reported on each drain.

use rb_types::MemoryChanged;
use std::collections::VecDeque;

/// A bounded ring of buffered change events with a since-last-drain dropped
/// counter. Cheap to clone via `Arc<Mutex<ChangeBuffer>>` at the call site.
#[derive(Debug)]
pub struct ChangeBuffer {
    events: VecDeque<MemoryChanged>,
    capacity: usize,
    /// Events dropped (evicted on overflow, or reported by broadcast `Lagged`)
    /// since the last `drain`. Reset to 0 by `drain`.
    dropped: u64,
}

/// The result of draining the ring: up to `max` events plus the number of
/// events dropped since the previous drain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drained {
    pub events: Vec<MemoryChanged>,
    pub dropped: u64,
}

impl ChangeBuffer {
    /// Create an empty ring holding at most `capacity` events (min 1).
    pub fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::new(),
            capacity: capacity.max(1),
            dropped: 0,
        }
    }

    /// Push one event, evicting (and counting) the oldest if at capacity.
    pub fn push(&mut self, evt: MemoryChanged) {
        if self.events.len() >= self.capacity {
            // Evict oldest: newest events are the most useful to a poller.
            let _ = self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.events.push_back(evt);
    }

    /// Record that the broadcast dropped `n` events for this subscriber.
    pub fn record_dropped(&mut self, n: u64) {
        self.dropped = self.dropped.saturating_add(n);
    }

    /// Drain up to `max` of the oldest buffered events, returning them plus the
    /// dropped count accumulated since the previous drain (then reset to 0).
    pub fn drain(&mut self, max: usize) -> Drained {
        let take = max.min(self.events.len());
        let events: Vec<MemoryChanged> = self.events.drain(..take).collect();
        let dropped = self.dropped;
        self.dropped = 0;
        Drained { events, dropped }
    }

    /// Number of buffered events currently held.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the ring currently holds no events.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};

    fn evt(kind: ChangeKind) -> MemoryChanged {
        MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Global,
            kind,
        }
    }

    #[test]
    fn push_then_drain_returns_events_in_order_no_drops() {
        let mut b = ChangeBuffer::new(8);
        let a = evt(ChangeKind::Created);
        let c = evt(ChangeKind::Updated);
        b.push(a.clone());
        b.push(c.clone());
        assert_eq!(b.len(), 2);
        let drained = b.drain(10);
        assert_eq!(drained.events, vec![a, c]);
        assert_eq!(drained.dropped, 0);
        assert!(b.is_empty(), "drain empties what it took");
    }

    #[test]
    fn drain_respects_max_and_leaves_remainder() {
        let mut b = ChangeBuffer::new(8);
        for _ in 0..5 {
            b.push(evt(ChangeKind::Created));
        }
        let first = b.drain(2);
        assert_eq!(first.events.len(), 2);
        assert_eq!(b.len(), 3, "remainder stays buffered");
        let second = b.drain(100);
        assert_eq!(second.events.len(), 3);
        assert_eq!(second.dropped, 0);
    }

    #[test]
    fn overflow_evicts_oldest_and_counts_drops() {
        let mut b = ChangeBuffer::new(2);
        let e1 = evt(ChangeKind::Created);
        let e2 = evt(ChangeKind::Updated);
        let e3 = evt(ChangeKind::Archived);
        b.push(e1);
        b.push(e2.clone());
        b.push(e3.clone()); // evicts e1
        assert_eq!(b.len(), 2, "capacity is never exceeded");
        let drained = b.drain(10);
        assert_eq!(
            drained.events,
            vec![e2, e3],
            "oldest evicted; newest retained"
        );
        assert_eq!(drained.dropped, 1, "one eviction counted as a drop");
    }

    #[test]
    fn record_dropped_accumulates_and_resets_on_drain() {
        let mut b = ChangeBuffer::new(4);
        b.record_dropped(3);
        b.push(evt(ChangeKind::Created));
        b.record_dropped(2);
        let drained = b.drain(10);
        assert_eq!(drained.events.len(), 1);
        assert_eq!(drained.dropped, 5, "3 + 2 reported once");
        // Dropped resets after a drain.
        let next = b.drain(10);
        assert_eq!(next.dropped, 0);
        assert!(next.events.is_empty());
    }

    #[test]
    fn zero_capacity_is_clamped_to_one() {
        let mut b = ChangeBuffer::new(0);
        b.push(evt(ChangeKind::Created));
        b.push(evt(ChangeKind::Updated)); // evicts the first
        assert_eq!(b.len(), 1);
        let drained = b.drain(10);
        assert_eq!(drained.events.len(), 1);
        assert_eq!(drained.dropped, 1);
    }
}
