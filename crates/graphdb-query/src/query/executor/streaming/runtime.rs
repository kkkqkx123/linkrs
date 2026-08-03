use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use parking_lot::RwLock;

use super::plan::types::PhysicalOperatorId;
use super::query_registry::{CancelToken, QueryId, QueryRegistry};
use super::slot::SlotLayout;
use super::spill::SpillManager;
use super::state::StateArenaSet;
use super::transaction_scope::{CancelReason, SessionTransactionController, TransactionScope};
use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::MemoryBudget;
use crate::query::executor::streaming::pool::TaskScheduler;
use crate::query::query_manager::QueryManager;
use crate::storage::QueryStorage;
use crate::utils::Arena;

/// Query identity information
#[derive(Debug, Clone, Default)]
pub struct QueryIdentity {
    pub query_id: u64,
    pub session_id: Option<String>,
    pub space_name: Option<String>,
}

/// Query-level columnar fast-path counters (T5 observability).
///
/// Shared via `Arc` with the chunks produced by source operators; every
/// `DataChunk::evaluate_expression` call records a hit (columnar batch path
/// succeeded) or a miss (fell back to per-row evaluation). The miss rate
/// exposes how much of the flat-column promise is actually kept.
#[derive(Debug, Default)]
pub struct ColumnarStats {
    pub columnar_hits: AtomicU64,
    pub columnar_misses: AtomicU64,
}

impl ColumnarStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_hit(&self) {
        self.columnar_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.columnar_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Fraction of evaluation calls that hit the columnar fast path.
    /// Returns 1.0 when nothing was evaluated (vacuous).
    pub fn hit_rate(&self) -> f64 {
        let hits = self.columnar_hits.load(Ordering::Relaxed);
        let misses = self.columnar_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            1.0
        } else {
            hits as f64 / total as f64
        }
    }
}

/// Per-operator profile snapshot
#[derive(Debug, Clone, Default)]
pub struct OperatorProfile {
    pub physical_operator_id: PhysicalOperatorId,
    pub node_id: i64,
    pub partition_id: Option<usize>,
    pub name: String,
    pub open_time_us: u64,
    pub next_time_us: u64,
    pub close_time_us: u64,
    pub output_rows: u64,
    pub peak_memory: u64,
    pub peak_memory_bytes: u64,
    pub spilled_bytes: u64,
    pub spill_count: u64,
}

/// Identifies an operator instance in a partitioned executor tree.
///
/// A physical operator occurs once per local partition. Logical node IDs may
/// be shared by multiple physical operators, so they are display metadata and
/// cannot identify a profile entry. Global and gather operators use `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperatorProfileKey {
    pub physical_operator_id: PhysicalOperatorId,
    pub partition_id: Option<usize>,
}

impl OperatorProfileKey {
    pub const fn new(
        physical_operator_id: PhysicalOperatorId,
        partition_id: Option<usize>,
    ) -> Self {
        Self {
            physical_operator_id,
            partition_id,
        }
    }
}

/// Per-operator atomic profile counters for lock-free hot-path updates.
///
/// Each operator registers a [`ProfileEntry`] during `open()`.  Subsequent
/// timing and row-count updates use atomic operations with no lock held,
/// eliminating mutex contention on the per-advance hot path.
#[derive(Debug)]
pub struct ProfileEntry {
    pub physical_operator_id: PhysicalOperatorId,
    pub node_id: AtomicI64,
    pub partition_id: Option<usize>,
    pub name: parking_lot::Mutex<String>,
    pub open_time_us: AtomicU64,
    pub next_time_us: AtomicU64,
    pub close_time_us: AtomicU64,
    pub output_rows: AtomicU64,
    pub peak_memory_bytes: AtomicU64,
    pub spilled_bytes: AtomicU64,
    pub spill_count: AtomicU64,
}

impl ProfileEntry {
    pub fn new(profile: &OperatorProfile) -> Self {
        Self {
            physical_operator_id: profile.physical_operator_id,
            node_id: AtomicI64::new(profile.node_id),
            partition_id: profile.partition_id,
            name: parking_lot::Mutex::new(profile.name.clone()),
            open_time_us: AtomicU64::new(profile.open_time_us),
            next_time_us: AtomicU64::new(profile.next_time_us),
            close_time_us: AtomicU64::new(profile.close_time_us),
            output_rows: AtomicU64::new(profile.output_rows),
            peak_memory_bytes: AtomicU64::new(profile.peak_memory_bytes),
            spilled_bytes: AtomicU64::new(profile.spilled_bytes),
            spill_count: AtomicU64::new(profile.spill_count),
        }
    }

