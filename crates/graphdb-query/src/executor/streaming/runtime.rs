//! Per-query execution runtime.
//!
//! Split by concern:
//! - [`columnar_stats`] — columnar fast-path counters and PROFILE snapshots
//! - [`profile`] — per-operator profiling ([`OperatorProfile`],
//!   [`ProfileBoard`], [`ProfileCollector`])
//! - [`resources`] — cleanup ownership ([`ResourceOwner`]) and the
//!   [`QueryFinishGuard`] RAII lifecycle guard
//! - this module — [`ExecutionRuntime`], the per-query composition root

pub mod columnar_stats;
pub mod profile;
pub mod resources;

pub use columnar_stats::{
    ColumnarStats, ColumnarStatsSnapshot, D1_EVAL_THRESHOLD, D1_TYPED_RATE_THRESHOLD,
    SELECTION_BOUNDARY_OPS,
};
pub use profile::{
    OperatorProfile, OperatorProfileKey, ProfileBoard, ProfileCollector, ProfileEntry,
};
pub use resources::{QueryFinishGuard, ResourceOwner};

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use parking_lot::RwLock;

use super::query_registry::{CancelToken, QueryId, QueryRegistry};
use super::spill::SpillManager;
use super::state::StateArenaSet;
use super::transaction_scope::{CancelReason, SessionTransactionController, TransactionScope};
use crate::executor::base::MemoryBudget;
use crate::executor::streaming::pool::TaskScheduler;
use crate::optimizer::stats::feedback::history::QueryFeedbackHistory;
use crate::query_manager::QueryManager;
use crate::storage::QueryStorage;
use graphdb_core::error::QueryError;
use graphdb_core::Arena;
use graphdb_core::Value;

/// Query identity information
#[derive(Debug, Clone, Default)]
pub struct QueryIdentity {
    pub query_id: u64,
    pub session_id: Option<String>,
    pub space_name: Option<String>,
}

