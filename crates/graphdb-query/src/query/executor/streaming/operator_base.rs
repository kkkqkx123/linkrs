use std::sync::Arc;

use super::runtime::{ExecutionRuntime, OperatorProfileKey};

/// Explicit operator lifecycle state machine.
///
/// # Transitions
/// - `New` → `Opened`: on successful `open()`
/// - `Opened` → `Exhausted`: when `advance()` returns `None`
/// - `Opened` / `Exhausted` → `Stopped`: when consumer calls `stop()` early
/// - Any → `Failed`: unrecoverable error (set during error cleanup)
/// - Any non-terminal → `Closed`: after `close()` completes (idempotent)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorLifecycle {
    /// Before `open()` has been called.
    New,
    /// After successful `open()`; can produce chunks via `advance()`.
    Opened,
    /// All data has been consumed (`advance()` returned `None`).
    Exhausted,
    /// Consumer signalled early termination via `stop()`.
    Stopped,
    /// Unrecoverable error during execution.
    Failed,
    /// Resources released. Terminal state.
    Closed,
}

impl OperatorLifecycle {
    /// Whether the operator was opened and not yet stopped/failed/closed.
    /// `Exhausted` is included so the close guard works after natural exhaustion.
    pub fn is_opened(self) -> bool {
        matches!(self, Self::Opened | Self::Exhausted)
    }

    /// Whether the operator is in a state that allows closing.
    /// Everything except `Closed` needs cleanup.
    pub fn can_close(self) -> bool {
        !matches!(self, Self::Closed)
    }

    pub fn mark_opened(&mut self) {
        debug_assert!(
            matches!(*self, Self::New),
            "mark_opened from {:?} (expected New)",
            self
        );
        *self = Self::Opened;
    }

    pub fn mark_exhausted(&mut self) {
        if matches!(*self, Self::Opened) {
            *self = Self::Exhausted;
        }
    }

    pub fn mark_stopped(&mut self) {
        if matches!(*self, Self::Opened | Self::Exhausted) {
            *self = Self::Stopped;
        }
    }

    pub fn mark_failed(&mut self) {
        *self = Self::Failed;
    }

    pub fn mark_closed(&mut self) {
        *self = Self::Closed;
    }
}

#[derive(Debug)]
pub struct OperatorBase {
    pub plan_node_id: i64,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub lifecycle: OperatorLifecycle,
    /// Whether this operator produces global (merged) output.
    /// Local operators process one partition at a time.
    pub is_global: bool,
    /// Local partition that owns this operator. `None` denotes a global or
    /// non-partitioned operator.
    pub partition_id: Option<usize>,
    /// Rows per chunk when this operator produces output.
    /// Source operators use this value directly; unary/blocking operators
    /// pass through whatever they receive from their child.
    pub chunk_size: usize,
}

impl OperatorBase {
    pub fn new(plan_node_id: i64) -> Self {
        Self {
            plan_node_id,
            runtime: None,
            lifecycle: OperatorLifecycle::New,
            is_global: false,
            partition_id: None,
            chunk_size: 1024,
        }
    }

    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        self
    }

    pub fn with_runtime(mut self, rt: Option<Arc<ExecutionRuntime>>) -> Self {
        self.runtime = rt;
        self
    }

    pub fn with_global(mut self, is_global: bool) -> Self {
        self.is_global = is_global;
        self
    }

    pub fn with_partition(mut self, partition_id: usize) -> Self {
        self.partition_id = Some(partition_id);
        self
    }

    pub fn profile_key(&self) -> OperatorProfileKey {
        OperatorProfileKey::new(self.plan_node_id, self.partition_id)
    }

    pub fn ensure_not_cancelled(&self) -> Result<(), crate::core::error::QueryError> {
        if let Some(rt) = &self.runtime {
            rt.ensure_not_cancelled()
        } else {
            Ok(())
        }
    }

    pub fn record_profile_rows(&self, count: u64) {
        if let Some(rt) = &self.runtime {
            let mut profile = rt.profile().lock();
            if let Some(entry) = profile.operators.get_mut(&self.profile_key()) {
                entry.output_rows += count;
            }
        }
    }

    pub fn register_resource<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if let Some(rt) = &self.runtime {
            rt.on_cleanup(f);
        }
    }
}