    /// Snapshot current atomics into an [`OperatorProfile`] for reporting.
    pub fn snapshot(&self) -> OperatorProfile {
        OperatorProfile {
            physical_operator_id: self.physical_operator_id,
            node_id: self.node_id.load(Ordering::Relaxed),
            partition_id: self.partition_id,
            name: self.name.lock().clone(),
            open_time_us: self.open_time_us.load(Ordering::Relaxed),
            next_time_us: self.next_time_us.load(Ordering::Relaxed),
            close_time_us: self.close_time_us.load(Ordering::Relaxed),
            output_rows: self.output_rows.load(Ordering::Relaxed),
            peak_memory: self.peak_memory_bytes.load(Ordering::Relaxed),
            peak_memory_bytes: self.peak_memory_bytes.load(Ordering::Relaxed),
            spilled_bytes: self.spilled_bytes.load(Ordering::Relaxed),
            spill_count: self.spill_count.load(Ordering::Relaxed),
        }
    }
}

/// Lock-free profile store for hot-path operator timing and row-count updates.
///
/// Uses `RwLock` for structural operations (first-access entry creation) and
/// `AtomicU64` counters so that per-advance increments never block.
///
/// Design:
/// - `register_operator()` — called during `open()`, pre-creates entries
/// - `record_timing()` / `record_rows()` — lock-free fast path
/// - `flush_to_collector()` — aggregate into a [`ProfileCollector`] at end
#[derive(Debug)]
pub struct ProfileBoard {
    entries: RwLock<HashMap<OperatorProfileKey, Arc<ProfileEntry>>>,
    pub total_rows: AtomicU64,
    pub total_time_us: AtomicU64,
    start_time: Mutex<Option<Instant>>,
    end_time: Mutex<Option<Instant>>,
    pub parallel_wall_time_us: AtomicU64,
    pub parallel_work_time_us: AtomicU64,
    pub parallel_workers: AtomicUsize,
    pub parallel_buffered_chunks_peak: AtomicUsize,
    pub parallel_buffered_bytes_peak: AtomicUsize,
}

impl Default for ProfileBoard {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileBoard {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            total_rows: AtomicU64::new(0),
            total_time_us: AtomicU64::new(0),
            start_time: Mutex::new(None),
            end_time: Mutex::new(None),
            parallel_wall_time_us: AtomicU64::new(0),
            parallel_work_time_us: AtomicU64::new(0),
            parallel_workers: AtomicUsize::new(0),
            parallel_buffered_chunks_peak: AtomicUsize::new(0),
            parallel_buffered_bytes_peak: AtomicUsize::new(0),
        }
    }

    pub fn record_start(&self) {
        *self.start_time.lock() = Some(Instant::now());
    }

    pub fn record_end(&self) {
        let elapsed = self
            .start_time
            .lock()
            .map(|t| t.elapsed().as_micros() as u64)
            .unwrap_or(0);
        self.total_time_us.store(elapsed, Ordering::Relaxed);
        *self.end_time.lock() = Some(Instant::now());
    }

    /// Register (or update) a per-operator profile entry from a snapshot.
    ///
    /// Called during `open()` to pre-populate entries so the hot-path
    /// access in `advance()` never needs to write-lock.
    pub fn register_operator(&self, profile: &OperatorProfile) -> Arc<ProfileEntry> {
        let key = OperatorProfileKey::new(profile.physical_operator_id, profile.partition_id);
        let entry = Arc::new(ProfileEntry::new(profile));
        self.entries.write().insert(key, entry.clone());
        entry
    }

    /// Find an entry by key (read-lock, hot-path friendly).
    pub fn get_entry(&self, key: &OperatorProfileKey) -> Option<Arc<ProfileEntry>> {
        self.entries.read().get(key).cloned()
    }

    /// Aggregate all entries into a [`ProfileCollector`] for EXPLAIN output.
    pub fn flush_to_collector(&self) -> ProfileCollector {
        let mut collector = ProfileCollector::new();
        let guard = self.entries.read();
        for entry in guard.values() {
            collector.operators.insert(
                OperatorProfileKey::new(entry.physical_operator_id, entry.partition_id),
                entry.snapshot(),
            );
        }
        drop(guard);
        collector.total_rows = self.total_rows.load(Ordering::Relaxed);
        collector.total_time_us = self.total_time_us.load(Ordering::Relaxed);
        collector.start_time = *self.start_time.lock();
        collector.end_time = *self.end_time.lock();
        collector.parallel_wall_time_us = self.parallel_wall_time_us.load(Ordering::Relaxed);
        collector.parallel_work_time_us = self.parallel_work_time_us.load(Ordering::Relaxed);
        collector.parallel_workers = self.parallel_workers.load(Ordering::Relaxed);
        collector.parallel_buffered_chunks_peak =
            self.parallel_buffered_chunks_peak.load(Ordering::Relaxed);
        collector.parallel_buffered_bytes_peak =
            self.parallel_buffered_bytes_peak.load(Ordering::Relaxed);
        collector
    }
}

