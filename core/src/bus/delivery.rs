//! Delivery class enforcement — sequence numbers for the authoritative
//! class (per `docs/02-architecture.md` §3.1).
//!
//! Slice 001 ships a per-publisher monotonic counter. Snapshot-plus-deltas
//! reconnect and back-pressure overrides land in later slices when
//! distribution is exercised.

use std::sync::atomic::{AtomicU64, Ordering};

/// The two delivery classes from `docs/02-architecture.md` §3.1.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DeliveryClass {
    /// `event`, `stream-item`. Drop-oldest under back-pressure; no
    /// sequence guarantees; no replay on reconnect.
    Lossy,
    /// `fact-assert`, `fact-retract`, `lifecycle`, `error`. Per-publisher
    /// monotonic sequence numbers; gap detection; snapshot-plus-deltas
    /// on reconnect (deferred for slice 001).
    Authoritative,
}

/// Strictly monotonic per-publisher sequence counter for the
/// authoritative class.
///
/// `next()` returns successive `u64` values starting at 0; sequence
/// values are unique and strictly increasing per publisher instance.
#[derive(Debug, Default)]
pub struct SequenceCounter {
    next: AtomicU64,
}

impl SequenceCounter {
    pub const fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
        }
    }

    /// Allocate the next sequence number.
    pub fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst)
    }

    /// Peek the next value that would be returned without consuming it.
    pub fn peek(&self) -> u64 {
        self.next.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn counter_starts_at_zero() {
        let c = SequenceCounter::new();
        assert_eq!(c.peek(), 0);
        assert_eq!(c.next(), 0);
        assert_eq!(c.next(), 1);
    }

    // T029: per-publisher sequence numbers are strictly monotonic.
    proptest! {
        #[test]
        fn strictly_monotonic_for_n_calls(n in 1usize..1024) {
            let counter = SequenceCounter::new();
            let mut prev: Option<u64> = None;
            for _ in 0..n {
                let v = counter.next();
                if let Some(p) = prev {
                    prop_assert!(v > p, "sequence not strictly monotonic: {p} -> {v}");
                }
                prev = Some(v);
            }
        }
    }
}
