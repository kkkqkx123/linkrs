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
//!
//! Scope: this is a query-side admission signal for building new
//! acceleration structures. Storage-side eviction must use the instance
//! [`crate::engine::resource_budget::MemoryPressureLevel`] derived from the
//! aggregate snapshot, never this process watermark.
//!
//! Pressure alignment: the process watermarks sit slightly *below* the
//! instance soft/hard ratios on purpose — query accelerations degrade
//! before storage admission starts rejecting work. By default the pair is
//! derived from the instance ratios through [`configure_from_instance_ratios`]
//! (`high = soft - 10pp`, `critical = hard - 5pp`, reproducing the 70/90
//! pair from the 0.80/0.95 instance defaults), so both tracks read from one
//! source of truth instead of drifting apart.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// Default process budget (2 GiB): generous headroom for single-node use,
/// overridable via [`set_process_budget`] at startup.
pub const DEFAULT_PROCESS_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Default high watermark as a percentage of the process budget.
pub const DEFAULT_HIGH_PCT: u64 = 70;

/// Default critical watermark as a percentage of the process budget.
pub const DEFAULT_CRITICAL_PCT: u64 = 90;

/// Gap between the instance soft ratio and the query-side high watermark
/// (fraction of budget). Query accelerations shed before storage admission
/// pressure begins.
pub(crate) const HIGH_MARGIN: f64 = 0.10;

/// Gap between the instance hard ratio and the query-side critical
/// watermark (fraction of budget).
pub(crate) const CRITICAL_MARGIN: f64 = 0.05;

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
    high_pct: AtomicU64,
    critical_pct: AtomicU64,
}

static WATERMARK: OnceLock<Watermark> = OnceLock::new();

fn watermark() -> &'static Watermark {
    WATERMARK.get_or_init(|| Watermark {
        budget_bytes: AtomicU64::new(DEFAULT_PROCESS_BUDGET_BYTES),
        reserved_bytes: AtomicU64::new(0),
        high_pct: AtomicU64::new(DEFAULT_HIGH_PCT),
        critical_pct: AtomicU64::new(DEFAULT_CRITICAL_PCT),
    })
}

/// Override the process-wide memory budget (bytes).
///
/// Later calls shrink/grow the ceiling immediately; current reservations are
/// never revoked, they just may exceed the new budget until released.
pub fn set_process_budget(bytes: u64) {
    watermark().budget_bytes.store(bytes, Ordering::Relaxed);
}

/// Override the pressure watermarks as percentages of the process budget.
///
/// Requires `0 < high < critical <= 100`. Later calls take effect for every
/// subsequent [`pressure`] read; reservations are never revoked.
pub fn set_pressure_watermarks(high_pct: u64, critical_pct: u64) -> Result<(), &'static str> {
    if high_pct == 0 || high_pct >= critical_pct || critical_pct > 100 {
        return Err("pressure watermarks must satisfy 0 < high < critical <= 100");
    }
    let w = watermark();
    w.high_pct.store(high_pct, Ordering::Relaxed);
    w.critical_pct.store(critical_pct, Ordering::Relaxed);
    Ok(())
}

/// Derive the query-side watermarks from the instance memory ratios so both
/// pressure tracks share one source of truth.
///
/// `high = soft - 10pp` and `critical = hard - 5pp` keep query-side
/// degradation strictly ahead of storage admission; with the instance
/// defaults (0.80 / 0.95) this reproduces the historical 70 / 90 pair.
/// Results are clamped into `0 < high < critical <= 100`.
pub fn configure_from_instance_ratios(soft_ratio: f64, hard_ratio: f64) -> Result<(), &'static str> {
    if !soft_ratio.is_finite() || !hard_ratio.is_finite() {
        return Err("instance memory ratios must be finite");
    }
    let high = ((soft_ratio - HIGH_MARGIN) * 100.0).round() as i64;
    let critical = ((hard_ratio - CRITICAL_MARGIN) * 100.0).round() as i64;
    let high = high.clamp(1, 98) as u64;
    let critical = critical.clamp((high + 1) as i64, 99) as u64;
    set_pressure_watermarks(high, critical)
}

/// Reset the pressure watermarks to their compiled-in defaults.
pub fn reset_pressure_watermarks() {
    let w = watermark();
    w.high_pct.store(DEFAULT_HIGH_PCT, Ordering::Relaxed);
    w.critical_pct.store(DEFAULT_CRITICAL_PCT, Ordering::Relaxed);
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
    let high_pct = w.high_pct.load(Ordering::Relaxed);
    let critical_pct = w.critical_pct.load(Ordering::Relaxed);
    // Percentages computed without floats to stay exact near boundaries.
    if used.saturating_mul(100) >= budget.saturating_mul(critical_pct) {
        Pressure::Critical
    } else if used.saturating_mul(100) >= budget.saturating_mul(high_pct) {
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

    #[test]
    fn pressure_watermarks_are_configurable() {
        set_process_budget(1000);
        release(reserved());
        set_pressure_watermarks(40, 60).expect("valid watermark pair");

        try_reserve(300); // 30%
        assert_eq!(pressure(), Pressure::Low);
        try_reserve(150); // 45%
        assert_eq!(pressure(), Pressure::High);
        try_reserve(150); // 60%
        assert_eq!(pressure(), Pressure::Critical);

        release(reserved());
        reset_pressure_watermarks();
        set_process_budget(DEFAULT_PROCESS_BUDGET_BYTES);
    }

    #[test]
    fn pressure_watermarks_reject_invalid_pairs() {
        assert!(set_pressure_watermarks(0, 90).is_err());
        assert!(set_pressure_watermarks(90, 90).is_err());
        assert!(set_pressure_watermarks(95, 90).is_err());
        assert!(set_pressure_watermarks(70, 101).is_err());
        assert!(set_pressure_watermarks(70, 90).is_ok());
        reset_pressure_watermarks();
    }

    #[test]
    fn instance_ratios_reproduce_default_watermarks() {
        configure_from_instance_ratios(0.80, 0.95).expect("instance defaults derive");
        set_process_budget(1000);
        release(reserved());

        try_reserve(690); // 69% < 70%
        assert_eq!(pressure(), Pressure::Low);
        try_reserve(100); // 79%
        assert_eq!(pressure(), Pressure::High);
        try_reserve(150); // 94%
        assert_eq!(pressure(), Pressure::Critical);

        release(reserved());
        reset_pressure_watermarks();
        set_process_budget(DEFAULT_PROCESS_BUDGET_BYTES);
    }
}
