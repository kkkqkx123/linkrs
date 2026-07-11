//! Per-query memory budget for blocking operators.
//!
//! Replaces the hardcoded 100MB limit with a configurable budget
//! passed through ExecutionContext.

use crate::core::error::QueryError;
use crate::core::Value;
use std::sync::Arc;

/// Memory budget for a single query execution.
///
/// Each blocking operator should call `try_reserve` before
/// buffering data. When the budget is exhausted the operator
/// returns an error, preventing OOM.
#[derive(Debug, Clone)]
pub struct MemoryBudget {
    /// Maximum bytes this query may use in blocking operators.
    pub max_bytes: usize,
    /// Number of bytes already accounted for.
    allocated: Arc<std::sync::atomic::AtomicUsize>,
}

impl MemoryBudget {
    /// Default per-query budget (512 MB).
    pub const DEFAULT_MAX: usize = 512 * 1024 * 1024;

    /// Create a new budget with `max_bytes` limit.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            allocated: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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

    /// Release `bytes` from the budget (called when data is freed).
    pub fn release(&self, bytes: usize) {
        self.allocated.fetch_sub(bytes, std::sync::atomic::Ordering::Relaxed);
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
}

impl MemoryTracker {
    /// Create a new tracker backed by `budget`.
    pub fn new(budget: MemoryBudget) -> Self {
        Self {
            budget,
            peak_bytes: 0,
            current_bytes: 0,
        }
    }

    /// Reserve `bytes` additional memory through the shared budget.
    ///
    /// Updates the per-operator peak tracker and returns an error when
    /// the global budget is exceeded.
    pub fn try_reserve(&mut self, bytes: usize) -> Result<(), QueryError> {
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

/// Trait for operators that can spill intermediate data to disk.
///
/// Each blocking operator that may exceed the memory budget should implement
/// this trait. The initial implementation may return `Err(QueryError::execution("spill not implemented"))`.
pub trait Spillable {
    /// Spill in-memory data to disk to free memory.
    fn spill_to_disk(&mut self) -> Result<(), QueryError>;

    /// Number of bytes currently spilled to disk.
    fn spilled_size(&self) -> u64;

    /// Whether this operator has spilled data to disk.
    fn has_spilled(&self) -> bool {
        self.spilled_size() > 0
    }
}
