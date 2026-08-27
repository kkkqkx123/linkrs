//! Typed GlobalState / LocalState arenas for per-execution operator state.
//!
//! Per the M4 spec:
//! - **GlobalState**: hash build, global aggregate, sort runs, exchange,
//!   result collector — addressed by [`PhysicalOperatorId`].
//! - **LocalState**: scan cursor, probe state, partial accumulator,
//!   expression workspace — addressed by `(PhysicalOperatorId, TaskId)`.
//!
//! State is kept separate from specs so that [`PhysicalPlan`] and
//! [`OperatorSpec`] remain immutable and cacheable.  State is allocated
//! once per execution instance and indexed via typed arenas rather than
//! inline fields, avoiding `dyn Any` downcast patterns.

use super::operators::state::{
    BlockingState, ExchangeState, FulltextState, GraphState, JoinState, SetState, SinkState,
    SourceState, TxnState, VectorState,
};

/// A task identifier within a fragment execution.
pub type TaskId = usize;

/// Global operator state — lives for the duration of the query and is
/// shared across all tasks/partitions that execute the same operator.
///
/// Examples: hash table for HashJoin, sort run for Sort, exchange
/// result buffer.
#[derive(Debug)]
pub enum GlobalState {
    Source(SourceState),
    Blocking(BlockingState),
    Join(JoinState),
    Graph(GraphState),
    Sink(SinkState),
    Set(SetState),
    Exchange(ExchangeState),
    Fulltext(FulltextState),
    Vector(VectorState),
    Txn(TxnState),
}

/// Local (per-task) operator state — each task or partition gets its own
/// copy.  Lives for the duration of a single task execution.
///
/// Typed per domain, matching the variant structure of [`GlobalState`].
/// The runtime accesses the correct variant via [`PhysicalOperatorId`]
/// + [`TaskId`] lookup — no `dyn Any` downcasts.
///
/// Examples: scan cursor position, probe cursor, partial aggregate
/// accumulator, chunk buffer.
#[derive(Debug)]
pub enum LocalState {
    Source(SourceState),
    Blocking(BlockingState),
    Join(JoinState),
    Graph(GraphState),
    Sink(SinkState),
    Set(SetState),
    Exchange(ExchangeState),
    Fulltext(FulltextState),
    Vector(VectorState),
    Txn(TxnState),
}

// ── Index key types ─────────────────────────────────────────────────────────

/// Key for indexing global state: the physical operator ID and partition.
/// `None` identifies global or non-partitioned state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlobalStateKey(
    pub crate::executor::streaming::plan::types::PhysicalOperatorId,
    pub Option<usize>,
);

/// Key for indexing local state: operator ID + partition ID + task ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalStateKey(
    pub crate::executor::streaming::plan::types::PhysicalOperatorId,
    pub Option<usize>,
    pub TaskId,
);

// ── GlobalStateArena ────────────────────────────────────────────────────────

/// Typed arena of global operator state objects.
///
/// Addressed by [`PhysicalOperatorId`].  Created once per execution
/// instance and populated during operator tree materialization.
#[derive(Debug, Default)]
pub struct GlobalStateArena {
    states: std::collections::HashMap<GlobalStateKey, GlobalState>,
}

impl GlobalStateArena {
    pub fn new() -> Self {
        Self {
            states: std::collections::HashMap::new(),
        }
    }

    /// Insert global state for an operator.
    pub fn insert(&mut self, key: GlobalStateKey, state: GlobalState) {
        self.states.insert(key, state);
    }

    /// Get a reference to global state.
    pub fn get(&self, key: &GlobalStateKey) -> Option<&GlobalState> {
        self.states.get(key)
    }

    /// Get a mutable reference to global state.
    pub fn get_mut(&mut self, key: &GlobalStateKey) -> Option<&mut GlobalState> {
        self.states.get_mut(key)
    }

    /// Remove and return global state (for cleanup).
    pub fn remove(&mut self, key: &GlobalStateKey) -> Option<GlobalState> {
        self.states.remove(key)
    }

    /// Clear all state.
    pub fn clear(&mut self) {
        self.states.clear();
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

// ── LocalStateArena ─────────────────────────────────────────────────────────

/// Typed arena of per-task local operator state.
///
/// Addressed by [`PhysicalOperatorId`] + [`TaskId`].  Created per
/// execution instance, with one entry per (operator, task) pair.
#[derive(Debug, Default)]
pub struct LocalStateArena {
    states: std::collections::HashMap<LocalStateKey, LocalState>,
}

impl LocalStateArena {
    pub fn new() -> Self {
        Self {
            states: std::collections::HashMap::new(),
        }
    }

    /// Insert local state for an operator + task.
    pub fn insert(&mut self, key: LocalStateKey, state: LocalState) {
        self.states.insert(key, state);
    }

    /// Get a reference to local state.
    pub fn get(&self, key: &LocalStateKey) -> Option<&LocalState> {
        self.states.get(key)
    }

    /// Get a mutable reference to local state.
    pub fn get_mut(&mut self, key: &LocalStateKey) -> Option<&mut LocalState> {
        self.states.get_mut(key)
    }

    /// Remove and return local state (for cleanup).
    pub fn remove(&mut self, key: &LocalStateKey) -> Option<LocalState> {
        self.states.remove(key)
    }

    /// Clear all state for a given operator (all tasks).
    pub fn clear_operator(
        &mut self,
        op_id: &crate::executor::streaming::plan::types::PhysicalOperatorId,
    ) {
        self.states.retain(|k, _| k.0 != *op_id);
    }

    /// Clear all state.
    pub fn clear(&mut self) {
        self.states.clear();
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

// ── StateArenaSet (combined) ────────────────────────────────────────────────

/// Combined set of global and local state arenas for an execution instance.
#[derive(Debug, Default)]
pub struct StateArenaSet {
    pub global: GlobalStateArena,
    pub local: LocalStateArena,
}

impl StateArenaSet {
    pub fn new() -> Self {
        Self {
            global: GlobalStateArena::new(),
            local: LocalStateArena::new(),
        }
    }

    /// Clear all state (both global and local).
    pub fn clear(&mut self) {
        self.global.clear();
        self.local.clear();
    }
}