/// Collects execution profile data across all operators (for EXPLAIN output).
#[derive(Debug, Default)]
pub struct ProfileCollector {
    pub operators: HashMap<OperatorProfileKey, OperatorProfile>,
    pub total_rows: u64,
    pub total_time_us: u64,
    pub start_time: Option<Instant>,
    pub end_time: Option<Instant>,
    /// Wall-clock time spent in parallel partition execution (P8).
    pub parallel_wall_time_us: u64,
    /// Sum of per-worker execution time (may exceed wall time, P8).
    pub parallel_work_time_us: u64,
    /// Maximum number of P8 workers used by any coordinator in this query.
    pub parallel_workers: usize,
    /// Peak number of chunks retained in P8 output queues.
    pub parallel_buffered_chunks_peak: usize,
    /// Peak accounted bytes retained in P8 output queues.
    pub parallel_buffered_bytes_peak: usize,
}

impl ProfileCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_start(&mut self) {
        self.start_time = Some(Instant::now());
    }

    pub fn record_end(&mut self) {
        self.end_time = Some(Instant::now());
        if let Some(start) = self.start_time {
            self.total_time_us = start.elapsed().as_micros() as u64;
        }
    }

    pub fn add_rows(&mut self, count: u64) {
        self.total_rows += count;
    }

    pub fn record_operator_profile(&mut self, profile: OperatorProfile) {
        let key = OperatorProfileKey::new(profile.physical_operator_id, profile.partition_id);
        self.operators.insert(key, profile);
    }

    /// Return a snapshot of the P8 parallel profile fields for
    /// EXPLAIN / PROFILE output.
    pub fn parallel_profile(&self) -> (u64, u64, usize, usize, usize) {
        (
            self.parallel_wall_time_us,
            self.parallel_work_time_us,
            self.parallel_workers,
            self.parallel_buffered_chunks_peak,
            self.parallel_buffered_bytes_peak,
        )
    }

    /// Aggregate profiles from partition execution into this collector.
    ///
    /// For each operator node_id, sums timing/output_rows and takes the max
    /// of peak_memory_bytes across partitions.
    pub fn aggregate_partition_profiles(&mut self, partition_profiles: &[ProfileCollector]) {
        for pp in partition_profiles {
            for (key, op) in &pp.operators {
                let entry = self
                    .operators
                    .entry(*key)
                    .or_insert_with(|| OperatorProfile {
                        physical_operator_id: key.physical_operator_id,
                        node_id: op.node_id,
                        partition_id: key.partition_id,
                        name: op.name.clone(),
                        ..OperatorProfile::default()
                    });
                entry.open_time_us += op.open_time_us;
                entry.next_time_us += op.next_time_us;
                entry.close_time_us += op.close_time_us;
                entry.output_rows += op.output_rows;
                entry.peak_memory_bytes = entry.peak_memory_bytes.max(op.peak_memory_bytes);
                entry.spill_count += op.spill_count;
                entry.spilled_bytes += op.spilled_bytes;
            }
            self.total_rows += pp.total_rows;
        }
    }
}

/// Manages cleanup of runtime resources (cursors, temp files, etc.)
pub struct ResourceOwner {
    cleanup: Vec<Box<dyn FnOnce() + Send>>,
}

