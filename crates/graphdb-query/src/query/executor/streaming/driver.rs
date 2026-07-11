//! ExecutorDriver: wraps operator open/next/close with runtime context.
//!
//! Phase 4: profiling (timing and per-operator row counts) moved to the
//! operator dispatch in executor.rs.  ExecutorDriver retains only the
//! total_rows counting via profile_add_rows, plus cancel-check and
//! resource-release orchestration.
//!
//! Future: remove ExecutorDriver entirely once all paths go through
//! the operator dispatch in executor.rs.

use std::sync::Arc;

use super::chunk::DataChunk;
use super::executor::StreamingExecutor;

use super::runtime::ExecutionRuntime;
use crate::core::error::QueryError;

/// Driver that wraps operator lifecycle with runtime context.
///
/// Phase 3: operator dispatch in executor.rs carries its own cancel
/// checking + profiling.  The driver is retained for engine.rs paths
/// that have not yet migrated.
#[derive(Debug)]
pub struct ExecutorDriver {
    runtime: Arc<ExecutionRuntime>,
}

impl ExecutorDriver {
    pub fn new(runtime: Arc<ExecutionRuntime>) -> Self {
        Self { runtime }
    }

    pub fn runtime(&self) -> &Arc<ExecutionRuntime> {
        &self.runtime
    }

    /// Open an operator with runtime cancel check.
    pub fn open(&self, executor: &mut StreamingExecutor) -> Result<(), QueryError> {
        self.runtime.ensure_not_cancelled()?;
        executor.open()?;
        Ok(())
    }

    /// Pull the next chunk and record total output rows.
    pub fn next(&self, executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        self.runtime.ensure_not_cancelled()?;
        let result = executor.advance()?;
        if let Some(ref chunk) = result {
            self.runtime.profile_add_rows(chunk.len() as u64);
        }
        Ok(result)
    }

    /// Close an operator. Query-owned runtime resources are released by the
    /// engine's final cleanup path, not by individual operators.
    pub fn close(&self, executor: &mut StreamingExecutor) -> Result<(), QueryError> {
        executor.close()
    }

    // ── Profile helpers ──

    /// Record peak memory for an operator from a MemoryTracker.
    pub fn record_peak_memory(&self, executor: &StreamingExecutor, peak_bytes: u64) {
        let key = executor.profile_key();
        let mut profile = self.runtime.profile().lock();
        if let Some(entry) = profile.operators.get_mut(&key) {
            entry.peak_memory = entry.peak_memory.max(peak_bytes);
        }
    }

    /// Convenience: check cancel inside a long-running operator loop.
    pub fn check_cancel(&self) -> Result<(), QueryError> {
        self.runtime.ensure_not_cancelled()
    }
}