/// Per-query execution runtime shared across all operators.
///
/// Centralises cancellation, memory tracking, profiling, resource
/// lifecycle, and query-registration so that operators do not each
/// carry ad-hoc context.
///
/// unified cancellation via [`CancelToken`]
/// has been removed; operators check [`CancelToken::is_cancelled`].
#[derive(Debug)]
pub struct ExecutionRuntime {
    /// Query identity (behind Mutex for write-once from the API layer).
    query_id: parking_lot::Mutex<QueryIdentity>,
    /// typed cancellation token with reason tracking (single source).
    /// Behind a `Mutex` because the token is adopted after the runtime is
    /// shared with the executor tree (`Arc::get_mut` is unusable once the
    /// tree holds clones); `&self` mutation keeps the registry-None path
    /// wired without `&mut` access.
    cancel_token_v2: parking_lot::Mutex<CancelToken>,
    /// Optional deadline; the query is cancelled after this instant.
    deadline: Option<Instant>,
    /// Per-query memory budget for blocking operators.
    pub memory_budget: MemoryBudget,
    /// Profile board with atomic counters for lock-free hot-path recording.
    profile: Arc<ProfileBoard>,
    /// Resource owner for cleanup of cursors, temp files, etc.
    resource_owner: Arc<Mutex<ResourceOwner>>,
    /// Optional reference to the global QueryManager for KILL QUERY, finish
    /// tracking, and progress forwarding. Behind a `Mutex` for interior
    /// mutability so the assembly can attach it after the runtime is shared
    /// with the executor tree (same pattern as `query_registry`).
    query_manager: parking_lot::Mutex<Option<Arc<QueryManager>>>,
    /// Row cadence for query-progress notifications. Zero disables emission.
    /// Checked with a single relaxed atomic load on the row-recording path,
    /// so the default zero keeps the hot path allocation- and lock-free.
    progress_rows_interval: AtomicU64,
    /// Highest processed-row watermark already reported to progress
    /// observers. Prevents duplicate notifications when several chunks land
    /// inside the same interval bucket.
    progress_last_emitted: AtomicU64,
    /// Session id reported in progress notifications. Set by the assembly
    /// that attaches the `QueryManager`; defaults to zero.
    progress_session_id: AtomicI64,
    /// Query id (in the `QueryManager` id space) reported in progress
    /// notifications. Negative means unset and suppresses emission.
    progress_query_id: AtomicI64,
    /// Session-level transaction controller for transaction commands.
    /// Behind a RwLock for interior mutability (set after runtime is shared).
    session_controller: parking_lot::RwLock<Option<Arc<SessionTransactionController>>>,
    /// Transaction scope for this execution (set by bindings).
    transaction_scope: Option<TransactionScope>,
    /// Optional reference to the [`QueryRegistry`] for KILL QUERY.
    /// Behind a `Mutex` for the same interior-mutability reason as the
    /// cancel token (the registry is attached after the executor tree clones
    /// the runtime `Arc`).
    query_registry: parking_lot::Mutex<Option<Arc<QueryRegistry>>>,
    /// Query ID allocated by the registry.
    registry_query_id: parking_lot::Mutex<Option<QueryId>>,
    /// Engine-level shared scheduler for dynamic partition execution.
    /// When set, all queries share the same worker pool instead of creating
    /// per-query threads.  Falls back to serial if neither this nor the
    /// per-query `worker_pool` is set.
    /// Behind a `parking_lot::Mutex` because it's written via `&self` (internal
    /// mutability pattern used throughout [`ExecutionRuntime`]).
    shared_scheduler: parking_lot::Mutex<Option<Arc<super::pool::SharedScheduler>>>,
    /// Query-level morsel worker pool for dynamic partition execution
    /// Kept for backward compatibility.
    /// Created when `max_workers > 1` and no `shared_scheduler` is set;
    /// `None` means serial fallback.
    /// Behind a Mutex so the engine can set the pool after construction.
    pub worker_pool: Arc<parking_lot::Mutex<Option<Arc<dyn TaskScheduler>>>>,
    /// Per-partition output channel capacity for parallel exchange/gather.
    pub max_buffered_chunks: AtomicUsize,
    /// Spill manager for offloading operator data to disk.
    pub spill_manager: Arc<parking_lot::Mutex<Option<Arc<SpillManager>>>>,
    /// Storage client for this query execution.
    /// Moved here from OperatorSpec so that the physical plan tree is
    /// truly immutable and cacheable without sharing storage handles.
    pub storage: Option<Arc<RwLock<dyn QueryStorage>>>,
    pub search: crate::executor::base::SearchContext,
    /// Per-partition operator state arenas.
    ///
    /// Indexed by `partition_id` so parallel workers do not contend on a
    /// single lock.  `state_arenas[0]` serves global / non-partitioned
    /// operators.
    ///
    /// Operators create/read/update their typed state during
    /// `open()` / `next()` / `close()`, indexed by [`PhysicalOperatorId`]
    /// stored in [`OperatorBase::physical_operator_id`](super::operators::base::OperatorBase).
    pub state_arenas: Vec<Mutex<StateArenaSet>>,

    /// Runtime parameter name→value map, bound at materialization time.
    /// Operators read this to resolve `Expression::Parameter` references.
    pub parameter_values: Option<Arc<HashMap<String, Value>>>,

    /// Runtime session variable name→value snapshot, bound at materialization
    /// time. Operators read this to resolve `Expression::SessionVariable`
    /// references.
    pub session_variable_values: Option<Arc<HashMap<String, Value>>>,