impl std::fmt::Debug for ResourceOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceOwner")
            .field("cleanup_count", &self.cleanup.len())
            .finish()
    }
}

impl Default for ResourceOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceOwner {
    pub fn new() -> Self {
        Self {
            cleanup: Vec::new(),
        }
    }

    pub fn add(&mut self, cleanup: Box<dyn FnOnce() + Send>) {
        self.cleanup.push(cleanup);
    }

    pub fn release_all(&mut self) {
        for f in self.cleanup.drain(..) {
            f();
        }
    }
}

/// Per-query execution runtime shared across all operators.
///
/// Centralises cancellation, memory tracking, profiling, resource
/// lifecycle, and query-registration so that operators do not each
/// carry ad-hoc context.
///
/// Phase 2 (M2): unified cancellation via [`CancelToken`] replaces the
/// legacy `AtomicBool`.  Both coexist during migration.
#[derive(Debug)]
pub struct ExecutionRuntime {
    /// Query identity (behind Mutex for write-once from the API layer).
    query_id: parking_lot::Mutex<QueryIdentity>,
    /// Legacy cancel flag (AtomicBool).  Replaced by `cancel_token_v2`.
    cancel_token: Arc<AtomicBool>,
    /// M2: typed cancellation token with reason tracking.
    cancel_token_v2: CancelToken,
    /// Optional deadline; the query is cancelled after this instant.
    deadline: Option<Instant>,
    /// Per-query memory budget for blocking operators.
    pub memory_budget: MemoryBudget,
    /// Profile board with atomic counters for lock-free hot-path recording.
    profile: Arc<ProfileBoard>,
    /// Resource owner for cleanup of cursors, temp files, etc.
    resource_owner: Arc<Mutex<ResourceOwner>>,
    /// Optional reference to the global QueryManager for KILL QUERY.
    query_manager: Option<Arc<QueryManager>>,
    /// M2: Session-level transaction controller for transaction commands.
    /// Behind a RwLock for interior mutability (set after runtime is shared).
    session_controller: parking_lot::RwLock<Option<Arc<SessionTransactionController>>>,
    /// M2: Transaction scope for this execution (set by bindings).
    transaction_scope: Option<TransactionScope>,
    /// M2: Optional reference to the [`QueryRegistry`] for KILL QUERY.
    query_registry: Option<Arc<QueryRegistry>>,
    /// M2: Query ID allocated by the registry.
    registry_query_id: Option<QueryId>,
    /// M6: Engine-level shared scheduler for dynamic partition execution.
    /// When set, all queries share the same worker pool instead of creating
    /// per-query threads.  Falls back to serial if neither this nor the
    /// per-query `worker_pool` is set.
    /// Behind a `parking_lot::Mutex` because it's written via `&self` (internal
    /// mutability pattern used throughout [`ExecutionRuntime`]).
    shared_scheduler: parking_lot::Mutex<Option<Arc<super::pool::SharedScheduler>>>,
    /// Query-level morsel worker pool for dynamic partition execution
    /// (legacy, kept for backward compat during M6 migration).
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
    #[cfg(feature = "fulltext-search")]
    pub fulltext_manager: Option<Arc<crate::search::manager::FulltextIndexManager>>,
    #[cfg(feature = "qdrant")]
    pub vector_coordinator: Option<Arc<crate::sync::VectorSyncCoordinator>>,
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

    /// Correlation frame for [`Argument`](super::operators::source_operator::SourceOperator::Argument)
    /// sources inside Apply right subtrees.
    ///
    /// `Apply` sets this before pulling from its right child; `Argument`
    /// reads the current row and layout to produce output.
    correlation_frame: Mutex<Option<(Arc<SlotLayout>, Vec<Value>)>>,

    /// Runtime parameter name→value map, bound at materialization time.
    /// Operators read this to resolve `Expression::Parameter` references.
    pub parameter_values: Option<Arc<HashMap<String, Value>>>,

    /// Per-query bumpalo arena for executor temporary allocations.
    pub arena: Option<Arc<Mutex<Arena>>>,
    /// Columnar fast-path hit/miss counters shared with produced chunks (T5).
    columnar_stats: Arc<ColumnarStats>,
}

