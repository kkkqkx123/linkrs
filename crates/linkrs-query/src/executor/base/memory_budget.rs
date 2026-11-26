//! Per-query memory budget for blocking operators.
//!
//! Replaces the hardcoded 100MB limit with a configurable budget
//! passed through ExecutionContext.

use linkrs_core::error::QueryError;
use linkrs_core::Value;
use std::sync::Arc;

static BUDGET_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How an operator starts buffering before its spill fallback engages.
///
/// The physical plan supplies this hint; the request budget decides whether
/// the hint is applied at materialization time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpillMode {
    /// Operator does not spill and tracks memory against the shared budget.
    InMemory,
    /// Operator buffers normally and spills when the shared budget is hit.
    #[default]
    OnDemand,
    /// Operator caps its own buffer first so the existing spill path starts
    /// with the first rows instead of after the whole query budget is spent.
    ExternalFromStart { local_limit: usize },
}

impl SpillMode {
    /// Choose a start mode from the plan's estimated working set and the
    /// operator's share of the query budget.
    pub fn from_working_set(working_set_bytes: u64, budget_share_bytes: usize) -> Self {
        if budget_share_bytes > 0 && working_set_bytes > budget_share_bytes as u64 {
            let local_limit = (budget_share_bytes / 16).clamp(4096, 64 * 1024);
            Self::ExternalFromStart { local_limit }
        } else {
            Self::OnDemand
        }
    }
}

/// Memory budget for a single query execution.
///
/// Each blocking operator should call `try_reserve` before
/// buffering data. When the budget is exhausted the operator
/// returns an error, preventing OOM.
#[derive(Clone)]
pub struct MemoryBudget {
    /// Maximum bytes this query may use in blocking operators.
    pub max_bytes: usize,
    /// Number of bytes already accounted for.
    allocated: Arc<std::sync::atomic::AtomicUsize>,
    /// Unique ID for debugging budget leaks.
    pub(crate) id: usize,
}

impl std::fmt::Debug for MemoryBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryBudget")
            .field("max_bytes", &self.max_bytes)
            .field(
                "allocated",
                &self.allocated.load(std::sync::atomic::Ordering::Relaxed),
            )
            .field("id", &self.id)
            .finish()
    }
}

impl MemoryBudget {
    /// Default per-query budget (512 MB).
    pub const DEFAULT_MAX: usize = 512 * 1024 * 1024;

    /// Create a new budget with `max_bytes` limit.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            allocated: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            id: BUDGET_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Create a budget with the default limit.
    pub fn default_budget() -> Self {
        Self::new(Self::DEFAULT_MAX)
    }

    /// Try to reserve `bytes` additional memory.
    ///
    /// Uses compare-and-swap so that the counter is only updated when the
    /// new total does not exceed the budget.  On overflow or over-budget
    /// the counter is left unchanged (no observable side-effect).
    ///
    /// Returns `Ok(true)` on success, `Err` when the budget would be exceeded
    /// or the request causes integer overflow.
    pub fn try_reserve(&self, bytes: usize) -> Result<bool, QueryError> {
        let mut prev = self.allocated.load(std::sync::atomic::Ordering::Relaxed);
        loop {
            let total = prev.checked_add(bytes).ok_or_else(|| {
                QueryError::execution(format!(
                    "Memory budget overflow: request {} bytes overflows usize",
                    bytes,
                ))
            })?;
            if total > self.max_bytes {
                return Err(QueryError::execution(format!(
                    "Memory budget exceeded: request {} bytes, total {} > budget {} bytes",
                    bytes, total, self.max_bytes,
                )));
            }
            match self.allocated.compare_exchange_weak(
                prev,
                total,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(true),
                Err(current) => prev = current,
            }
        }
    }

    /// Release `bytes` from the budget, clamped to the current reservation.
    pub fn release(&self, bytes: usize) {
        let mut prev = self.allocated.load(std::sync::atomic::Ordering::Relaxed);
        loop {
            let next = prev.saturating_sub(bytes);
            match self.allocated.compare_exchange_weak(
                prev,
                next,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(current) => prev = current,
            }
        }
    }

