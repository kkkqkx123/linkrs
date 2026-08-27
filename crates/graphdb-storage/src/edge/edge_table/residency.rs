//! Segment residency tracking for memory-aware tiering.
//!
//! Tracks whether a frozen segment's CSR data is resident in physical memory
//! or has been evicted to a spill file. Enables transparent reload on access.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

/// Residency state of a segment's CSR data.
#[derive(Debug, Clone)]
pub enum SegmentResidency {
    /// CSR data is resident in physical memory and directly accessible.
    Resident,
    /// CSR data has been evicted to a spill file.
    /// The segment's CSR is empty; access triggers transparent reload.
    Evicted {
        /// Path to the spill file containing serialized CSR data.
        spill_path: PathBuf,
        /// Size of the spill file in bytes (for memory accounting).
        spill_size: u64,
    },
}

impl SegmentResidency {
    /// Returns true if the segment data is resident in memory.
    pub fn is_resident(&self) -> bool {
        matches!(self, SegmentResidency::Resident)
    }

    /// Returns true if the segment data has been evicted to disk.
    pub fn is_evicted(&self) -> bool {
        matches!(self, SegmentResidency::Evicted { .. })
    }

    /// Returns the spill size if evicted, or 0 if resident.
    pub fn spill_size(&self) -> u64 {
        match self {
            SegmentResidency::Resident => 0,
            SegmentResidency::Evicted { spill_size, .. } => *spill_size,
        }
    }
}

/// Access timestamp counter for LRU eviction ordering.
/// Uses a monotonic counter rather than wall-clock time for determinism.
pub struct AccessClock {
    counter: AtomicU64,
}

impl AccessClock {
    pub fn new() -> Self {
        Self {
            counter: AtomicU64::new(0),
        }
    }

    /// Record an access and return the assigned timestamp.
    pub fn tick(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Read the current timestamp without incrementing.
    pub fn now(&self) -> u64 {
        self.counter.load(Ordering::Relaxed)
    }
}

impl Default for AccessClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Global shared access clock for LRU eviction ordering.
///
/// Both the read path (recording segment accesses) and the eviction engine
/// (comparing last-access timestamps) use this single clock instance,
/// ensuring that LRU timestamps are comparable across the system.
pub(crate) static GLOBAL_ACCESS_CLOCK: LazyLock<AccessClock> = LazyLock::new(AccessClock::new);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_residency_state_checks() {
        let resident = SegmentResidency::Resident;
        assert!(resident.is_resident());
        assert!(!resident.is_evicted());
        assert_eq!(resident.spill_size(), 0);

        let evicted = SegmentResidency::Evicted {
            spill_path: PathBuf::from("/tmp/spill.bin"),
            spill_size: 1024,
        };
        assert!(!evicted.is_resident());
        assert!(evicted.is_evicted());
        assert_eq!(evicted.spill_size(), 1024);
    }

    #[test]
    fn test_access_clock_monotonic() {
        let clock = AccessClock::new();
        let t1 = clock.tick();
        let t2 = clock.tick();
        let t3 = clock.tick();
        assert!(t1 < t2);
        assert!(t2 < t3);
        assert!(clock.now() > t3);
    }
}