impl ExecutionRuntime {
    /// Create a new execution runtime with the given query identity, memory budget,
    /// and optional storage client.
    pub fn new(
        query_id: QueryIdentity,
        memory_budget: MemoryBudget,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        #[cfg(feature = "fulltext-search")] fulltext_manager: Option<
            Arc<crate::search::manager::FulltextIndexManager>,
        >,
        #[cfg(feature = "qdrant")] vector_coordinator: Option<
            Arc<crate::sync::VectorSyncCoordinator>,
        >,
    ) -> Self {
        Self {
            query_id: parking_lot::Mutex::new(query_id),
            cancel_token: Arc::new(AtomicBool::new(false)),
            cancel_token_v2: CancelToken::new(),
            deadline: None,
            memory_budget,
            profile: Arc::new(ProfileBoard::new()),
            resource_owner: Arc::new(Mutex::new(ResourceOwner::new())),
            query_manager: None,
            session_controller: parking_lot::RwLock::new(None),
            transaction_scope: None,
            query_registry: None,
            registry_query_id: None,
            shared_scheduler: parking_lot::Mutex::new(None),
            worker_pool: Arc::new(parking_lot::Mutex::new(None)),
            max_buffered_chunks: AtomicUsize::new(10),
            spill_manager: Arc::new(parking_lot::Mutex::new(None)),
            storage,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager,
            #[cfg(feature = "qdrant")]
            vector_coordinator,
            state_arenas: vec![Mutex::new(StateArenaSet::new())],
            correlation_frame: Mutex::new(None),
            parameter_values: None,
            arena: Some(Arc::new(Mutex::new(Arena::new()))),
            columnar_stats: Arc::new(ColumnarStats::new()),
        }
    }

    /// Create a runtime with default settings (query_id = 0, default memory budget, no storage).
    pub fn default_budget() -> Self {
        Self::new(
            QueryIdentity::default(),
            MemoryBudget::default_budget(),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "qdrant")]
            None,
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

    #[cfg(feature = "fulltext-search")]
    pub fn set_fulltext_manager(
        &mut self,
        manager: Option<Arc<crate::search::manager::FulltextIndexManager>>,
    ) {
        self.fulltext_manager = manager;
    }

    #[cfg(feature = "qdrant")]
    pub fn set_vector_coordinator(
        &mut self,
        coordinator: Option<Arc<crate::sync::VectorSyncCoordinator>>,
    ) {
        self.vector_coordinator = coordinator;
    }

    /// Attach a QueryManager so that KILL QUERY and finish tracking work.
    pub fn set_query_manager(&mut self, qm: Arc<QueryManager>) {
        self.query_manager = Some(qm);
    }

    /// Register this query with the attached QueryManager and return a
    /// [`QueryFinishGuard`] that marks it finished on drop.
    ///
    /// Returns `None` when no QueryManager is attached (non-fatal).
    pub fn finish_guard(&self) -> Option<QueryFinishGuard> {
        let qm = self.query_manager.as_ref()?.clone();
        let id = self.query_id();
        Some(QueryFinishGuard::new(qm, id.query_id as i64))
    }

    // ── M2: QueryRegistry integration ──

    /// Attach a [`QueryRegistry`] and the allocated [`QueryId`].
    pub fn set_query_registry(&mut self, registry: Arc<QueryRegistry>, qid: QueryId) {
        self.query_registry = Some(registry);
        self.registry_query_id = Some(qid);
    }

    /// Return the registry-allocated query ID, if set.
    pub fn registry_query_id(&self) -> Option<QueryId> {
        self.registry_query_id
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
        self.cancel_token_v2.clone()
    }

    // ── Cancellation ──

    /// Legacy token used to signal cancellation (shared with operators and I/O).
    pub fn cancel_token(&self) -> Arc<AtomicBool> {
        self.cancel_token.clone()
    }