    /// Current allocated bytes.
    pub fn current(&self) -> usize {
        self.allocated.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Rough estimate of the memory used by a single row (Vec<Value> heap + Value deep data).
    pub fn estimate_row_memory(row: &[Value]) -> usize {
        row.iter().map(|v| v.estimated_size()).sum()
    }

    /// Rough estimate of the memory used by a slice of rows.
    pub fn estimate_rows_memory(rows: &[Vec<Value>]) -> usize {
        rows.iter().map(|row| Self::estimate_row_memory(row)).sum()
    }
}

/// Per-operator memory tracker wrapping a shared `MemoryBudget`.
///
/// Each blocking operator should hold its own `MemoryTracker` and call
/// `try_reserve` before buffering data. The tracker records per-operator
/// peak memory so it can later be reported to the `ProfileCollector`.
#[derive(Debug, Clone)]
pub struct MemoryTracker {
    budget: MemoryBudget,
    peak_bytes: usize,
    current_bytes: usize,
    local_limit: usize,
}

impl MemoryTracker {
    /// Create a new tracker backed by `budget`.
    pub fn new(budget: MemoryBudget) -> Self {
        Self::with_local_limit(budget, usize::MAX)
    }

    /// Create a tracker with an additional per-operator cap.
    ///
    /// The shared budget stays authoritative; the local cap only makes the
    /// operator reach its existing spill path earlier.
    pub fn with_local_limit(budget: MemoryBudget, local_limit: usize) -> Self {
        Self {
            budget,
            peak_bytes: 0,
            current_bytes: 0,
            local_limit,
        }
    }

    /// Create the tracker described by a physical plan's spill mode.
    pub fn for_mode(mode: SpillMode, budget: MemoryBudget) -> Self {
        match mode {
            SpillMode::ExternalFromStart { local_limit } => {
                Self::with_local_limit(budget, local_limit)
            }
            SpillMode::InMemory | SpillMode::OnDemand => Self::new(budget),
        }
    }

    /// Reserve `bytes` additional memory through the shared budget.
    ///
    /// Updates the per-operator peak tracker and returns an error when
    /// the global budget is exceeded.
    pub fn try_reserve(&mut self, bytes: usize) -> Result<(), QueryError> {
        // A single large row must be bufferable; otherwise an external-start
        // operator could fail before it has anything to spill. The next row
        // will cross the cap and trigger the spill path.
        if self.current_bytes > 0 && self.current_bytes.saturating_add(bytes) > self.local_limit {
            return Err(QueryError::execution(format!(
                "Memory budget exceeded: operator local limit {} bytes exceeded",
                self.local_limit,
            )));
        }
        self.budget.try_reserve(bytes)?;
        self.current_bytes += bytes;
        self.peak_bytes = self.peak_bytes.max(self.current_bytes);
        Ok(())
    }

    /// Release `bytes` from both the global budget and this tracker.
    pub fn release(&mut self, bytes: usize) {
        self.budget.release(bytes);
        self.current_bytes = self.current_bytes.saturating_sub(bytes);
    }

    /// Peak memory observed by this tracker.
    pub fn peak(&self) -> usize {
        self.peak_bytes
    }

    /// Current tracked bytes.
    pub fn current(&self) -> usize {
        self.current_bytes
    }

    /// Convenience: reserve memory for a single row (includes Value deep heap data).
    pub fn try_reserve_row(&mut self, row: &[Value]) -> Result<(), QueryError> {
        let mem = MemoryBudget::estimate_row_memory(row);
        self.try_reserve(mem)
    }

    /// Convenience: reserve memory for many rows.
    pub fn try_reserve_rows(&mut self, rows: &[Vec<Value>]) -> Result<(), QueryError> {
        let mem = MemoryBudget::estimate_rows_memory(rows);
        self.try_reserve(mem)
    }

    /// Release all tracked bytes and reset the tracker to zero.
    ///
    /// Called by operator `close()` instead of re-estimating current state.
    pub fn reset(&mut self) {
        self.budget.release(self.current_bytes);
        self.current_bytes = 0;
        // peak_bytes intentionally preserved for profile reporting
    }
}

/// RAII guard that releases reserved memory when dropped.
///
/// Created by [`MemoryBudget::reserve`]. The caller can `forget()` this
/// guard to transfer release responsibility to an explicit `release()` call.
#[derive(Debug)]
pub struct MemoryReservation {
    budget: MemoryBudget,
    bytes: usize,
}

/// Scoped reservation associated with one [`MemoryTracker`].
///
/// Unlike [`MemoryReservation`], dropping this guard updates both the
/// query-wide budget and the per-operator tracker. The mutable borrow keeps
/// the tracker from being reset while the reservation is still outstanding.
#[derive(Debug)]
pub struct MemoryTrackerReservation<'a> {
    tracker: &'a mut MemoryTracker,
    bytes: usize,
}

impl MemoryReservation {
    fn new(budget: MemoryBudget, bytes: usize) -> Self {
        Self { budget, bytes }
    }

    /// Forget the reservation — the memory is not released on drop.
    ///
    /// Use when the caller takes over release responsibility (e.g. after
    /// transferring ownership of the allocated data).
    pub fn forget(mut self) {
        self.bytes = 0;
    }