    /// Per-query bumpalo arena for executor temporary allocations.
    pub arena: Option<Arc<Mutex<Arena>>>,
    /// Columnar fast-path hit/miss counters shared with produced chunks (T5).
    columnar_stats: Arc<ColumnarStats>,
    /// Cross-query adaptive policy for the typed columnar chunk layout.
    ///
    /// Injected by the materializer from the query bindings (owned by the
    /// optimizer engine).  The policy is read at chunk-production time by
    /// the scan gates; when the runtime finishes, the per-query columnar
    /// stats are merged back into the policy so later queries can adapt.
    columnar_policy: Option<Arc<super::chunk::ColumnarPolicy>>,
    /// Per-query override for the shared columnar policy decision.
    ///
    /// Defaults to inherit; internal callers may pin one query without
    /// touching the shared counters.
    columnar_override: super::chunk::QueryColumnarOverride,
    /// Shared query feedback history for collecting execution statistics.
    ///
    /// Injected by the materializer from the query bindings; when set, the
    /// execution instance records estimated-vs-actual operator feedback here
    /// after execution completes (stats feedback loop).
    pub feedback_history: Option<Arc<QueryFeedbackHistory>>,
    /// Macro catalog manager (user-defined macros), shared engine-wide.
    ///
    /// Injected by the materializer from the query bindings; read by DDL
    /// operators executing `CREATE/DROP MACRO` and `SHOW MACROS`.
    pub macro_manager: Option<Arc<graphdb_core::metadata::MacroManager>>,
    /// Type-alias catalog manager (user-defined types), shared engine-wide.
    ///
    /// Injected by the materializer from the query bindings; read by DDL
    /// operators executing `CREATE/DROP TYPE`.
    pub type_alias_manager: Option<Arc<graphdb_core::metadata::TypeAliasManager>>,
    /// Working tables of enclosing recursive-CTE fixpoints, keyed by the
    /// mangled CTE tag (`crate::cte::mangle_cte_name`).
    ///
    /// Written by the fixpoint operator before each step iteration and
    /// cleared afterwards; read by CTE scan sources. Serial execution only
    /// (fixpoint fragments are never partitioned).
    pub cte_tables:
        Arc<parking_lot::Mutex<std::collections::HashMap<String, Vec<Vec<graphdb_core::Value>>>>>,
}

