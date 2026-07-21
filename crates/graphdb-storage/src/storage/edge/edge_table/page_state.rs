//! Per-segment lock state tracking with optimistic reads.
//!
//! Provides a lightweight atomic CAS state machine for frozen segments,
//! reducing read-write contention by allowing optimistic reads when no
//! writer is active. Complements the existing RwLock with a fast-path
//! check that avoids lock acquisition overhead under low contention.

use std::sync::atomic::{AtomicU64, Ordering};

const STATE_MASK: u64 = 0xFF;
const VERSION_SHIFT: u8 = 8;

/// Lock state for a frozen segment.
///
/// Uses a single atomic u64 with packed fields:
/// - Bits [0..8]: current state (SegmentState as u8)
/// - Bits [8..64]: version counter (incremented on every state transition)
///
/// The read path uses `try_optimistic_read` to avoid RwLock acquisition
/// when no writer is active. The write path uses CAS to transition states.
#[derive(Debug)]
pub struct SegmentLockState {
    state_and_version: AtomicU64,
}

/// State of a frozen segment's lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SegmentState {
    /// Readable by optimistic readers; writers must CAS to Locked.
    Unlocked = 0,
    /// Exclusively held by a writer (merge/freeze).
    Locked = 1,
    /// Marked for eviction but still readable (second-chance).
    Marked = 2,
    /// Evicted to disk; must reload before access.
    Evicted = 3,
}

impl std::fmt::Display for SegmentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SegmentState::Unlocked => write!(f, "Unlocked"),
            SegmentState::Locked => write!(f, "Locked"),
            SegmentState::Marked => write!(f, "Marked"),
            SegmentState::Evicted => write!(f, "Evicted"),
        }
    }
}

impl SegmentLockState {
    /// Create a new lock state in `Unlocked` with version 0.
    pub fn new() -> Self {
        Self {
            state_and_version: AtomicU64::new(SegmentState::Unlocked as u64),
        }
    }

    /// Read the current packed value (state + version).
    #[inline]
    pub fn read_packed(&self) -> u64 {
        self.state_and_version.load(Ordering::Acquire)
    }

    /// Read the current state.
    #[inline]
    pub fn read_state(&self) -> SegmentState {
        let packed = self.read_packed();
        let state_bits = (packed & STATE_MASK) as u8;
        match state_bits {
            0 => SegmentState::Unlocked,
            1 => SegmentState::Locked,
            2 => SegmentState::Marked,
            3 => SegmentState::Evicted,
            _ => unreachable!("invalid state bits: {}", state_bits),
        }
    }

    /// Check if the segment is write-locked (state == Locked).
    #[inline]
    pub fn is_write_locked(&self) -> bool {
        self.read_state() == SegmentState::Locked
    }

    /// Attempt to CAS from `expected` state to `target` state.
    /// Returns true on success. On failure, no state change occurs.
    pub fn try_transition(&self, expected: SegmentState, target: SegmentState) -> bool {
        let current = self.read_packed();
        let current_state = (current & STATE_MASK) as u8;

        if current_state != expected as u8 {
            return false;
        }

        let version = current >> VERSION_SHIFT;
        let new_packed = (version << VERSION_SHIFT) | target as u64;

        self.state_and_version
            .compare_exchange(current, new_packed, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// CAS from Unlocked → Marked (second-chance eviction).
    #[inline]
    pub fn try_mark(&self) -> bool {
        self.try_transition(SegmentState::Unlocked, SegmentState::Marked)
    }

    /// CAS from Marked → Evicted.
    #[inline]
    pub fn try_evict(&self) -> bool {
        self.try_transition(SegmentState::Marked, SegmentState::Evicted)
    }

    /// CAS from Evicted → Unlocked (on reload).
    #[inline]
    pub fn try_resurrect(&self) -> bool {
        self.try_transition(SegmentState::Evicted, SegmentState::Unlocked)
    }

    /// Attempt an optimistic read. Returns the result of `func` if successful.
    /// If the state is Locked or changed during the read, returns None.
    ///
    /// The caller should perform the actual data read inside `func`. The
    /// function verifies that no writer was active before and after the call.
    pub fn try_optimistic_read<F, R>(&self, func: F) -> Option<R>
    where
        F: FnOnce() -> R,
    {
        let packed_before = self.read_packed();
        if (packed_before & STATE_MASK) as u8 == SegmentState::Locked as u8 {
            return None;
        }

        let result = func();

        let packed_after = self.read_packed();
        if packed_before == packed_after {
            Some(result)
        } else {
            None
        }
    }
}

impl Default for SegmentLockState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_is_unlocked() {
        let state = SegmentLockState::new();
        assert_eq!(state.read_state(), SegmentState::Unlocked);
        assert!(!state.is_write_locked());
    }

    #[test]
    fn test_try_transition() {
        let state = SegmentLockState::new();
        assert!(state.try_transition(SegmentState::Unlocked, SegmentState::Locked));
        assert_eq!(state.read_state(), SegmentState::Locked);
        assert!(state.is_write_locked());
        assert!(!state.try_transition(SegmentState::Unlocked, SegmentState::Marked));
    }

    #[test]
    fn test_mark_and_evict_flow() {
        let state = SegmentLockState::new();

        // Unlocked → Marked
        assert!(state.try_mark());
        assert_eq!(state.read_state(), SegmentState::Marked);

        // Marked → Evicted
        assert!(state.try_evict());
        assert_eq!(state.read_state(), SegmentState::Evicted);

        // Evicted → Unlocked (reload)
        assert!(state.try_resurrect());
        assert_eq!(state.read_state(), SegmentState::Unlocked);
    }

    #[test]
    fn test_optimistic_read_succeeds_when_unlocked() {
        let state = SegmentLockState::new();
        let data = [1u8, 2, 3, 4, 5];
        let result = state.try_optimistic_read(|| data.iter().sum::<u8>());
        assert_eq!(result, Some(15));
    }

    #[test]
    fn test_optimistic_read_fails_when_locked() {
        let state = SegmentLockState::new();
        assert!(state.try_transition(SegmentState::Unlocked, SegmentState::Locked));
        let data = [1u8, 2, 3];
        let result = state.try_optimistic_read(|| data.iter().sum::<u8>());
        assert_eq!(result, None);
    }
}
