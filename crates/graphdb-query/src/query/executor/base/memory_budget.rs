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
    /// Returns `Ok(true)` if within budget, `Ok(false)` if
    /// already over budget but does not error (caller decides
    /// what to do), or an error if the budget would be exceeded.
    pub fn try_reserve(&self, bytes: usize) -> Result<bool, QueryError> {
        let prev = self.allocated.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
        let total = prev + bytes;
        if total > self.max_bytes {
            Err(QueryError::execution(format!(
                "Memory budget exceeded: {} > {} bytes",
                total, self.max_bytes
            )))
        } else {
            Ok(true)
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

    /// Rough estimate of the memory used by a slice of rows.
    pub fn estimate_rows_memory(rows: &[Vec<Value>]) -> usize {
        rows.iter()
            .map(|row| row.capacity() * std::mem::size_of::<Value>())
            .sum()
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

    /// Convenience: reserve memory for a single row estimate.
    pub fn try_reserve_row(&mut self, row: &[Value]) -> Result<(), QueryError> {
        let mem = row.len() * std::mem::size_of::<Value>();
        self.try_reserve(mem)
    }

    /// Convenience: reserve memory for many rows estimate.
    pub fn try_reserve_rows(&mut self, rows: &[Vec<Value>]) -> Result<(), QueryError> {
        let mem = MemoryBudget::estimate_rows_memory(rows);
        self.try_reserve(mem)
    }

}