    /// Check whether the query has been cancelled (checks both legacy and v2).
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.load(Ordering::Relaxed)
            || self.cancel_token_v2.is_cancelled()
            || self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    /// Return an error if the query has been cancelled.
    pub fn ensure_not_cancelled(&self) -> Result<(), QueryError> {
        if self.is_cancelled() {
            let reason = self
                .cancel_token_v2
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
    /// Sets both the legacy `AtomicBool` and the M2 [`CancelToken`].
    /// Also marks the query as Killed in the attached QueryManager and
    /// cancels the registry entry (if configured).
    pub fn cancel_with_reason(&self, reason: CancelReason) {
        self.cancel_token.store(true, Ordering::Relaxed);
        self.cancel_token_v2.cancel(reason.clone());
        if let Some(ref qm) = self.query_manager {
            let id = self.query_id();
            let _ = qm.kill_query(id.query_id as i64);
        }
        if let (Some(ref reg), Some(qid)) = (&self.query_registry, self.registry_query_id) {
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
        self.profile.total_rows.fetch_add(count, Ordering::Relaxed);
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

    /// Set the engine-level shared scheduler for this query (M6).
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
    /// or from the legacy per-query pool, or `None` for serial fallback.
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

    // ── Correlation frame (for Argument / Apply) ──

    /// Set the correlation row that [`Argument`] sources will read.
    ///
    /// Called by `ApplyOperator` before pulling from the right subtree.
    pub fn set_correlation_frame(&self, layout: Arc<SlotLayout>, row: Vec<Value>) {
        *self.correlation_frame.lock() = Some((layout, row));
    }

    /// Take the current correlation frame, if any.
    ///
    /// Called by `SourceOperator::Argument` on each `next()` call.
    pub fn take_correlation_frame(&self) -> Option<(Arc<SlotLayout>, Vec<Value>)> {
        self.correlation_frame.lock().take()
    }

    /// Clear the correlation frame (used after right subtree evaluation).
    pub fn clear_correlation_frame(&self) {
        *self.correlation_frame.lock() = None;
    }
}

/// RAII guard that marks a query as finished in the QueryManager on drop.
///
/// Created by [`ExecutionRuntime::finish_guard`]. Ensures the query
/// lifecycle is tracked even when the caller forgets to call finish
/// explicitly, or when execution panics mid-flight.
#[derive(Debug)]
pub struct QueryFinishGuard {
    query_manager: Arc<QueryManager>,
    query_id: i64,
    finished: bool,
}

impl QueryFinishGuard {
    pub fn new(query_manager: Arc<QueryManager>, query_id: i64) -> Self {
        Self {
            query_manager,
            query_id,
            finished: false,
        }
    }

    /// Mark the query as finished immediately without waiting for Drop.
    pub fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            let _ = self.query_manager.finish_query(self.query_id);
        }
    }
}

impl Drop for QueryFinishGuard {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.query_manager.finish_query(self.query_id);
        }
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
    fn test_profile_collector() {
        let mut pc = ProfileCollector::new();
        pc.record_start();
        std::thread::sleep(std::time::Duration::from_micros(100));
        pc.record_end();
        assert!(pc.total_time_us > 0);
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

    #[test]
    fn test_partition_profile_aggregation_preserves_partition_identity() {
        let mut first = ProfileCollector::new();
        first.record_operator_profile(OperatorProfile {
            physical_operator_id: PhysicalOperatorId(7),
            node_id: 7,
            partition_id: Some(0),
            name: "ScanVertices".to_string(),
            output_rows: 2,
            peak_memory_bytes: 10,
            ..OperatorProfile::default()
        });
        let mut second = ProfileCollector::new();
        second.record_operator_profile(OperatorProfile {
            physical_operator_id: PhysicalOperatorId(7),
            node_id: 7,
            partition_id: Some(1),
            name: "ScanVertices".to_string(),
            output_rows: 3,
            peak_memory_bytes: 20,
            ..OperatorProfile::default()
        });

        let mut aggregate = ProfileCollector::new();
        aggregate.aggregate_partition_profiles(&[first, second]);

        assert_eq!(aggregate.operators.len(), 2);
        assert_eq!(
            aggregate
                .operators
                .get(&OperatorProfileKey::new(PhysicalOperatorId(7), Some(0)))
                .expect("partition zero profile")
                .output_rows,
            2
        );
        assert_eq!(
            aggregate
                .operators
                .get(&OperatorProfileKey::new(PhysicalOperatorId(7), Some(1)))
                .expect("partition one profile")
                .peak_memory_bytes,
            20
        );
    }

    #[test]
    fn test_resource_owner() {
        let mut owner = ResourceOwner::new();
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f = flag.clone();
        owner.add(Box::new(move || {
            f.store(true, std::sync::atomic::Ordering::Relaxed);
        }));
        owner.release_all();
        assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
    }
}
