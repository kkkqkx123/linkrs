//! Process-wide memory watermark coordinator.
//!
//! A lightweight global accounting layer shared by every memory consumer in
//! the process (query blocking-operator budgets, columnar acceleration
//! caches, spill staging). It replaces the removed vmcache-style
//! `BufferManager`: instead of paging data in and out, it tracks aggregate
//! reservations against one configurable budget and reports the current
//! pressure level so consumers can *degrade accelerations* conservatively
//! rather than fail.
//!
//! Degradation contract: under `High`/`Critical` pressure consumers must
//! give up non-essential speedups (columnar chunk caches, batch fast paths)
//! and stay on the plain row path. Hot data is never evicted here.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// Default process budget (2 GiB): generous headroom for single-node use,
/// overridable via [`set_process_budget`] at startup.
pub const DEFAULT_PROCESS_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Aggregated memory pressure level derived from reserved/budget ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressure {
    /// Below the high watermark: normal operation.
    Low,
    /// Above high watermark: stop building new acceleration structures.
    High,
    /// Above critical watermark: also shed existing optional buffers where
    /// cheaply possible.
    Critical,
}

impl Pressure {
    /// Whether building new columnar acceleration structures is allowed.
    pub fn allows_columnar(&self) -> bool {
        matches!(self, Pressure::Low)
    }
}

struct Watermark {
    budget_bytes: AtomicU64,
    reserved_bytes: AtomicU64,
}

static WATERMARK: OnceLock<Watermark> = OnceLock::new();

fn watermark() -> &'static Watermark {
    WATERMARK.get_or_init(|| Watermark {
        budget_bytes: AtomicU64::new(DEFAULT_PROCESS_BUDGET_BYTES),
        reserved_bytes: AtomicU64::new(0),
    })
}

/// Override the process-wide memory budget (bytes).
///
/// Later calls shrink/grow the ceiling immediately; current reservations are
/// never revoked, they just may exceed the new budget until released.
pub fn set_process_budget(bytes: u64) {
    watermark().budget_bytes.store(bytes, Ordering::Relaxed);
}

/// Currently configured process budget.
pub fn process_budget() -> u64 {
    watermark().budget_bytes.load(Ordering::Relaxed)
}

/// Bytes currently reserved through this coordinator.
pub fn reserved() -> u64 {
    watermark().reserved_bytes.load(Ordering::Relaxed)
}

/// Try to reserve `bytes`; returns `false` when the process budget would be
/// exceeded. Reservations are advisory accounting, not allocation.
pub fn try_reserve(bytes: u64) -> bool {
    let w = watermark();
    let mut prev = w.reserved_bytes.load(Ordering::Relaxed);
    loop {
        let total = match prev.checked_add(bytes) {
            Some(t) => t,
            None => return false,
        };
        if total > w.budget_bytes.load(Ordering::Relaxed) {
            return false;
        }
        match w.reserved_bytes.compare_exchange_weak(
            prev,
            total,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(current) => prev = current,
        }
    }
}

/// Release a previous reservation.
pub fn release(bytes: u64) {
    let w = watermark();
    let prev = w.reserved_bytes.fetch_sub(bytes, Ordering::AcqRel);
    debug_assert!(prev >= bytes, "watermark release underflow");
}

/// Current pressure level.
pub fn pressure() -> Pressure {
    let w = watermark();
    let budget = w.budget_bytes.load(Ordering::Relaxed).max(1);
    let used = w.reserved_bytes.load(Ordering::Relaxed);
    // Percentages computed without floats to stay exact near boundaries.
    const HIGH_PCT: u64 = 70;
    const CRITICAL_PCT: u64 = 90;
    if used.saturating_mul(100) >= budget.saturating_mul(CRITICAL_PCT) {
        Pressure::Critical
    } else if used.saturating_mul(100) >= budget.saturating_mul(HIGH_PCT) {
        Pressure::High
    } else {
        Pressure::Low
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_accounts_and_releases() {
        set_process_budget(1024);
        assert!(try_reserve(512));
        assert_eq!(reserved(), 512);
        assert!(pressure() == Pressure::Low || pressure() == Pressure::High);

        assert!(
            !try_reserve(600),
            "over-budget reservation must be rejected"
        );
        release(512);
        assert_eq!(reserved(), 0);
        set_process_budget(DEFAULT_PROCESS_BUDGET_BYTES);
    }

    #[test]
    fn pressure_levels_follow_watermarks() {
        set_process_budget(1000);
        release(reserved()); // reset any residue from other tests

        try_reserve(500); // 50%
        assert_eq!(pressure(), Pressure::Low);
        try_reserve(250); // 75%
        assert_eq!(pressure(), Pressure::High);
        try_reserve(200); // 95%
        assert_eq!(pressure(), Pressure::Critical);

        release(950);
        assert_eq!(pressure(), Pressure::Low);
        set_process_budget(DEFAULT_PROCESS_BUDGET_BYTES);
    }

    #[test]
    fn columnar_allowed_only_under_low_pressure() {
        set_process_budget(1000);
        release(reserved());

        try_reserve(990);
        assert_eq!(pressure(), Pressure::Critical);
        assert!(!pressure().allows_columnar());
        release(990);
        assert!(pressure().allows_columnar());
        set_process_budget(DEFAULT_PROCESS_BUDGET_BYTES);
    }
}
