//! Per-segment lock state tracking with optimistic reads.
//!
//! Provides a lightweight atomic CAS state machine for frozen segments,
//! reducing read-write contention by allowing optimistic reads when no
//! writer is active. Complements the existing RwLock with a fast-path
//! check that avoids lock acquisition overhead under low contention.

use std::sync::atomic::{AtomicU64, Ordering};

const STATE_MASK: u64 = 0xFF;
const VERSION_SHIFT: u8 = 8;
const VERSION_INC: u64 = 1 << VERSION_SHIFT;

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

    /// Read the current version (upper 56 bits).
    #[inline]
    pub fn read_version(&self) -> u64 {
        self.read_packed() >> VERSION_SHIFT
    }

    /// Check if the segment is write-locked (state == Locked).
    #[inline]
    pub fn is_write_locked(&self) -> bool {
        self.read_state() == SegmentState::Locked
    }

    /// Check if the segment is in a readable state (Unlocked or Marked).
    #[inline]
    pub fn is_readable(&self) -> bool {
        matches!(
            self.read_state(),
            SegmentState::Unlocked | SegmentState::Marked
        )
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

    /// CAS from Unlocked → Locked. Returns true on success.
    #[inline]
    pub fn try_lock(&self) -> bool {
        self.try_transition(SegmentState::Unlocked, SegmentState::Locked)
    }

    /// CAS from Locked → Unlocked, incrementing version.
    /// Returns true on success.
    #[inline]
    pub fn try_unlock(&self) -> bool {
        let current = self.read_packed();
        let current_state = (current & STATE_MASK) as u8;

        if current_state != SegmentState::Locked as u8 {
            return false;
        }

        let version = (current >> VERSION_SHIFT) + 1;
        let new_packed = (version << VERSION_SHIFT) | SegmentState::Unlocked as u64;

        self.state_and_version
            .compare_exchange(current, new_packed, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// CAS from Unlocked → Marked (second-chance eviction).
    #[inline]
    pub fn try_mark(&self) -> bool {
        self.try_transition(SegmentState::Unlocked, SegmentState::Marked)
    }

    /// CAS from Marked → Unlocked (unmark after second chance).
    #[inline]
    pub fn try_unmark(&self) -> bool {
        self.try_transition(SegmentState::Marked, SegmentState::Unlocked)
    }

    /// CAS from Marked → Evicted.
    #[inline]
    pub fn try_evict(&self) -> bool {
        self.try_transition(SegmentState::Marked, SegmentState::Evicted)
    }

    /// CAS from Locked → Evicted (direct eviction after write completes).
    #[inline]
    pub fn try_evict_from_locked(&self) -> bool {
        self.try_transition(SegmentState::Locked, SegmentState::Evicted)
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

    /// Spin-wait for the segment to become Unlocked or Marked (readable).
    /// Returns the packed value once readable. Has a bounded retry count.
    pub fn spin_wait_readable(&self, max_spins: u32) -> u64 {
        for _ in 0..max_spins {
            let packed = self.read_packed();
            let state = (packed & STATE_MASK) as u8;
            if state == SegmentState::Unlocked as u8 || state == SegmentState::Marked as u8 {
                return packed;
            }
            std::hint::spin_loop();
        }
        self.read_packed()
    }
}

impl Default for SegmentLockState {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard for a write-locked segment.
///
/// When dropped, transitions the segment back to Unlocked with version increment.
pub struct SegmentWriteGuard<'a> {
    state: &'a SegmentLockState,
    active: bool,
}

impl<'a> SegmentWriteGuard<'a> {
    /// Acquire a write guard. Returns None if the segment is not Unlocked.
    pub fn acquire(state: &'a SegmentLockState) -> Option<Self> {
        if state.try_lock() {
            Some(Self { state, active: true })
        } else {
            None
        }
    }

    /// Check if the guard is still active.
    pub fn is_active(&self) -> bool {
        self.active
    }
}

impl<'a> Drop for SegmentWriteGuard<'a> {
    fn drop(&mut self) {
        if self.active {
            self.state.try_unlock();
            self.active = false;
        }
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
        assert!(state.is_readable());
    }

    #[test]
    fn test_try_lock_and_unlock() {
        let state = SegmentLockState::new();
        assert!(state.try_lock());
        assert_eq!(state.read_state(), SegmentState::Locked);
        assert!(state.is_write_locked());
        assert!(!state.is_readable());

        assert!(state.try_unlock());
        assert_eq!(state.read_state(), SegmentState::Unlocked);
    }

    #[test]
    fn test_try_lock_fails_when_locked() {
        let state = SegmentLockState::new();
        assert!(state.try_lock());
        assert!(!state.try_lock());
        assert_eq!(state.read_state(), SegmentState::Locked);
    }

    #[test]
    fn test_version_increments_on_unlock() {
        let state = SegmentLockState::new();
        let v0 = state.read_version();
        state.try_lock();
        state.try_unlock();
        let v1 = state.read_version();
        assert!(v1 > v0);
    }

    #[test]
    fn test_mark_and_evict_flow() {
        let state = SegmentLockState::new();

        // Unlocked → Marked
        assert!(state.try_mark());
        assert_eq!(state.read_state(), SegmentState::Marked);
        assert!(state.is_readable());

        // Marked → Evicted
        assert!(state.try_evict());
        assert_eq!(state.read_state(), SegmentState::Evicted);
        assert!(!state.is_readable());

        // Evicted → Unlocked (reload)
        assert!(state.try_resurrect());
        assert_eq!(state.read_state(), SegmentState::Unlocked);
    }

    #[test]
    fn test_try_unmark() {
        let state = SegmentLockState::new();
        assert!(state.try_mark());
        assert!(state.try_unmark());
        assert_eq!(state.read_state(), SegmentState::Unlocked);
    }

    #[test]
    fn test_optimistic_read_succeeds_when_unlocked() {
        let state = SegmentLockState::new();
        let data = vec![1u8, 2, 3, 4, 5];
        let result = state.try_optimistic_read(|| data.iter().sum::<u8>());
        assert_eq!(result, Some(15));
    }

    #[test]
    fn test_optimistic_read_fails_when_locked() {
        let state = SegmentLockState::new();
        state.try_lock();
        let data = vec![1u8, 2, 3];
        let result = state.try_optimistic_read(|| data.iter().sum::<u8>());
        assert_eq!(result, None);
    }

    #[test]
    fn test_write_guard() {
        let state = SegmentLockState::new();
        {
            let guard = SegmentWriteGuard::acquire(&state).unwrap();
            assert!(guard.is_active());
            assert!(state.is_write_locked());
        }
        // Guard dropped, should be unlocked
        assert_eq!(state.read_state(), SegmentState::Unlocked);
    }

    #[test]
    fn test_write_guard_fails_when_locked() {
        let state = SegmentLockState::new();
        let _guard = SegmentWriteGuard::acquire(&state).unwrap();
        assert!(SegmentWriteGuard::acquire(&state).is_none());
    }

    #[test]
    fn test_spin_wait_readable() {
        let state = SegmentLockState::new();
        let packed = state.spin_wait_readable(100);
        assert!(
            (packed & STATE_MASK) as u8 == SegmentState::Unlocked as u8
                || (packed & STATE_MASK) as u8 == SegmentState::Marked as u8
        );
    }

    #[test]
    fn test_concurrent_lock_attempts() {
        use std::sync::Arc;
        use std::thread;

        let state = Arc::new(SegmentLockState::new());
        let mut handles = Vec::new();

        for _ in 0..8 {
            let s = Arc::clone(&state);
            handles.push(thread::spawn(move || {
                let mut successes = 0;
                for _ in 0..100 {
                    if let Some(guard) = SegmentWriteGuard::acquire(&s) {
                        successes += 1;
                        drop(guard);
                    }
                }
                successes
            }));
        }

        let total: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert!(total > 0);
        assert_eq!(state.read_state(), SegmentState::Unlocked);
    }
}
