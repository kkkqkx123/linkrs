use std::sync::Arc;

use parking_lot::Mutex;

use super::super::runtime::{ExecutionRuntime, OperatorProfileKey};
use super::super::spill::SpillManager;
use super::super::state::{
    GlobalState, GlobalStateKey, LocalState, LocalStateKey, StateArenaSet, TaskId,
};
use crate::executor::streaming::plan::types::PhysicalOperatorId;
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::Arena;
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

    pub fn is_exhausted(self) -> bool {
        matches!(self, Self::Exhausted)
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
    /// Unique physical identity used for this operator's mutable runtime
    /// state. Production materialization must provide the arena-assigned ID.
    pub physical_operator_id: PhysicalOperatorId,
    /// Immutable output layout supplied by the physical plan.  Operators use
    /// this for chunks they construct themselves, including empty-result
    /// paths, rather than inferring schema from data rows.
    pub output_layout: Arc<SlotLayout>,
    /// Set when a `reset()` call degraded to the transitional `close + open`
    /// fallback for this operator (see `StreamingExecutor::reset`). Surfaced
    /// in EXPLAIN as `reset:fallback` so un-resettable paths stay auditable.
    pub reset_used_fallback: bool,
}

impl OperatorBase {
    pub fn new(plan_node_id: i64) -> Self {
        Self {
            plan_node_id,
            runtime: None,
            lifecycle: OperatorLifecycle::New,
            is_global: false,
            partition_id: None,
            chunk_size: 2048,
            physical_operator_id: PhysicalOperatorId(plan_node_id.unsigned_abs() as usize),
            output_layout: Arc::new(SlotLayout::new(Vec::new())),
            reset_used_fallback: false,
        }
    }

    /// Lock and return the partition-local operator state arena from the runtime.
    ///
    /// Each partition gets its own arena so parallel workers don't contend.
    /// Panics if no runtime is attached.
    pub fn state_arena(&self) -> parking_lot::MutexGuard<'_, StateArenaSet> {
        self.runtime
            .as_ref()
            .expect("runtime required")
            .state_arena_for(self.partition_id)
            .lock()
    }

    /// Override the legacy tree-node identity with the arena-assigned physical
    /// operator identity. This is required by production materialization.
    pub fn with_physical_operator_id(mut self, operator_id: PhysicalOperatorId) -> Self {
        self.physical_operator_id = operator_id;
        self
    }

    /// Attach the output contract layout during physical-plan materialization.
    pub fn with_output_layout(mut self, output_layout: Arc<SlotLayout>) -> Self {
        self.output_layout = output_layout;
        self
    }

    /// Return the [`GlobalStateKey`] for this operator's slot.
    pub fn state_key(&self) -> GlobalStateKey {
        GlobalStateKey(self.physical_operator_id, self.partition_id)
    }

    /// Take the [`GlobalState`] out of the arena (for cleanup).
    pub fn take_state(&mut self) -> Option<GlobalState> {
        let rt = self.runtime.as_ref()?;
        let key = self.state_key();
        rt.state_arena_for(self.partition_id)
            .lock()
            .global
            .remove(&key)
    }

    /// Insert a [`GlobalState`] into this operator's slot in the runtime arena.
    ///
    /// No-op when no runtime is attached (e.g. in unit tests that construct
    /// executor trees directly without an [`ExecutionRuntime`]).
    pub fn insert_state(&mut self, state: GlobalState) {
        let Some(rt) = self.runtime.as_ref() else {
            return;
        };
        let key = self.state_key();
        rt.state_arena_for(self.partition_id)
            .lock()
            .global
            .insert(key, state);
    }

    // ── Local state access (per-task) ──

    /// Return the [`LocalStateKey`] for this operator + task.
    pub fn local_state_key(&self, task_id: TaskId) -> LocalStateKey {
        LocalStateKey(self.physical_operator_id, self.partition_id, task_id)
    }

    /// Insert a [`LocalState`] into the arena for a given task.
    pub fn insert_local_state(&mut self, task_id: TaskId, state: LocalState) {
        let Some(rt) = self.runtime.as_ref() else {
            return;
        };
        let key = self.local_state_key(task_id);
        rt.state_arena_for(self.partition_id)
            .lock()
            .local
            .insert(key, state);
    }

    /// Take a [`LocalState`] out of the arena (for cleanup).
    pub fn take_local_state(&mut self, task_id: TaskId) -> Option<LocalState> {
        let rt = self.runtime.as_ref()?;
        let key = self.local_state_key(task_id);
        rt.state_arena_for(self.partition_id)
            .lock()
            .local
            .remove(&key)
    }

    /// Access local state for a given task via a closure.
    ///
    /// The closure receives `Option<&mut LocalState>` scoped within the
    /// state arena lock.  Returns the closure's result.
    pub fn with_local_state<T, F>(&self, task_id: TaskId, f: F) -> Option<T>
    where
        F: FnOnce(&mut LocalState) -> T,
    {
        let rt = self.runtime.as_ref()?;
        let key = self.local_state_key(task_id);
        let mut arena = rt.state_arena_for(self.partition_id).lock();
        arena.local.get_mut(&key).map(f)
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
        OperatorProfileKey::new(self.physical_operator_id, self.partition_id)
    }

    pub fn ensure_not_cancelled(&self) -> Result<(), graphdb_core::error::QueryError> {
        if let Some(rt) = &self.runtime {
            rt.ensure_not_cancelled()
        } else {
            Ok(())
        }
    }

    pub fn record_profile_rows(&self, count: u64) {
        if let Some(rt) = &self.runtime {
            if let Some(entry) = rt.profile().get_entry(&self.profile_key()) {
                entry
                    .output_rows
                    .fetch_add(count, std::sync::atomic::Ordering::Relaxed);
                entry
                    .advance_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

    /// Convenience accessor for the spill manager from the runtime.
    pub fn spill_manager(&self) -> Option<Arc<SpillManager>> {
        self.runtime.as_ref().and_then(|rt| rt.get_spill_manager())
    }

    /// Return a reference to the bumpalo arena, if configured.
    pub fn arena(&self) -> Option<&Arc<Mutex<Arena>>> {
        self.runtime.as_ref().and_then(|rt| rt.arena())
    }

    /// Reset the lifecycle to `New` so a `close + open` sequence can run
    /// again on the same operator instance (reset fallback path).
    pub fn mark_new(&mut self) {
        self.lifecycle = OperatorLifecycle::New;
    }
}

#[cfg(test)]
mod tests {
    use super::OperatorBase;
    use crate::executor::streaming::plan::types::PhysicalOperatorId;

    #[test]
    fn state_keys_include_physical_operator_and_partition() {
        let global = OperatorBase::new(10).with_physical_operator_id(PhysicalOperatorId(7));
        let partition_zero = OperatorBase::new(10)
            .with_physical_operator_id(PhysicalOperatorId(7))
            .with_partition(0);
        let partition_one = OperatorBase::new(10)
            .with_physical_operator_id(PhysicalOperatorId(7))
            .with_partition(1);

        assert_ne!(global.state_key(), partition_zero.state_key());
        assert_ne!(partition_zero.state_key(), partition_one.state_key());
        assert_ne!(
            partition_zero.local_state_key(0),
            partition_zero.local_state_key(1)
        );
    }
}