    /// The amount of memory currently reserved by this guard.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for MemoryReservation {
    fn drop(&mut self) {
        if self.bytes > 0 {
            self.budget.release(self.bytes);
        }
    }
}

impl Drop for MemoryTrackerReservation<'_> {
    fn drop(&mut self) {
        if self.bytes > 0 {
            self.tracker.release(self.bytes);
        }
    }
}

impl MemoryBudget {
    /// Reserve `bytes` and return a [`MemoryReservation`] guard.
    ///
    /// The guard releases the memory on drop.  This is the recommended
    /// RAII alternative to the raw `try_reserve` / `release` pair.
    pub fn reserve(&self, bytes: usize) -> Result<MemoryReservation, QueryError> {
        self.try_reserve(bytes)?;
        Ok(MemoryReservation::new(self.clone(), bytes))
    }
}

impl MemoryTracker {
    /// Reserve `bytes` through the shared budget and return a scoped
    /// reservation guard.  Unlike the raw `try_reserve` the guard
    /// automatically releases on drop, protecting against leaks.
    pub fn reserve_guarded(
        &mut self,
        bytes: usize,
    ) -> Result<MemoryTrackerReservation<'_>, QueryError> {
        self.try_reserve(bytes)?;
        Ok(MemoryTrackerReservation {
            tracker: self,
            bytes,
        })
    }
}

/// Trait for operators that can spill intermediate data to disk.
///
/// Each blocking operator that may exceed the memory budget implements
/// spill by partitioning buffered rows to managed spill files.
pub trait Spillable {
    /// Spill in-memory data to disk to free memory.
    fn spill_to_disk(&mut self) -> Result<(), QueryError>;

    /// Number of bytes currently spilled to disk.
    fn spilled_size(&self) -> u64;

    /// Number of discrete spill operations (runs/files) written to disk.
    fn spill_count(&self) -> u64 {
        0
    }

    /// Number of logical rows written to spill runs.
    fn spilled_rows(&self) -> u64 {
        0
    }

    /// Whether this operator has spilled data to disk.
    fn has_spilled(&self) -> bool {
        self.spilled_size() > 0
    }

    /// Spill data if not already spilled, returning whether any data was spilled.
    fn try_spill(&mut self) -> Result<bool, QueryError> {
        if !self.has_spilled() {
            self.spill_to_disk()?;
        }
        Ok(self.has_spilled())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_saturates_at_zero() {
        let budget = MemoryBudget::new(64);
        assert!(budget.try_reserve(32).expect("reserve memory"));
        budget.release(64);
        assert_eq!(budget.current(), 0);
        budget.release(16);
        assert_eq!(budget.current(), 0);
    }

    #[test]
    fn tracked_reservation_releases_budget_and_tracker() {
        let budget = MemoryBudget::new(64);
        let mut tracker = MemoryTracker::new(budget.clone());

        {
            let reservation = tracker.reserve_guarded(32).expect("reserve memory");
            assert_eq!(reservation.bytes, 32);
        }

        assert_eq!(budget.current(), 0);
        assert_eq!(tracker.current(), 0);
        assert_eq!(tracker.peak(), 32);
    }
}

#[cfg(test)]
mod spill_mode_tests {
    use super::*;

    #[test]
    fn local_limit_forces_spill_before_shared_budget() {
        let budget = MemoryBudget::new(1024);
        let mut tracker = MemoryTracker::for_mode(
            SpillMode::ExternalFromStart { local_limit: 32 },
            budget.clone(),
        );
        tracker.try_reserve(16).expect("first reservation");
        assert!(tracker.try_reserve(32).is_err());
        assert_eq!(tracker.current(), 16);
        assert_eq!(budget.current(), 16);
        tracker.reset();
        assert_eq!(budget.current(), 0);
    }

    #[test]
    fn single_row_may_exceed_local_limit() {
        let mut tracker = MemoryTracker::with_local_limit(MemoryBudget::new(1024), 1);
        tracker.try_reserve(64).expect("single oversized row");
        assert_eq!(tracker.current(), 64);
        assert!(tracker.try_reserve(1).is_err());
    }

    #[test]
    fn working_set_above_budget_share_starts_external() {
        assert_eq!(
            SpillMode::from_working_set(1024, 128),
            SpillMode::ExternalFromStart { local_limit: 4096 }
        );
        assert_eq!(SpillMode::from_working_set(64, 128), SpillMode::OnDemand);
        assert_eq!(SpillMode::from_working_set(1024, 0), SpillMode::OnDemand);
    }
}