impl ExecutionRuntime {
    /// Create a new execution runtime with the given query identity, memory budget,
    /// and optional storage client.
    pub fn new(
        query_id: QueryIdentity,
        memory_budget: MemoryBudget,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        search: crate::executor::base::SearchContext,
    ) -> Self {
        Self {
            query_id: parking_lot::Mutex::new(query_id),
            cancel_token_v2: parking_lot::Mutex::new(CancelToken::new()),
            deadline: None,
            memory_budget,
            profile: Arc::new(ProfileBoard::new()),
            resource_owner: Arc::new(Mutex::new(ResourceOwner::new())),
            query_manager: parking_lot::Mutex::new(None),
            progress_rows_interval: AtomicU64::new(0),
            progress_last_emitted: AtomicU64::new(0),
            progress_session_id: AtomicI64::new(0),
            progress_query_id: AtomicI64::new(-1),
            session_controller: parking_lot::RwLock::new(None),
            transaction_scope: None,
            query_registry: parking_lot::Mutex::new(None),
            registry_query_id: parking_lot::Mutex::new(None),
            shared_scheduler: parking_lot::Mutex::new(None),
            worker_pool: Arc::new(parking_lot::Mutex::new(None)),
            max_buffered_chunks: AtomicUsize::new(10),
            spill_manager: Arc::new(parking_lot::Mutex::new(None)),
            storage,
            search,
            state_arenas: vec![Mutex::new(StateArenaSet::new())],
            parameter_values: None,
            session_variable_values: None,
            arena: Some(Arc::new(Mutex::new(Arena::new()))),
            columnar_stats: Arc::new(ColumnarStats::new()),
            columnar_policy: None,
            columnar_override: super::chunk::QueryColumnarOverride::inherit(),
            feedback_history: None,
            macro_manager: None,
            type_alias_manager: None,
            cte_tables: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Create a runtime with default settings (query_id = 0, default memory budget, no storage).
    pub fn default_budget() -> Self {
        Self::new(
            QueryIdentity::default(),
            MemoryBudget::default_budget(),
            None,
            crate::executor::base::SearchContext::default(),
        )
    }

    // ── Query identity ──

    pub fn query_id(&self) -> QueryIdentity {
        self.query_id.lock().clone()
    }

    /// Override the query ID number after construction.
    ///
    /// The factory initialises `query_id.query_id` to 0; the API layer assigns the
    /// real server-side ID before the handle is returned to the caller.
    pub fn assign_query_id(&self, id: u64) {
        self.query_id.lock().query_id = id;
    }

    #[cfg(feature = "fulltext")]
    pub fn set_fulltext_manager(
        &mut self,
        manager: Option<Arc<graphdb_fulltext::manager::FulltextIndexManager>>,
    ) {
        self.search.fulltext_manager = manager;
    }

    #[cfg(feature = "vector")]
    pub fn set_vector_coordinator(
        &mut self,
        coordinator: Option<Arc<graphdb_sync::VectorSyncCoordinator>>,
    ) {
        self.search.vector_coordinator = coordinator;
    }

    /// Attach a QueryManager so that KILL QUERY, finish tracking, and progress
    /// forwarding work.
    ///
    /// Uses interior mutability (`&self`) because the runtime is shared with
    /// the executor tree before the assembly attaches the manager, so `&mut`
    /// access is unavailable at that point.
    pub fn set_query_manager(&self, qm: Arc<QueryManager>) {
        *self.query_manager.lock() = Some(qm);
    }

    /// Register this query with the attached QueryManager and return a
    /// [`QueryFinishGuard`] that marks it finished on drop.
    ///
    /// Returns `None` when no QueryManager is attached (non-fatal).
    pub fn finish_guard(&self) -> Option<QueryFinishGuard> {
        let qm = self.query_manager.lock().as_ref()?.clone();
        let id = self.query_id();
        Some(QueryFinishGuard::new(qm, id.query_id as i64))
    }

    // ── QueryRegistry integration ──

    /// Attach a [`QueryRegistry`] and the allocated [`QueryId`].
    ///
    /// Interior mutability via `Mutex`: the runtime is shared with the
    /// executor tree before registration, so `&mut` access is not available.
    pub fn set_query_registry(&self, registry: Arc<QueryRegistry>, qid: QueryId) {
        *self.query_registry.lock() = Some(registry);
        *self.registry_query_id.lock() = Some(qid);
    }

    /// Return the registry-allocated query ID, if set.
    pub fn registry_query_id(&self) -> Option<QueryId> {
        *self.registry_query_id.lock()
    }

    /// Set the session-level transaction controller for transaction commands.
    pub fn set_session_controller(&self, ctrl: Arc<SessionTransactionController>) {
        *self.session_controller.write() = Some(ctrl);
    }

    /// Return the session-level transaction controller, if set.
    pub fn session_controller(&self) -> Option<Arc<SessionTransactionController>> {
        self.session_controller.read().clone()
    }

    /// Set the parameter name→value map for this execution instance.
    pub fn set_parameter_values(&mut self, values: Arc<HashMap<String, Value>>) {
        self.parameter_values = Some(values);
    }

    /// Return the parameter name→value map, if bound.
    pub fn parameter_values(&self) -> Option<Arc<HashMap<String, Value>>> {
        self.parameter_values.clone()
    }

    /// Set the session variable snapshot for this execution instance.
    pub fn set_session_variable_values(&mut self, values: Arc<HashMap<String, Value>>) {
        self.session_variable_values = Some(values);
    }

    /// Return the session variable snapshot, if bound.
    pub fn session_variable_values(&self) -> Option<Arc<HashMap<String, Value>>> {
        self.session_variable_values.clone()
    }

    /// Set the macro catalog manager for this execution instance.
    pub fn set_macro_manager(
        &mut self,
        manager: Option<Arc<graphdb_core::metadata::MacroManager>>,
    ) {
        self.macro_manager = manager;
    }

    /// Return the macro catalog manager, if bound.
    pub fn macro_manager(&self) -> Option<Arc<graphdb_core::metadata::MacroManager>> {
        self.macro_manager.clone()
    }

    /// Set the type-alias catalog manager for this execution instance.
    pub fn set_type_alias_manager(
        &mut self,
        manager: Option<Arc<graphdb_core::metadata::TypeAliasManager>>,
    ) {
        self.type_alias_manager = manager;
    }

    /// Return the type-alias catalog manager, if bound.
    pub fn type_alias_manager(&self) -> Option<Arc<graphdb_core::metadata::TypeAliasManager>> {
        self.type_alias_manager.clone()
    }

    /// Publish a recursive-CTE working table for the duration of one step
    /// iteration. Overwrites any previous table under the same mangled tag.
    pub fn set_cte_table(&self, mangled_tag: &str, rows: Vec<Vec<graphdb_core::Value>>) {
        self.cte_tables.lock().insert(mangled_tag.to_string(), rows);
    }

    /// Read the current working table published under a mangled CTE tag.
    pub fn cte_table(&self, mangled_tag: &str) -> Option<Vec<Vec<graphdb_core::Value>>> {
        self.cte_tables.lock().get(mangled_tag).cloned()
    }

    /// Remove the working table published under a mangled CTE tag.
    pub fn clear_cte_table(&self, mangled_tag: &str) {
        self.cte_tables.lock().remove(mangled_tag);
    }

    /// Set the transaction scope for this execution.
    pub fn set_transaction_scope(&mut self, scope: TransactionScope) {
        self.transaction_scope = Some(scope);
    }

    /// Return the current transaction scope, if any.
    pub fn transaction_scope(&self) -> Option<&TransactionScope> {
        self.transaction_scope.as_ref()
    }

    /// Return the typed [`CancelToken`] for cooperative cancellation.
    pub fn cancel_token_v2(&self) -> CancelToken {
        self.cancel_token_v2.lock().clone()
    }

    /// Propagate a write-conflict failure of the current transaction.
    ///
    /// Called by write operators when a storage access fails with a
    /// conflict-classified error (write-write conflict, rollback-only). The
    /// session controller is marked rollback-only so a later COMMIT is
    /// rejected, and the shared cancellation token is cancelled with the
    /// typed [`CancelReason::TransactionConflict`] reason — terminating any
    /// pipeline stages of the same transaction that have not started yet.
    pub fn note_transaction_conflict(&self) {
        if let Some(controller) = self.session_controller() {
            controller.mark_rollback_only();
        }
        self.cancel_with_reason(CancelReason::TransactionConflict);
    }

    /// Adopt an externally-owned cancellation token.
    ///
    /// Called at instantiation so the runtime, the query registry, and the
    /// request-scoped [`QueryContext`] share one token (single source for
    /// KILL QUERY / cancellation).  Interior mutability via `Mutex` because
    /// the executor tree holds an `Arc` clone of the runtime by the time
    /// instantiation wires the token.
    pub fn set_cancel_token(&self, token: CancelToken) {
        *self.cancel_token_v2.lock() = token;
    }

    // ── Cancellation ──

    /// Shared cancellation token for cooperative checks by operators and I/O.
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel_token_v2.lock().clone()
    }

    /// Check whether the query has been cancelled (token or deadline).
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token_v2.lock().is_cancelled()
            || self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    /// Return an error if the query has been cancelled.
    pub fn ensure_not_cancelled(&self) -> Result<(), QueryError> {
        if self.is_cancelled() {
            let reason = self
                .cancel_token_v2
                .lock()
                .reason()
                .map(|r| r.to_string())
                .unwrap_or_else(|| "Query cancelled".to_string());
            Err(QueryError::execution(reason))
        } else {
            Ok(())
        }
    }

    /// Cancel this query with a typed reason.
    ///
    /// Sets the [`CancelToken`], marks the query as Killed in the
    /// attached QueryManager, and cancels the registry entry (if configured).
    pub fn cancel_with_reason(&self, reason: CancelReason) {
        self.cancel_token_v2.lock().cancel(reason.clone());
        if let Some(qm) = self.query_manager.lock().as_ref() {
            let id = self.query_id();
            let _ = qm.kill_query(id.query_id as i64);
        }
        if let (Some(ref reg), Some(qid)) = (
            &*self.query_registry.lock(),
            self.registry_query_id.lock().as_ref().copied(),
        ) {
            reg.cancel(qid, reason);
        }
    }

    /// Legacy cancel (no typed reason).  Delegates to [`cancel_with_reason`]
    /// with [`CancelReason::UserKill`].
    pub fn cancel(&self) {
        self.cancel_with_reason(CancelReason::UserKill);
    }

    /// Enable deadline-based cancellation.
    pub fn cancel_on_deadline(&self) {
        if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                self.cancel_with_reason(CancelReason::Deadline);
            }
        }
    }

    /// Set or clear a deadline.
    pub fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.deadline = deadline;
    }

    // ── Profile ──

    pub fn profile(&self) -> &Arc<ProfileBoard> {
        &self.profile
    }

    /// Register an operator profile entry (called during `open()`).
    pub fn register_operator(&self, op_profile: &OperatorProfile) -> Arc<ProfileEntry> {
        self.profile.register_operator(op_profile)
    }

    /// Return a [`StateArenaSet`] mutex for the given partition.
    ///
    /// Non-partitioned operators (`partition_id == None`) always map to
    /// arena 0.  Partitioned operators map to `(partition_id + 1)`, with
    /// fallback to the last arena when the partition count was under-estimated.
    pub fn state_arena_for(&self, partition_id: Option<usize>) -> &Mutex<StateArenaSet> {
        let idx = partition_id.map(|p| p + 1).unwrap_or(0);
        let capped = idx.min(self.state_arenas.len().saturating_sub(1));
        &self.state_arenas[capped]
    }

    /// Set the number of partition arenas (must be ≥ 1).
    pub fn set_partition_count(&mut self, count: usize) {
        let count = count.max(1);
        self.state_arenas
            .resize_with(count, || Mutex::new(StateArenaSet::new()));
    }

    /// Record that execution has started (profile timing).
    pub fn profile_start(&self) {
        self.profile.record_start();
    }

    /// Record that execution has ended.
    pub fn profile_end(&self) {
        self.profile.record_end();
    }

    /// Add rows to the profile counter.
    pub fn profile_add_rows(&self, count: u64) {
        let previous = self.profile.total_rows.fetch_add(count, Ordering::Relaxed);
        self.maybe_emit_progress(previous, previous.saturating_add(count));
    }

    /// Row cadence for query-progress notifications (`0` disables).
    ///
    /// Set alongside `set_query_manager` by the assembly that owns both the
    /// executor and the `QueryManager`. Zero by default: unconfigured
    /// runtimes never emit and pay a single relaxed atomic load per batch.
    pub fn set_progress_rows_interval(&self, rows_interval: u64) {
        self.progress_rows_interval
            .store(rows_interval, Ordering::Relaxed);
    }

    /// Identity reported in progress notifications, in the `QueryManager`
    /// id space. A negative query id suppresses emission until the assembly
    /// provides the real mapping between the two query identity schemes.
    pub fn set_progress_identity(&self, session_id: i64, query_id: i64) {
        self.progress_session_id
            .store(session_id, Ordering::Relaxed);
        self.progress_query_id.store(query_id, Ordering::Relaxed);
        self.progress_last_emitted.store(0, Ordering::Relaxed);
    }

    /// Forward a row watermark to the attached `QueryManager` when it
    /// crosses an unreported interval bucket. Cheap no-op unless a manager
    /// is attached, an interval is configured, and the identity is set.
    fn maybe_emit_progress(&self, previous_total: u64, new_total: u64) {
        let interval = self.progress_rows_interval.load(Ordering::Relaxed);
        if interval == 0 {
            return;
        }
        let query_id = self.progress_query_id.load(Ordering::Relaxed);
        if query_id < 0 {
            return;
        }
        // Clone out of the guard so the lock is released before any callback
        // runs (progress observers must never be invoked under the lock).
        let query_manager = match self.query_manager.lock().as_ref() {
            Some(query_manager) => Arc::clone(query_manager),
            None => return,
        };
        if query_manager.progress_callback_count() == 0 {
            return;
        }
        if new_total / interval == previous_total / interval {
            return;
        }
        let session_id = self.progress_session_id.load(Ordering::Relaxed);
        // Monotonic guard: concurrent workers may cross buckets out of
        // order; only the highest watermark is reported.
        let last = self.progress_last_emitted.load(Ordering::Relaxed);
        if new_total > last
            && self
                .progress_last_emitted
                .compare_exchange(last, new_total, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            query_manager.emit_progress(session_id, query_id, new_total);
        }
    }

    // ── Resource ownership ──

    pub fn resource_owner(&self) -> &Arc<Mutex<ResourceOwner>> {
        &self.resource_owner
    }

    /// Register a cleanup callback.
    pub fn on_cleanup<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.resource_owner.lock().add(Box::new(f));
    }

    /// Release all owned resources.
    pub fn release_resources(&self) {
        self.resource_owner.lock().release_all();
    }

    /// Set the morsel worker pool for this query.
    ///
    /// When a shared scheduler is available via
    /// [`set_shared_scheduler`](Self::set_shared_scheduler), this call is
    /// ignored — the shared scheduler's pool is used instead.  Legacy support:
    /// creates a per-query pool when no shared scheduler is configured.
    pub fn set_worker_pool(&self, pool: Option<super::pool::MorselWorkerPool>) {
        if self.shared_scheduler.lock().is_some() {
            return;
        }
        *self.worker_pool.lock() = pool.map(|p| Arc::new(p) as Arc<dyn TaskScheduler>);
    }

    /// Set the engine-level shared scheduler for this query.
    ///
    /// When set, all parallel execution uses the shared worker pool instead
    /// of per-query threads.  The scheduler's `Arc<dyn TaskScheduler>` is
    /// injected into `worker_pool` so existing consumer code paths
    /// (Exchange / Gather operators) continue to work unchanged.
    ///
    /// Takes priority over any per-query pool that may have been set.
    pub fn set_shared_scheduler(&self, scheduler: Option<Arc<super::pool::SharedScheduler>>) {
        *self.shared_scheduler.lock() = scheduler.clone();
        if let Some(ref ss) = scheduler {
            ss.apply_to_runtime(self);
        }
    }

    /// Raw injection — set the worker pool from an `Arc<dyn TaskScheduler>`.
    /// Used internally by [`SharedScheduler::apply_to_runtime`].
    pub(crate) fn set_shared_scheduler_raw(&self, pool: Option<Arc<dyn TaskScheduler>>) {
        *self.worker_pool.lock() = pool;
    }

    /// Return the shared scheduler, if set.
    pub fn get_shared_scheduler(&self) -> Option<Arc<super::pool::SharedScheduler>> {
        self.shared_scheduler.lock().clone()
    }

    /// Return the effective worker pool — either from the shared scheduler,
    /// or from the per-query pool, or `None` for serial fallback.
    pub fn effective_worker_pool(&self) -> Option<Arc<dyn TaskScheduler>> {
        self.worker_pool.lock().clone()
    }

    /// Set the spill manager for this query execution.
    pub fn set_spill_manager(&self, manager: Option<Arc<SpillManager>>) {
        if let Some(ref m) = manager {
            m.register_cleanup(self);
        }
        *self.spill_manager.lock() = manager;
    }

    /// Access the spill manager.
    pub fn get_spill_manager(&self) -> Option<Arc<SpillManager>> {
        self.spill_manager.lock().clone()
    }

    /// Return a reference to the bumpalo arena, if configured.
    pub fn arena(&self) -> Option<&Arc<Mutex<Arena>>> {
        self.arena.as_ref()
    }

    /// Return the columnar fast-path counters shared with produced chunks.
    pub fn columnar_stats(&self) -> Arc<ColumnarStats> {
        Arc::clone(&self.columnar_stats)
    }

    /// Return the shared columnar layout policy, if injected.
    pub fn columnar_policy(&self) -> Option<Arc<super::chunk::ColumnarPolicy>> {
        self.columnar_policy.clone()
    }

    /// Inject the shared columnar layout policy (owned by the optimizer
    /// engine; set by the materializer from the query bindings).
    pub fn set_columnar_policy(&mut self, policy: Option<Arc<super::chunk::ColumnarPolicy>>) {
        self.columnar_policy = policy;
    }

    /// Per-query columnar override for this runtime.
    pub fn columnar_override(&self) -> super::chunk::QueryColumnarOverride {
        self.columnar_override
    }

    /// Pin the columnar decision for this query only.
    pub fn set_columnar_override(&mut self, query_override: super::chunk::QueryColumnarOverride) {
        self.columnar_override = query_override;
    }

    /// Merge this query's columnar hit/miss counts into the shared policy.
    ///
    /// Called once when the query finishes (materialized, streaming, and
    /// discard paths) so the adaptive gate learns across queries.
    pub fn flush_columnar_stats_to_policy(&self) {
        if let Some(policy) = &self.columnar_policy {
            let snapshot = ColumnarStatsSnapshot::from_stats(&self.columnar_stats);
            // B path hits also represent typed benefit; feed them into the
            // policy together with the A path hits. Wasted builds stay out:
            // they measure built-but-unconsumed layouts, not hits/misses.
            let effective_hits = snapshot.columnar_hits + snapshot.columnar_b_path_hits;
            policy.merge(effective_hits, snapshot.columnar_misses);
        }
    }

    /// Reset the bumpalo arena, freeing all temporary allocations.
    pub fn reset_arena(&self) {
        if let Some(arena) = &self.arena {
            arena.lock().reset();
        }
    }

    /// Set the per-partition output channel capacity for parallel operators.
    pub fn set_max_buffered_chunks(&self, chunks: usize) {
        self.max_buffered_chunks
            .store(chunks.max(1), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_default_budget() {
        let rt = ExecutionRuntime::default_budget();
        assert!(!rt.is_cancelled());
        assert_eq!(rt.query_id().query_id, 0);
    }

    #[test]
    fn test_cancel_token() {
        let rt = ExecutionRuntime::default_budget();
        assert!(!rt.is_cancelled());
        rt.cancel();
        assert!(rt.is_cancelled());
        assert!(rt.ensure_not_cancelled().is_err());
    }

    #[test]
    fn test_deadline() {
        let mut rt = ExecutionRuntime::default_budget();
        rt.set_deadline(Some(Instant::now()));
        assert!(rt.is_cancelled());
    }

    #[test]
    fn test_profile_add_rows() {
        let rt = ExecutionRuntime::default_budget();
        rt.profile_add_rows(10);
        rt.profile_add_rows(20);
        assert_eq!(
            rt.profile()
                .total_rows
                .load(std::sync::atomic::Ordering::Relaxed),
            30
        );
    }
}
