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
