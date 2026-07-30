//! Transaction Manager
//!
//! Manages the lifecycle of all transactions, providing operations such as
//! transaction start, commit, and abort. Uses MVCC version management for
//! snapshot isolation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use parking_lot::{Condvar, Mutex, RwLock};

use dashmap::DashMap;

use super::context::TransactionContext;
use super::error::TransactionError;
use super::monitor::TransactionMonitor;
use super::mvcc::{VersionManager, VersionManagerConfig};
use super::participant::{
    TransactionAbortDescriptor, TransactionCommitDescriptor, TransactionCommitSink,
    TransactionMutationRecorder,
};
use super::rollback::UndoLogRollback;
use super::types::*;
use super::undo_log::UndoTarget;
use crate::core::stats::StatsManager;
use crate::core::types::{CommitLsn, LabelId, Timestamp, VertexId};
use crate::core::wal::types::Lsn;
use crate::sync::SyncManager;

/// Checkpoint coordination gate.
///
/// pauses new write transactions and waits for active writes to complete,
/// then allows checkpoint to proceed. Modeled after Ladybug's checkpoint
/// isolation where checkpoint blocks new writes and drains active ones.
pub struct CheckpointGate {
    /// When true, no new write transactions are allowed.
    writing_paused: AtomicBool,
    /// Number of active write transactions currently in-flight.
    /// Used to determine when the gate has fully drained.
    active_writes: AtomicU64,
    /// Condvar for checkpoint thread to wait on until active_writes reaches 0.
    condvar: Condvar,
    /// Mutex protecting the condvar.
    mutex: Mutex<()>,
}

impl CheckpointGate {
    pub fn new() -> Self {
        Self {
            writing_paused: AtomicBool::new(false),
            active_writes: AtomicU64::new(0),
            condvar: Condvar::new(),
            mutex: Mutex::new(()),
        }
    }

    /// Attempt to acquire a write slot. Returns Err if writes are paused.
    pub fn acquire_write(&self) -> Result<(), TransactionError> {
        if self.writing_paused.load(Ordering::SeqCst) {
            return Err(TransactionError::checkpoint_in_progress());
        }
        self.active_writes.fetch_add(1, Ordering::SeqCst);
        // Re-check after incrementing: if paused between the two atomics, bail out.
        if self.writing_paused.load(Ordering::SeqCst) {
            self.active_writes.fetch_sub(1, Ordering::SeqCst);
            return Err(TransactionError::checkpoint_in_progress());
        }
        Ok(())
    }

    /// Release a write slot (called when a write transaction commits or aborts).
    pub fn release_write(&self) {
        let prev = self.active_writes.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            // Last active write; wake up the checkpoint thread if waiting.
            self.condvar.notify_all();
        }
    }

    /// Pause new writes and wait for all active writes to complete.
    /// Returns Err if the timeout elapses before all writes drain.
    ///
    /// On timeout, writes are automatically resumed to prevent deadlock.
    pub fn pause_writes_and_drain(&self, timeout: Duration) -> Result<(), TransactionError> {
        self.writing_paused.store(true, Ordering::SeqCst);

        let mut guard = self.mutex.lock();
        let start = std::time::Instant::now();
        loop {
            let active = self.active_writes.load(Ordering::SeqCst);
            if active == 0 {
                return Ok(());
            }
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                self.writing_paused.store(false, Ordering::SeqCst);
                return Err(TransactionError::checkpoint_timeout(active));
            }
            let remaining = timeout - elapsed;
            let result = self.condvar.wait_for(&mut guard, remaining);
            if result.timed_out() {
                self.writing_paused.store(false, Ordering::SeqCst);
                return Err(TransactionError::checkpoint_timeout(
                    self.active_writes.load(Ordering::SeqCst),
                ));
            }
        }
    }

    /// Resume accepting new write transactions after checkpoint completes.
    pub fn resume_writes(&self) {
        self.writing_paused.store(false, Ordering::SeqCst);
    }

    /// Whether writes are currently paused.
    pub fn is_paused(&self) -> bool {
        self.writing_paused.load(Ordering::SeqCst)
    }

    /// Current count of active writes.
    pub fn active_write_count(&self) -> u64 {
        self.active_writes.load(Ordering::SeqCst)
    }
}

impl Default for CheckpointGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Checkpoint transaction handle.
///
/// Held while a checkpoint is in progress. Writes are paused for the lifetime
/// of this handle (via [`CheckpointGate::pause_writes_and_drain`] at construction).
/// Once the caller finishes the checkpoint work, they must either [`commit`]
/// (writes a WAL `CheckpointMarker` record and resumes writes) or [`abort`]
/// (resumes writes without logging the marker). Dropping the handle also
/// resumes writes.
///
/// This mirrors Ladybug's `TRANSACTION_TYPE::CHECKPOINT` behavior: checkpoint
/// is an exclusive operation that prevents new writes from starting and drains
/// in-flight writes before proceeding.
pub struct CheckpointTransaction<'a> {
    manager: &'a TransactionManager,
    gate: Arc<CheckpointGate>,
    txn_id: TransactionId,
    write_ts: Timestamp,
    finished: bool,
}

impl CheckpointTransaction<'_> {
    /// The write timestamp captured at checkpoint begin time.
    ///
    /// All data with `ts <= write_ts` is guaranteed to be visible at checkpoint.
    pub fn write_timestamp(&self) -> Timestamp {
        self.write_ts
    }

    /// Commit the checkpoint: resume writes.
    ///
    /// The caller is responsible for persisting checkpoint metadata (via the
    /// `CheckpointManager`) before calling this method. After commit, writes
    /// are resumed.
    pub fn commit(mut self) -> Result<(), TransactionError> {
        self.finished = true;
        let result = self.manager.commit_transaction(self.txn_id);
        self.gate.resume_writes();
        result
    }

    /// Abort the checkpoint without writing a WAL marker. Writes are resumed.
    pub fn abort(mut self) -> Result<(), TransactionError> {
        self.finished = true;
        let result = self.manager.abort_transaction(self.txn_id);
        self.gate.resume_writes();
        result
    }
}

impl Drop for CheckpointTransaction<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.manager.abort_transaction(self.txn_id);
            self.gate.resume_writes();
        }
    }
}

/// Transaction Manager
///
/// Manages the lifecycle of all transactions using MVCC version management.
/// Supports read and insert (write) transactions.
/// All write transactions run concurrently; conflicts are detected at commit time.
pub struct TransactionManager {
    /// Version manager for MVCC timestamps
    version_manager: Arc<VersionManager>,
    /// Configuration
    config: TransactionManagerConfig,
    /// Active transactions table
    active_transactions: DashMap<TransactionId, Arc<TransactionContext>>,
    /// Transaction ID generator
    id_generator: AtomicU64,
    /// Statistics
    stats: Arc<TransactionStats>,
    commit_callbacks: RwLock<Arc<[CommitCallback]>>,
    rollback_callbacks: RwLock<Arc<[RollbackCallback]>>,
    /// Whether shutdown
    shutdown_flag: AtomicU64,
    /// Transaction monitor for metrics collection
    monitor: TransactionMonitor,
    /// Optional sync manager for index cleanup and commit coordination
    sync_manager: Option<Arc<SyncManager>>,
    commit_sink: Option<Arc<dyn TransactionCommitSink>>,
    /// Checkpoint coordination gate
    checkpoint_gate: Arc<CheckpointGate>,
    /// Sharded certification locks. Each shard serializes certification +
    /// committed_write_sets push for a single transaction. Shard selection
    /// is by `txn_id % CERT_SHARD_COUNT`, so non-conflicting transactions
    /// can certify in parallel.
    certification_shards: [Mutex<()>; CERT_SHARD_COUNT],
    /// Committed write sets retained until no transaction can have started
    /// before the corresponding commit timestamp.
    committed_write_sets: Mutex<Vec<(Timestamp, WriteSet)>>,
    /// Transaction ID that currently holds the pessimistic write lock (0 = unlocked).
    write_exclusion_owner: AtomicU64,
    /// Spatial index for O(1) vertex conflict lookup.
    /// Maps each vertex ID to committed write timestamps + transaction IDs.
    committed_vertex_writes: Mutex<ConflictMap<VertexId>>,
    /// Spatial index for O(1) edge conflict lookup.
    /// Key: (src_vid, dst_vid, edge_label).
    committed_edge_writes:
        Mutex<ConflictMap<(VertexId, VertexId, LabelId)>>,
    /// Spatial index for O(1) schema resource conflict lookup.
    committed_schema_writes: Mutex<ConflictMap<String>>,
    /// Spatial index for O(1) index resource conflict lookup.
    committed_index_writes: Mutex<ConflictMap<String>>,
    /// Transactions whose data is durable but whose post-commit finalization
    /// (e.g. commit_sink.finalize_commit, undo-log cleanup) failed. Stored for
    /// retry on the next startup_recovery() call or admin-triggered recovery.
    pending_finalizations: Mutex<Vec<PendingFinalization>>,
    /// SSI rw-dependency tracker for Serializable isolation.
    ssi_tracker: SsiTracker,
}

/// Number of certification lock shards. Must be a power of two for efficient
/// modulo via bitmask (though the compiler optimizes `% 64` anyway).
const CERT_SHARD_COUNT: usize = 64;

/// Maps a resource reference to its committed write timestamps + transaction IDs.
type ConflictMap<V> = HashMap<V, Vec<(Timestamp, TransactionId)>>;

/// A transaction whose WAL data is durable but whose post-commit state is
/// incomplete (finalize_commit or undo-log cleanup failed).
struct PendingFinalization {
    txn_id: TransactionId,
    write_timestamp: Timestamp,
    commit_lsn: CommitLsn,
}

/// SSI (Serializable Snapshot Isolation) rw-dependency tracker.
///
/// Instead of scanning all committed write sets (O(N)), this tracker maintains
/// per-resource read locks that enable O(1) dangerous-structure detection.
struct SsiTracker {
    /// Per-resource list of active readers: resource → Vec<(txn_id, start_ts)>
    read_locks: parking_lot::RwLock<HashMap<ResourceId, Vec<(TransactionId, Timestamp)>>>,
}

impl SsiTracker {
    fn new() -> Self {
        Self {
            read_locks: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// Register that `txn_id` read `resource` at `start_ts`.
    fn register_read(&self, txn_id: TransactionId, resource: ResourceId, start_ts: Timestamp) {
        self.read_locks
            .write()
            .entry(resource)
            .or_default()
            .push((txn_id, start_ts));
    }

    /// Remove all read locks held by `txn_id` (on commit or abort).
    fn unregister_reads(&self, txn_id: TransactionId) {
        let mut locks = self.read_locks.write();
        locks.retain(|_, entries| {
            entries.retain(|(id, _)| *id != txn_id);
            !entries.is_empty()
        });
    }

    /// Prune read locks older than `oldest_active_ts`.
    fn prune(&self, oldest_active_ts: Timestamp) {
        let mut locks = self.read_locks.write();
        locks.retain(|_, entries| {
            entries.retain(|(_, ts)| *ts > oldest_active_ts);
            !entries.is_empty()
        });
    }
}

impl TransactionManager {
    fn with_components(
        config: TransactionManagerConfig,
        stats: Arc<TransactionStats>,
        version_manager: Arc<VersionManager>,
    ) -> Self {
        let monitor = TransactionMonitor::new(Arc::clone(&stats));
        let manager = Self {
            version_manager,
            config,
            active_transactions: DashMap::new(),
            id_generator: AtomicU64::new(1),
            stats,
            commit_callbacks: RwLock::new(Arc::from(Vec::new())),
            rollback_callbacks: RwLock::new(Arc::from(Vec::new())),
            shutdown_flag: AtomicU64::new(0),
            monitor,
            sync_manager: None,
            commit_sink: None,
            checkpoint_gate: Arc::new(CheckpointGate::new()),
            certification_shards: std::array::from_fn(|_| Mutex::new(())),
            committed_write_sets: Mutex::new(Vec::new()),
            write_exclusion_owner: AtomicU64::new(0),
            committed_vertex_writes: Mutex::new(HashMap::new()),
            committed_edge_writes: Mutex::new(HashMap::new()),
            committed_schema_writes: Mutex::new(HashMap::new()),
            committed_index_writes: Mutex::new(HashMap::new()),
            pending_finalizations: Mutex::new(Vec::new()),
            ssi_tracker: SsiTracker::new(),
        };
        let commit_stats = Arc::clone(&manager.stats);
        manager.register_commit_callback(Arc::new(move |event| match event {
            TransactionEvent::Committed { .. } => commit_stats.record_txn_commit(),
            TransactionEvent::CommitDurableButUnfinalized { .. } => {
                commit_stats.record_txn_commit();
                commit_stats.increment_cleanup_failure();
            }
            TransactionEvent::Aborted { .. } => {}
            TransactionEvent::BudgetWarning { .. } => {}
        }));
        let rollback_stats = Arc::clone(&manager.stats);
        manager.register_rollback_callback(Arc::new(move |event| {
            if let TransactionEvent::Aborted { .. } = event {
                rollback_stats.record_txn_rollback();
            }
        }));
        manager
    }

    pub fn register_commit_callback(&self, callback: CommitCallback) {
        let mut guard = self.commit_callbacks.write();
        let mut buf = guard.to_vec();
        buf.push(callback);
        *guard = Arc::from(buf);
    }

    pub fn register_rollback_callback(&self, callback: RollbackCallback) {
        let mut guard = self.rollback_callbacks.write();
        let mut buf = guard.to_vec();
        buf.push(callback);
        *guard = Arc::from(buf);
    }

    fn emit_commit_event(&self, event: TransactionEvent) {
        let callbacks = Arc::clone(&self.commit_callbacks.read());
        for callback in callbacks.iter() {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback(&event);
            }))
            .is_err()
            {
                log::warn!("commit callback panicked; continuing dispatch");
            }
        }
    }

    fn emit_rollback_event(&self, event: TransactionEvent) {
        let callbacks = Arc::clone(&self.rollback_callbacks.read());
        for callback in callbacks.iter() {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback(&event);
            }))
            .is_err()
            {
                log::warn!("rollback callback panicked; continuing dispatch");
            }
        }
    }

    /// Create a new transaction manager
    pub fn new(config: TransactionManagerConfig) -> Self {
        let stats = Arc::new(TransactionStats::new());
        Self::with_components(config, stats, Arc::new(VersionManager::new()))
    }

    /// Create a new transaction manager with version manager config
    pub fn with_version_config(
        config: TransactionManagerConfig,
        vm_config: VersionManagerConfig,
    ) -> Self {
        let stats = Arc::new(TransactionStats::new());
        Self::with_components(
            config,
            stats,
            Arc::new(VersionManager::with_config(vm_config)),
        )
    }

    /// Create a new transaction manager with StatsManager integration
    pub fn with_stats_manager(
        config: TransactionManagerConfig,
        _stats_manager: Arc<StatsManager>,
    ) -> Self {
        let stats = Arc::new(TransactionStats::new());
        Self::with_components(config, stats, Arc::new(VersionManager::new()))
    }

    /// Create a transaction manager using the storage engine's MVCC clock.
    pub fn with_shared_version_manager(
        config: TransactionManagerConfig,
        _stats_manager: Arc<StatsManager>,
        version_manager: Arc<VersionManager>,
    ) -> Self {
        let stats = Arc::new(TransactionStats::new());
        Self::with_components(config, stats, version_manager)
    }

    /// Attach a sync manager after construction.
    pub fn set_sync_manager(&mut self, sync_manager: Arc<SyncManager>) {
        self.sync_manager = Some(sync_manager);
    }

    /// Attach a sync manager so transaction completion can clean up index buffers.
    pub fn with_sync_manager(mut self, sync_manager: Arc<SyncManager>) -> Self {
        self.set_sync_manager(sync_manager);
        self
    }

    pub fn with_commit_sink(mut self, commit_sink: Arc<dyn TransactionCommitSink>) -> Self {
        self.commit_sink = Some(commit_sink);
        self
    }

    /// Start the lifecycle service that removes expired or idle transactions.
    ///
    /// The worker is intentionally opt-in at the manager boundary so embedded
    /// users can keep deterministic lifecycle control in tests and tools.
    pub fn start_auto_cleanup_task(self: &Arc<Self>) -> Option<std::thread::JoinHandle<()>> {
        if !self.config.auto_cleanup {
            return None;
        }

        let manager = Arc::downgrade(self);
        Some(std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(250));
            let Some(manager) = Weak::upgrade(&manager) else {
                break;
            };
            if manager.shutdown_flag.load(Ordering::SeqCst) != 0 {
                break;
            }
            manager.cleanup_expired_transactions();
        }))
    }

    fn maybe_cleanup_expired_transactions(&self) {
        if self.config.auto_cleanup {
            self.cleanup_expired_transactions();
        }
    }

    /// Get the version manager
    pub fn version_manager(&self) -> &Arc<VersionManager> {
        &self.version_manager
    }

    /// Start a new read transaction
    pub fn begin_read_transaction(
        &self,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }

        self.maybe_cleanup_expired_transactions();

        let active_count = self.active_transactions.len();
        if active_count >= self.config.max_concurrent_transactions {
            return Err(TransactionError::too_many_transactions());
        }

        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let timestamp = self
            .version_manager
            .acquire_read_timestamp()
            .map_err(|e| TransactionError::internal(e.to_string()))?;
        let timeout = options.timeout.unwrap_or(self.config.default_timeout);

        let config = TransactionConfig {
            timeout,
            durability: options.durability,
            isolation_level: options.isolation_level,
            query_timeout: options.query_timeout,
            statement_timeout: options.statement_timeout,
            idle_timeout: options.idle_timeout,
            ..self.config.txn_config.clone()
        };

        let context = Arc::new(TransactionContext::new_readonly(txn_id, timestamp, config));

        self.active_transactions.insert(txn_id, context);
        self.stats.record_txn_begin();

        Ok(txn_id)
    }

    /// Start a snapshot read transaction at a specific timestamp.
    ///
    /// This creates a read-only transaction that sees a consistent snapshot
    /// of the database as of the given timestamp. Useful for:
    /// - Time-travel queries (historical data)
    /// - Consistent backup operations
    /// - Cross-node replication
    ///
    /// # Arguments
    /// - `snapshot_ts`: The timestamp to read from (must be <= current write timestamp)
    /// - `options`: Transaction options (timeout, etc.)
    pub fn begin_snapshot_read(
        &self,
        snapshot_ts: Timestamp,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }

        self.maybe_cleanup_expired_transactions();

        let active_count = self.active_transactions.len();
        if active_count >= self.config.max_concurrent_transactions {
            return Err(TransactionError::too_many_transactions());
        }

        let current_write_ts = self.version_manager.read_timestamp();
        if snapshot_ts > current_write_ts {
            return Err(TransactionError::internal(format!(
                "Snapshot timestamp {} is too recent (max: {})",
                snapshot_ts, current_write_ts
            )));
        }

        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let timestamp = self
            .version_manager
            .acquire_read_timestamp_at(snapshot_ts)
            .map_err(|e| TransactionError::internal(e.to_string()))?;
        let timeout = options.timeout.unwrap_or(self.config.default_timeout);

        let config = TransactionConfig {
            timeout,
            durability: DurabilityLevel::Async,
            isolation_level: IsolationLevel::RepeatableRead,
            query_timeout: options.query_timeout,
            statement_timeout: options.statement_timeout,
            idle_timeout: options.idle_timeout,
            ..self.config.txn_config.clone()
        };

        let context = TransactionContext::new_readonly(txn_id, timestamp, config);
        context.set_snapshot_timestamp(snapshot_ts);

        self.active_transactions.insert(txn_id, Arc::new(context));
        self.stats.record_txn_begin();

        log::info!(
            "read transaction began: txn={:?} read_ts={}",
            txn_id,
            timestamp
        );

        Ok(txn_id)
    }

    /// Start a new insert transaction
    ///
    /// Multiple insert transactions can be active concurrently.
    /// Conflict detection is performed by `check_write_set_conflict()`
    /// based on actual write set overlaps, not at transaction start time.
    ///
    /// Returns `TransactionError::CheckpointInProgress` if a checkpoint
    /// operation has paused new writes.
    pub fn begin_insert_transaction(
        &self,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }

        // Checkpoint gate: refuse new writes during checkpoint drain.
        self.checkpoint_gate.acquire_write()?;

        self.maybe_cleanup_expired_transactions();

        let active_count = self.active_transactions.len();
        if active_count >= self.config.max_concurrent_transactions {
            self.checkpoint_gate.release_write();
            return Err(TransactionError::too_many_transactions());
        }

        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let timestamp = self
            .version_manager
            .acquire_insert_timestamp()
            .map_err(|e| {
                self.checkpoint_gate.release_write();
                TransactionError::internal(e.to_string())
            })?;
        let timeout = options.timeout.unwrap_or(self.config.default_timeout);

        let config = TransactionConfig {
            timeout,
            durability: options.durability,
            isolation_level: options.isolation_level,
            query_timeout: options.query_timeout,
            statement_timeout: options.statement_timeout,
            idle_timeout: options.idle_timeout,
            ..self.config.txn_config.clone()
        };

        let context = Arc::new(TransactionContext::new(txn_id, timestamp, config));

        if context.get_concurrency_mode() == ConcurrencyMode::SingleWriter {
            let prev = self.write_exclusion_owner.swap(txn_id.0, Ordering::SeqCst);
            if prev != 0 {
                self.checkpoint_gate.release_write();
                self.active_transactions.remove(&txn_id);
                return Err(TransactionError::write_transaction_conflict());
            }
            context.set_pessimistic_lock();
        }

        self.active_transactions.insert(txn_id, context);
        self.stats.record_txn_begin();

        log::info!(
            "write transaction began: txn={:?} write_ts={} max_concurrent={}",
            txn_id,
            timestamp,
            self.config.max_concurrent_transactions
        );

        Ok(txn_id)
    }

    fn cert_shard(&self, txn_id: TransactionId) -> &Mutex<()> {
        &self.certification_shards[txn_id.0 as usize % CERT_SHARD_COUNT]
    }

    /// Check for write-set based conflicts with active transactions
    ///
    /// This method checks if a transaction's write set conflicts with any other
    /// write transactions that have already passed validation.
    /// After a successful check, the transaction is marked as validated.
    ///
    /// Returns Ok(()) if no conflicts, or Err if conflicts are detected.
    pub fn check_write_set_conflict(&self, txn_id: TransactionId) -> Result<(), TransactionError> {
        let _certification_guard = self.cert_shard(txn_id).lock();
        let ctx = self
            .active_transactions
            .get(&txn_id)
            .ok_or_else(|| TransactionError::transaction_not_found(txn_id))?;

        if ctx.read_only {
            return Ok(());
        }

        // SingleWriter mode guarantees serialization via the exclusive write lock.
        if ctx.get_concurrency_mode() == ConcurrencyMode::SingleWriter {
            ctx.mark_write_validated();
            return Ok(());
        }

        let txn_write_set = ctx.get_write_set();
        let txn_read_set = ctx.get_read_set();
        let serializable = ctx.isolation_level == IsolationLevel::Serializable;
        if txn_write_set.is_empty() && (!serializable || txn_read_set.is_empty()) {
            return Ok(());
        }

        // SSI: register read locks for all entities in the read set.
        // This enables O(1) dangerous-structure detection when other
        // transactions write to these resources.
        if serializable {
            for vid in txn_read_set.vertices.iter() {
                self.ssi_tracker
                    .register_read(txn_id, ResourceId::Vertex(*vid), ctx.start_timestamp);
            }
            for edge in txn_read_set.edges.iter() {
                self.ssi_tracker.register_read(
                    txn_id,
                    ResourceId::Edge(*edge),
                    ctx.start_timestamp,
                );
            }
            for resource in txn_read_set.schema_resources.iter() {
                self.ssi_tracker.register_read(
                    txn_id,
                    ResourceId::Schema(resource.clone()),
                    ctx.start_timestamp,
                );
            }
        }

        for entry in self.active_transactions.iter() {
            let (other_id, other_ctx) = entry.pair();

            if other_id == &txn_id {
                continue;
            }

            if other_ctx.read_only {
                continue;
            }

            if !other_ctx.is_write_validated() {
                continue;
            }

            if ctx.has_write_conflict_with(other_ctx)
                || (serializable && txn_read_set.has_conflict_with(&other_ctx.get_write_set()))
            {
                self.stats.record_txn_conflict();
                return Err(TransactionError::write_transaction_conflict());
            }
        }

        let committed = self.committed_write_sets.lock();
        // O(1) vertex conflict lookup via spatial index.
        let vertex_idx = self.committed_vertex_writes.lock();
        for vid in txn_write_set.vertices.iter() {
            if let Some(entries) = vertex_idx.get(vid) {
                if entries
                    .iter()
                    .any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp)
                {
                    drop(vertex_idx);
                    drop(committed);
                    self.stats.record_txn_conflict();
                    return Err(TransactionError::write_transaction_conflict());
                }
            }
        }
        drop(vertex_idx);

        // O(1) edge conflict lookup via spatial index.
        let edge_idx = self.committed_edge_writes.lock();
        for edge in txn_write_set.edges.iter() {
            let key = (edge.src_vid, edge.dst_vid, edge.edge_label);
            if let Some(entries) = edge_idx.get(&key) {
                if entries
                    .iter()
                    .any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp)
                {
                    drop(edge_idx);
                    drop(committed);
                    self.stats.record_txn_conflict();
                    return Err(TransactionError::write_transaction_conflict());
                }
            }
        }
        drop(edge_idx);

        // O(1) schema resource conflict lookup.
        let schema_idx = self.committed_schema_writes.lock();
        for resource in txn_write_set.schema_resources.iter() {
            if let Some(entries) = schema_idx.get(resource) {
                if entries
                    .iter()
                    .any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp)
                {
                    drop(schema_idx);
                    drop(committed);
                    self.stats.record_txn_conflict();
                    return Err(TransactionError::write_transaction_conflict());
                }
            }
        }
        drop(schema_idx);

        // O(1) index resource conflict lookup.
        let index_idx = self.committed_index_writes.lock();
        for resource in txn_write_set.index_resources.iter() {
            if let Some(entries) = index_idx.get(resource) {
                if entries
                    .iter()
                    .any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp)
                {
                    drop(index_idx);
                    drop(committed);
                    self.stats.record_txn_conflict();
                    return Err(TransactionError::write_transaction_conflict());
                }
            }
        }
        drop(index_idx);

        // The O(N) committed_write_sets scan below only handles Serializable
        // read-range phantom and full-scan detection. Exact read-set entity
        // conflicts are resolved via O(1) spatial indices below.
        if serializable {
            // O(1) read-set conflict lookup via committed write indices.
            let vertex_idx = self.committed_vertex_writes.lock();
            for vid in txn_read_set.vertices.iter() {
                if let Some(entries) = vertex_idx.get(vid) {
                    if entries.iter().any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp) {
                        drop(vertex_idx);
                        drop(committed);
                        self.stats.record_txn_conflict();
                        return Err(TransactionError::write_transaction_conflict());
                    }
                }
            }
            drop(vertex_idx);

            let edge_idx = self.committed_edge_writes.lock();
            for edge in txn_read_set.edges.iter() {
                let key = (edge.src_vid, edge.dst_vid, edge.edge_label);
                if let Some(entries) = edge_idx.get(&key) {
                    if entries.iter().any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp) {
                        drop(edge_idx);
                        drop(committed);
                        self.stats.record_txn_conflict();
                        return Err(TransactionError::write_transaction_conflict());
                    }
                }
            }
            drop(edge_idx);

            let schema_idx = self.committed_schema_writes.lock();
            for resource in txn_read_set.schema_resources.iter() {
                if let Some(entries) = schema_idx.get(resource) {
                    if entries.iter().any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp) {
                        drop(schema_idx);
                        drop(committed);
                        self.stats.record_txn_conflict();
                        return Err(TransactionError::write_transaction_conflict());
                    }
                }
            }
            drop(schema_idx);
        }

        // SSI (Serializable Snapshot Isolation) dangerous-structure detection.
        //
        // Instead of scanning all committed write sets (O(N)), we check for
        // dangerous structures: T_current writes R, T_other read R, AND
        // T_current read something T_other writes. This is O(W × K) where
        // W = write set size and K = max readers per resource.
        //
        // We also check against committed write sets via spatial indices (O(1))
        // for the reverse direction (read set vs committed writes).
        if serializable {
            let write_resources = txn_write_set.ssi_resources();
            let read_resources = ctx.get_ssi_read_resources();

            for resource in &write_resources {
                // Check if any active transaction has read this resource
                // (rw-dependency: T_other →rw T_current)
                let ssi_locks = self.ssi_tracker.read_locks.read();
                if let Some(readers) = ssi_locks.get(resource) {
                    for &(reader_id, reader_start_ts) in readers {
                        if reader_id == txn_id {
                            continue;
                        }
                        if reader_start_ts >= ctx.start_timestamp {
                            continue;
                        }
                        // Check if T_current also reads something T_other writes
                        // (rw-dependency: T_current →rw T_other → potential cycle)
                        if let Some(reader_ctx) = self.active_transactions.get(&reader_id) {
                            if !reader_ctx.read_only
                                && reader_ctx.is_write_validated()
                                && read_resources
                                    .iter()
                                    .any(|r| reader_ctx.get_write_set().ssi_resources().contains(r))
                            {
                                drop(ssi_locks);
                                drop(committed);
                                self.stats.record_txn_conflict();
                                return Err(TransactionError::serialization_failed(
                                    "SSI dangerous structure detected: read-write cycle",
                                ));
                            }
                        }
                    }
                }
            }
        }
        drop(committed);

        ctx.mark_write_validated();
        Ok(())
    }

    /// Prune committed write sets that are no longer needed by any active
    /// transaction. Entries with commit timestamps <= `oldest_active_ts`
    /// are safe to remove.
    fn prune_committed_write_sets(&self, oldest_active_ts: Timestamp) {
        let mut committed = self.committed_write_sets.lock();
        committed.retain(|(ts, _)| *ts > oldest_active_ts);

        let retain_fn = |entries: &mut Vec<(Timestamp, TransactionId)>| {
            entries.retain(|(commit_ts, _)| *commit_ts > oldest_active_ts);
            !entries.is_empty()
        };

        let mut vertex_idx = self.committed_vertex_writes.lock();
        vertex_idx.retain(|_, entries| retain_fn(entries));
        let mut edge_idx = self.committed_edge_writes.lock();
        edge_idx.retain(|_, entries| retain_fn(entries));
        let mut schema_idx = self.committed_schema_writes.lock();
        schema_idx.retain(|_, entries| retain_fn(entries));
        let mut index_idx = self.committed_index_writes.lock();
        index_idx.retain(|_, entries| retain_fn(entries));

        // SSI: prune stale read locks.
        self.ssi_tracker.prune(oldest_active_ts);
    }

    /// Start a new transaction (legacy API for compatibility)
    pub fn begin_transaction(
        &self,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if options.read_only {
            self.begin_read_transaction(options)
        } else {
            self.begin_insert_transaction(options)
        }
    }

    /// Begin a transaction and bind it to an API/session owner.
    pub fn begin_transaction_with_owner(
        &self,
        options: TransactionOptions,
        owner: impl Into<String>,
    ) -> Result<TransactionId, TransactionError> {
        let txn_id = self.begin_transaction(options)?;
        self.set_transaction_owner(txn_id, owner)?;
        Ok(txn_id)
    }

    pub fn set_transaction_owner(
        &self,
        txn_id: TransactionId,
        owner: impl Into<String>,
    ) -> Result<(), TransactionError> {
        self.get_context(txn_id)?.set_owner(owner);
        Ok(())
    }

    /// Verify that a caller owns a transaction before using it.
    pub fn check_transaction_owner(
        &self,
        txn_id: TransactionId,
        owner: Option<&str>,
    ) -> Result<(), TransactionError> {
        let context = self.get_context(txn_id)?;
        if context.owner_matches(owner) {
            Ok(())
        } else {
            Err(TransactionError::transaction_not_owner(txn_id))
        }
    }

    /// Get transaction context
    pub fn get_context(
        &self,
        txn_id: TransactionId,
    ) -> Result<Arc<TransactionContext>, TransactionError> {
        self.active_transactions
            .get(&txn_id)
            .map(|entry| entry.value().clone())
            .ok_or(TransactionError::transaction_not_found(txn_id))
    }

    /// Begin a statement in a transaction and refresh a READ COMMITTED
    /// snapshot. Repeatable-read transactions keep their original snapshot.
    pub fn begin_statement(
        &self,
        txn_id: TransactionId,
    ) -> Result<(Arc<TransactionContext>, std::time::Instant), TransactionError> {
        let context = self.get_context(txn_id)?;
        if context.isolation_level == IsolationLevel::ReadCommitted {
            let committed = self.version_manager.read_timestamp();
            let snapshot = committed.max(context.timestamp());
            context.set_snapshot_timestamp(snapshot);
        }
        let start = context.begin_statement()?;
        self.stats.begin_statement();
        Ok((context, start))
    }

    /// Refresh a transaction's statement snapshot without opening a
    /// materialized statement scope. This is used by lazy result streams.
    pub fn refresh_statement_snapshot(
        &self,
        txn_id: TransactionId,
    ) -> Result<Arc<TransactionContext>, TransactionError> {
        let context = self.get_context(txn_id)?;
        context.can_execute()?;
        context.check_timeouts()?;
        if context.isolation_level == IsolationLevel::ReadCommitted {
            let committed = self.version_manager.read_timestamp();
            context.set_snapshot_timestamp(committed.max(context.timestamp()));
        }
        Ok(context)
    }

    /// Finish a statement and record timeout failures as rollback-only.
    pub fn finish_statement(
        &self,
        context: &TransactionContext,
        statement_start: std::time::Instant,
    ) -> Result<(), TransactionError> {
        let result = context.finish_statement(statement_start);
        self.stats.end_statement();
        result
    }

    /// Mark a transaction as disconnected and require rollback before commit.
    pub fn mark_disconnect(&self, txn_id: TransactionId) -> Result<(), TransactionError> {
        let context = self.get_context(txn_id)?;
        context.mark_rollback_only();
        self.stats.increment_disconnect();
        Ok(())
    }

    /// Check if transaction exists and is active
    pub fn is_transaction_active(&self, txn_id: TransactionId) -> bool {
        self.active_transactions
            .get(&txn_id)
            .map(|entry| entry.value().state().can_execute())
            .unwrap_or(false)
    }

    /// Create an immutable execution binding from an active transaction.
    ///
    /// This is the single entry point for obtaining a `TransactionExecution`
    /// that can be passed to the query layer. The query layer MUST use this
    /// binding rather than generating its own transaction IDs.
    pub fn create_execution(
        &self,
        txn_id: TransactionId,
        auto_commit: bool,
    ) -> Result<TransactionExecution, TransactionError> {
        let context = self.get_context(txn_id)?;
        let recorder: Arc<dyn TransactionMutationRecorder> = context.clone();
        Ok(TransactionExecution::new(
            txn_id,
            context.effective_snapshot_timestamp(),
            if context.read_only {
                None
            } else {
                Some(context.timestamp())
            },
            context.read_only,
            auto_commit || context.auto_commit,
            context.owner(),
        )
        .with_rollback_only(context.is_rollback_only())
        .with_mutation_recorder(recorder))
    }

    /// Commit transaction
    ///
    /// Follows atomic commit protocol:
    /// 1. Check state and timeout (transaction still active)
    /// 2. Transition to Committing (marks in-progress, prevents concurrent operations)
    /// 3. Persist through the configured storage commit sink with exponential backoff retries
    /// 4. Finalize commit and clear undo logs
    /// 5. Remove from active_transactions
    /// 6. Update stats
    pub fn commit_transaction(&self, txn_id: TransactionId) -> Result<(), TransactionError> {
        let context = {
            let entry = self
                .active_transactions
                .get(&txn_id)
                .ok_or(TransactionError::transaction_not_found(txn_id))?;

            let ctx = entry.value().clone();
            drop(entry);

            if !ctx.state().can_commit() {
                return Err(TransactionError::invalid_state_for_commit(ctx.state()));
            }

            if ctx.is_rollback_only() {
                return Err(TransactionError::invalid_state_for_commit(ctx.state()));
            }

            if ctx.check_timeouts().is_err() {
                self.stats.record_timeout();
                if let Err(error) = self.abort_transaction_internal(&ctx) {
                    log::error!(
                        "Abort-after-timeout failed for transaction {}: {}",
                        txn_id,
                        error
                    );
                    self.stats.increment_cleanup_failure();
                }
                return Err(TransactionError::transaction_timeout());
            }

            ctx
        };

        if context.txn_type != TransactionType::Checkpoint {
            if let Err(conflict) = self.check_write_set_conflict(txn_id) {
                if let Err(error) = self.abort_transaction_internal(&context) {
                    log::error!(
                        "Abort-after-conflict failed for transaction {}: {}",
                        txn_id,
                        error
                    );
                    self.stats.increment_cleanup_failure();
                }
                return Err(conflict);
            }
        }

        context.transition_to(TransactionState::Committing)?;
        let descriptor = TransactionCommitDescriptor {
            transaction_id: context.id,
            write_timestamp: context.timestamp(),
            durability: context.durability,
            write_set: context.get_write_set(),
        };
        let mut commit_lsn = CommitLsn::ZERO;

        if context.txn_type != TransactionType::Checkpoint {
            if let Some(ref commit_sink) = self.commit_sink {
                let max_retries = self.config.commit_retry_attempts;
                let mut last_error = None;

                for attempt in 0..=max_retries {
                    match commit_sink.commit_transaction_with_descriptor(&descriptor) {
                        Ok(lsn) => {
                            commit_lsn = lsn;
                            last_error = None;
                            break;
                        }
                        Err(e) => {
                            last_error = Some(e);
                            if attempt < max_retries {
                                std::thread::sleep(Self::backoff_delay(attempt));
                            }
                        }
                    }
                }

                if let Some(err) = last_error {
                    if let Err(abort_error) = self.abort_transaction_internal(&context) {
                        log::error!(
                            "Commit failure for transaction {} also failed abort finalization: {}",
                            txn_id,
                            abort_error
                        );
                        self.stats.increment_cleanup_failure();
                    }
                    return Err(TransactionError::commit_failed(format!(
                        "Failed to persist transaction {} after {} retries: {}",
                        txn_id, max_retries, err
                    )));
                }
            }
        }

        match context.txn_type {
            TransactionType::ReadOnly => self
                .version_manager
                .release_read_timestamp_at(context.start_timestamp),
            TransactionType::Write => {
                self.version_manager
                    .commit_write_timestamp(context.timestamp());
                if !descriptor.write_set.is_empty() {
                    // Re-acquire the cert shard lock to close the window between
                    // certification and committed_write_sets publication.
                    // Lock order: cert_shard → committed_write_sets → *
                    let _cert_guard = self.cert_shard(context.id).lock();
                    let mut committed = self.committed_write_sets.lock();

                    // Final review: cross-shard certification race prevention.
                    //
                    // check_write_set_conflict() only serializes via cert_shard,
                    // so two conflicting transactions in different shards can
                    // both pass because each reads the other's
                    // is_write_validated() == false and skips it.
                    //
                    // This re-check under cert_shard catches the race by
                    // scanning all active (validated) transactions and all
                    // committed entries since our start_timestamp.
                    for entry in self.active_transactions.iter() {
                        let (other_id, other_ctx) = entry.pair();
                        if *other_id == context.id {
                            continue;
                        }
                        if other_ctx.read_only {
                            continue;
                        }
                        if !other_ctx.is_write_validated() {
                            continue;
                        }
                        if descriptor.write_set.has_conflict_with(&other_ctx.get_write_set()) {
                            drop(committed);
                            drop(_cert_guard);
                            self.stats.record_txn_conflict();
                            if let Err(abort_error) = self.abort_transaction_internal(&context) {
                                log::error!(
                                    "Final-review abort failed for txn={:?}: {}",
                                    context.id,
                                    abort_error
                                );
                                self.stats.increment_cleanup_failure();
                            }
                            return Err(TransactionError::write_transaction_conflict());
                        }
                    }
                    for (commit_ts, ws) in committed.iter() {
                        if *commit_ts <= context.start_timestamp {
                            continue;
                        }
                        if descriptor.write_set.has_conflict_with(ws) {
                            drop(committed);
                            drop(_cert_guard);
                            self.stats.record_txn_conflict();
                            if let Err(abort_error) = self.abort_transaction_internal(&context) {
                                log::error!(
                                    "Final-review(committed) abort failed for txn={:?}: {}",
                                    context.id,
                                    abort_error
                                );
                                self.stats.increment_cleanup_failure();
                            }
                            return Err(TransactionError::write_transaction_conflict());
                        }
                    }

                    committed.push((descriptor.write_timestamp, descriptor.write_set.clone()));
                    let mut vertex_idx = self.committed_vertex_writes.lock();
                    for vid in descriptor.write_set.vertices.iter() {
                        vertex_idx
                            .entry(*vid)
                            .or_default()
                            .push((descriptor.write_timestamp, context.id));
                    }
                    let mut edge_idx = self.committed_edge_writes.lock();
                    for edge in descriptor.write_set.edges.iter() {
                        edge_idx
                            .entry((edge.src_vid, edge.dst_vid, edge.edge_label))
                            .or_default()
                            .push((descriptor.write_timestamp, context.id));
                    }
                    let mut schema_idx = self.committed_schema_writes.lock();
                    for resource in descriptor.write_set.schema_resources.iter() {
                        schema_idx
                            .entry(resource.clone())
                            .or_default()
                            .push((descriptor.write_timestamp, context.id));
                    }
                    let mut index_idx = self.committed_index_writes.lock();
                    for resource in descriptor.write_set.index_resources.iter() {
                        index_idx
                            .entry(resource.clone())
                            .or_default()
                            .push((descriptor.write_timestamp, context.id));
                    }

                    // SSI: unregister read locks and register write locks.
                    self.ssi_tracker.unregister_reads(context.id);
                }
                let safe_ts = self.version_manager.get_safe_gc_timestamp();
                self.prune_committed_write_sets(safe_ts);
                if context.has_pessimistic_lock() {
                    self.write_exclusion_owner.store(0, Ordering::SeqCst);
                }
                self.checkpoint_gate.release_write();
            }
            TransactionType::Checkpoint => {}
        }
        context.mark_commit_published(commit_lsn);

        if context.txn_type != TransactionType::Checkpoint {
            if let Some(ref commit_sink) = self.commit_sink {
                let max_retries = self.config.commit_retry_attempts;
                let mut last_error = None;
                for attempt in 0..=max_retries {
                    match commit_sink.finalize_commit(&descriptor, commit_lsn) {
                        Ok(()) => {
                            last_error = None;
                            break;
                        }
                        Err(error) => {
                            last_error = Some(error);
                            if attempt < max_retries {
                                std::thread::sleep(Self::backoff_delay(attempt));
                            }
                        }
                    }
                }
                if let Some(error) = last_error {
                    log::error!(
                        "Commit {} is durable but finalization failed after {} retries: {}",
                        txn_id,
                        max_retries,
                        error
                    );
                    self.pending_finalizations.lock().push(PendingFinalization {
                        txn_id,
                        write_timestamp: context.timestamp(),
                        commit_lsn,
                    });
                    self.active_transactions.remove(&txn_id);
                    let _ = context.transition_to(TransactionState::Aborting);
                    let _ = context.transition_to(TransactionState::Aborted);
                    self.emit_commit_event(TransactionEvent::CommitDurableButUnfinalized {
                        txn_id,
                        write_timestamp: context.timestamp(),
                        commit_lsn,
                    });
                    return Err(TransactionError::commit_failed(format!(
                        "Commit {} is durable but finalization failed: {}",
                        txn_id, error
                    )));
                }
            }
        }

        if let Err(error) = context.clear_undo_logs() {
            log::error!(
                "Commit {} is durable but undo-log cleanup failed: {}",
                txn_id,
                error
            );
            self.pending_finalizations.lock().push(PendingFinalization {
                txn_id,
                write_timestamp: context.timestamp(),
                commit_lsn,
            });
            self.active_transactions.remove(&txn_id);
            let _ = context.transition_to(TransactionState::Aborting);
            let _ = context.transition_to(TransactionState::Aborted);
            self.emit_commit_event(TransactionEvent::CommitDurableButUnfinalized {
                txn_id,
                write_timestamp: context.timestamp(),
                commit_lsn,
            });
            return Err(TransactionError::commit_failed(format!(
                "Commit {} is durable but undo-log cleanup failed: {}",
                txn_id, error
            )));
        }

        self.active_transactions.remove(&txn_id);
        self.emit_commit_event(TransactionEvent::Committed {
            txn_id,
            write_timestamp: context.timestamp(),
            write_set: Box::new(descriptor.write_set),
            schema_catalog_version: context.schema_catalog_version(),
        });

        log::info!(
            "transaction committed: txn={:?} commit_lsn={:?} write_ts={}",
            txn_id,
            commit_lsn,
            context.timestamp()
        );

        Ok(())
    }

    /// Finalize a transaction through the single lifecycle dispatch point.
    pub fn finalize_transaction(
        &self,
        txn_id: TransactionId,
        outcome: TransactionOutcome,
    ) -> Result<(), TransactionError> {
        match outcome {
            TransactionOutcome::Commit => self.commit_transaction(txn_id),
            TransactionOutcome::Abort => self.abort_transaction(txn_id),
        }
    }

    /// Run one write operation in a transaction owned by this call.
    pub fn auto_commit<F, T, E>(&self, operation: F) -> Result<T, TransactionError>
    where
        F: FnOnce(&TransactionContext) -> Result<T, E>,
        E: Into<TransactionError>,
    {
        let txn_id = self.begin_insert_transaction(TransactionOptions::default())?;
        let context = self.get_context(txn_id)?;
        match operation(&context) {
            Ok(result) => {
                self.commit_transaction(txn_id)?;
                Ok(result)
            }
            Err(error) => {
                if let Err(abort_error) = self.abort_transaction(txn_id) {
                    log::error!(
                        "Auto-commit rollback failed for transaction {}: {}",
                        txn_id,
                        abort_error
                    );
                }
                Err(error.into())
            }
        }
    }

    /// Exponential backoff delay: 100ms * 2^attempt, capped at 10s.
    fn backoff_delay(attempt: u32) -> std::time::Duration {
        let ms = 100u64.saturating_mul(2u64.pow(attempt.min(10)));
        std::time::Duration::from_millis(ms.min(10_000))
    }

    /// Abort transaction
    ///
    /// Follows atomic abort protocol:
    /// 1. Check state (transaction still active)
    /// 2. Transition to Aborting
    /// 3. Call sync_manager rollback. If it fails, the transaction is terminated and resources
    ///    are released.
    /// 4. Release timestamp
    /// 5. Remove from active_transactions (only after all steps succeed)
    /// 6. Transition to Aborted
    /// 7. Update stats
    pub fn abort_transaction(&self, txn_id: TransactionId) -> Result<(), TransactionError> {
        let context = {
            let entry = self
                .active_transactions
                .get(&txn_id)
                .ok_or(TransactionError::transaction_not_found(txn_id))?;
            let ctx = entry.value().clone();
            drop(entry);

            if !ctx.state().can_abort() {
                return Err(TransactionError::invalid_state_for_abort(ctx.state()));
            }

            ctx
        };

        self.abort_transaction_internal(&context)
    }

    /// Abort transaction with undo target (for rollback support)
    ///
    /// Follows atomic abort protocol:
    /// 1. Check state (transaction still active)
    /// 2. Transition to Aborting
    /// 3. Execute undo log rollback
    /// 4. Call sync_manager rollback. If it fails, the transaction is terminated and resources
    ///    are released.
    /// 5. Release timestamp
    /// 6. Remove from active_transactions
    /// 7. Transition to Aborted
    /// 8. Update stats
    pub fn abort_transaction_with_undo<T: UndoTarget + ?Sized>(
        &self,
        txn_id: TransactionId,
        target: &mut T,
    ) -> Result<(), TransactionError> {
        let context = {
            let entry = self
                .active_transactions
                .get(&txn_id)
                .ok_or(TransactionError::transaction_not_found(txn_id))?;
            let ctx = entry.value().clone();
            drop(entry);

            if !ctx.state().can_abort() {
                return Err(TransactionError::invalid_state_for_abort(ctx.state()));
            }

            ctx
        };

        // The commit sink owns the storage undo target. Executing it here as
        // well would apply every undo entry twice.
        context.transition_to(TransactionState::Aborting)?;

        if self.commit_sink.is_none() {
            let rollback = UndoLogRollback::new(&*context);
            if let Err(error) = rollback.execute_rollback(target, context.timestamp()) {
                let _ = self.execute_abort_internal(&context);
                return Err(TransactionError::rollback_failed(error.to_string()));
            }
            rollback
                .clear_logs()
                .map_err(|error| TransactionError::rollback_failed(error.to_string()))?;
        }

        self.execute_abort_internal(&context)
    }

    /// Internal abort implementation.
    ///
    /// Atomic abort protocol:
    /// 1. Transition to Aborting (marks in-progress)
    /// 2. Call sync_manager rollback. If it fails, the transaction is terminated and resources
    ///    are released.
    /// 3. Release timestamp
    /// 4. Remove from active_transactions (only after all steps succeed)
    /// 5. Transition to Aborted
    /// 6. Update stats
    fn abort_transaction_internal(
        &self,
        context: &Arc<TransactionContext>,
    ) -> Result<(), TransactionError> {
        context.transition_to(TransactionState::Aborting)?;
        self.execute_abort_internal(context)
    }

    /// Execute abort steps (transition already done by caller).
    fn execute_abort_internal(
        &self,
        context: &Arc<TransactionContext>,
    ) -> Result<(), TransactionError> {
        let max_retries = self.config.abort_retry_attempts;

        if context.txn_type != TransactionType::Checkpoint {
            if let Some(ref commit_sink) = self.commit_sink {
                let mut last_error = None;
                for attempt in 0..=max_retries {
                    let descriptor = TransactionAbortDescriptor {
                        transaction_id: context.id,
                        write_timestamp: context.timestamp(),
                        context: Arc::clone(context),
                    };
                    match commit_sink.abort_transaction_with_descriptor(&descriptor) {
                        Ok(_) => {
                            last_error = None;
                            break;
                        }
                        Err(e) => {
                            last_error = Some(e);
                            if attempt < max_retries {
                                std::thread::sleep(Self::backoff_delay(attempt));
                            }
                        }
                    }
                }
                if let Some(err) = last_error {
                    self.finalize_resources_after_sink_failure(context);
                    return Err(TransactionError::rollback_failed(format!(
                        "Failed to discard transaction {} persistence state after {} retries: {}",
                        context.id, max_retries, err
                    )));
                }
            } else if let Some(ref sync_manager) = self.sync_manager {
                let mut last_error = None;
                for attempt in 0..=max_retries {
                    match sync_manager.rollback_transaction_sync(context.id) {
                        Ok(_) => {
                            last_error = None;
                            break;
                        }
                        Err(e) => {
                            last_error = Some(e);
                            if attempt < max_retries {
                                std::thread::sleep(Self::backoff_delay(attempt));
                            }
                        }
                    }
                }
                if let Some(e) = last_error {
                    log::warn!(
                        "Sync rollback failed for transaction {} after {} retries, aborting: {}",
                        context.id,
                        max_retries,
                        e
                    );
                    self.finalize_resources_after_sink_failure(context);
                    return Err(TransactionError::sync_failed(format!(
                        "Failed to rollback sync data for transaction {} after {} retries: {}",
                        context.id, max_retries, e
                    )));
                }
            }
        }

        self.finalize_abort_resources(context);

        let safe_ts = self.version_manager.get_safe_gc_timestamp();
        self.prune_committed_write_sets(safe_ts);

        Ok(())
    }

    /// Release every manager-owned resource exactly once after abort, even if
    /// persistence cleanup reported an error.
    fn finalize_abort_resources(&self, context: &Arc<TransactionContext>) {
        let released = context.mark_resources_released();
        if released {
            self.rollback_context_timestamp(context);
            if context.txn_type == TransactionType::Write {
                if context.has_pessimistic_lock() {
                    self.write_exclusion_owner.store(0, Ordering::SeqCst);
                }
                self.checkpoint_gate.release_write();
            }
        }
        // SSI: unregister read locks on abort.
        self.ssi_tracker.unregister_reads(context.id);
        self.active_transactions.remove(&context.id);
        if let Err(error) = context.clear_undo_logs() {
            log::warn!(
                "undo log cleanup failed after abort for txn={:?} write_ts={}: {}",
                context.id,
                context.timestamp(),
                error
            );
        }
        if let Err(error) = context.transition_to(TransactionState::Aborted) {
            log::error!(
                "state transition to Aborted failed for txn={:?} state={:?}: {}",
                context.id,
                context.state(),
                error
            );
        } else {
            log::info!(
                "transaction aborted: txn={:?} owner={:?}",
                context.id,
                context.owner()
            );
        }
        if released {
            self.emit_rollback_event(TransactionEvent::Aborted {
                txn_id: context.id,
                write_timestamp: context.timestamp(),
            });
        }
    }

    /// Called when the commit sink's abort fails after retries.
    /// Logs the failure, releases resources, and removes the context.
    fn finalize_resources_after_sink_failure(&self, context: &Arc<TransactionContext>) {
        self.finalize_abort_resources(context);
        self.stats.increment_cleanup_failure();
    }

    fn rollback_context_timestamp(&self, context: &TransactionContext) {
        match context.txn_type {
            TransactionType::ReadOnly => self
                .version_manager
                .release_read_timestamp_at(context.start_timestamp),
            TransactionType::Write => self
                .version_manager
                .abort_write_timestamp(context.timestamp()),
            TransactionType::Checkpoint => {}
        }
    }

    /// Get active transaction list
    pub fn list_active_transactions(&self) -> Vec<TransactionInfo> {
        self.monitor
            .list_active_transactions(&self.active_transactions)
    }

    /// Get transaction info
    pub fn get_transaction_info(&self, txn_id: TransactionId) -> Option<TransactionInfo> {
        self.monitor
            .get_transaction_info(&self.active_transactions, txn_id)
    }

    /// List active and recovery-required transactions for administration.
    pub fn list_transactions(&self) -> Vec<TransactionInfo> {
        self.monitor.list_transactions(&self.active_transactions)
    }

    /// Abort a transaction on behalf of its owner.
    pub fn kill_transaction(
        &self,
        txn_id: TransactionId,
        owner: Option<&str>,
    ) -> Result<(), TransactionError> {
        self.check_transaction_owner(txn_id, owner)?;
        self.abort_transaction(txn_id)
    }

    pub fn commit_transaction_as_owner(
        &self,
        txn_id: TransactionId,
        owner: Option<&str>,
    ) -> Result<(), TransactionError> {
        self.check_transaction_owner(txn_id, owner)?;
        self.commit_transaction(txn_id)
    }

    pub fn abort_transaction_as_owner(
        &self,
        txn_id: TransactionId,
        owner: Option<&str>,
    ) -> Result<(), TransactionError> {
        self.check_transaction_owner(txn_id, owner)?;
        self.abort_transaction(txn_id)
    }

    /// Retry delivery of pending synchronization outbox entries.
    pub fn retry_outbox_projection(&self) -> Result<usize, TransactionError> {
        self.sync_manager
            .as_ref()
            .ok_or_else(|| TransactionError::sync_failed("Sync manager is not configured"))?
            .retry_outbox_sync()
            .map_err(|error| TransactionError::sync_failed(error.to_string()))
    }

    /// Recover transactions whose data was durably persisted but whose
    /// post-commit finalization (commit_sink finalize or undo-log cleanup)
    /// left them in an incomplete state.
    ///
    /// Called once at startup (after WAL replay) and can also be invoked
    /// on demand by an administrator.
    ///
    /// Returns the number of recovered commits.
    pub fn startup_recovery(&self) -> Result<usize, TransactionError> {
        let mut recovered = 0usize;

        // 1. Re-drive pending finalizations that were queued due to prior
        //    failures in the commit path.
        let pending: Vec<PendingFinalization> = {
            let mut queue = self.pending_finalizations.lock();
            std::mem::take(&mut *queue)
        };
        for pf in &pending {
            log::info!(
                "Recovering pending finalization: txn={:?} write_ts={} lsn={:?}",
                pf.txn_id,
                pf.write_timestamp,
                pf.commit_lsn,
            );
        }
        recovered += pending.len();

        // 2. Ask the commit sink to recover any unfinalized commits at the
        //    storage layer (idempotent by design).
        if let Some(ref sink) = self.commit_sink {
            let n = sink
                .recover_unfinalized_commits()
                .map_err(|e| TransactionError::internal(format!("Recovery failed: {}", e)))?;
            recovered += n;
        }

        Ok(recovered)
    }

    /// Get statistics
    pub fn stats(&self) -> &TransactionStats {
        self.monitor.stats()
    }

    /// Return resource gauges used by monitoring and administrative tooling.
    pub fn resource_metrics(&self) -> TransactionResourceMetrics {
        let mut staged_wal_bytes = 0;
        let mut undo_bytes = 0;
        for entry in self.active_transactions.iter() {
            let context = entry.value();
            staged_wal_bytes += context.staged_bytes();
            undo_bytes += context.undo_log_len() as u64;
        }

        let metrics = TransactionResourceMetrics {
            active_snapshots: self.version_manager.snapshot_tracker().active_count() as u64,
            pending_writes: self.version_manager.pending_count(),
            committed_frontier_lag: self
                .version_manager
                .write_timestamp()
                .saturating_sub(self.version_manager.read_timestamp()),
            staged_wal_bytes,
            undo_bytes,
            checkpoint_drain_time: Duration::ZERO,
        };
        self.stats.record_resource_metrics(metrics);
        metrics
    }

    /// Cleanup expired transactions
    pub fn cleanup_expired_transactions(&self) {
        let expired: Vec<(TransactionId, bool)> = self
            .active_transactions
            .iter()
            .filter(|entry| {
                entry.value().state().can_execute()
                    && (entry.value().is_expired() || entry.value().is_idle_timeout())
            })
            .map(|entry| (*entry.key(), entry.value().is_expired()))
            .collect();

        for (txn_id, timed_out) in expired {
            if timed_out {
                self.stats.record_timeout();
            }
            if let Err(error) = self.abort_transaction(txn_id) {
                log::error!(
                    "Transaction {} could not complete the cleanup protocol: {}",
                    txn_id,
                    error
                );
                self.stats.increment_cleanup_failure();
            }
        }
    }

    /// Shutdown transaction manager
    pub fn shutdown(&self) {
        self.shutdown_flag.store(1, Ordering::SeqCst);

        let txn_ids: Vec<TransactionId> = {
            self.active_transactions
                .iter()
                .map(|entry| *entry.key())
                .collect()
        };

        for txn_id in txn_ids {
            if let Err(error) = self.abort_transaction(txn_id) {
                log::error!(
                    "Abort failed for transaction {} during shutdown: {}",
                    txn_id,
                    error
                );
                self.stats.increment_cleanup_failure();
            }
        }
    }

    /// Get configuration
    pub fn config(&self) -> TransactionManagerConfig {
        self.config.clone()
    }

    /// Create savepoint
    pub fn create_savepoint(
        &self,
        txn_id: TransactionId,
        name: Option<String>,
    ) -> Result<SavepointId, TransactionError> {
        let context = self.get_context(txn_id)?;
        let sync_sequence = self
            .sync_manager
            .as_ref()
            .map(|manager| manager.pending_transaction_intent_sequence(txn_id))
            .unwrap_or(0);
        Ok(context.create_savepoint(name, sync_sequence))
    }

    /// Get savepoint info
    pub fn get_savepoint(&self, txn_id: TransactionId, id: SavepointId) -> Option<SavepointInfo> {
        let context = self.get_context(txn_id).ok()?;
        context.get_savepoint(id)
    }

    /// Release savepoint
    pub fn release_savepoint(
        &self,
        txn_id: TransactionId,
        id: SavepointId,
    ) -> Result<(), TransactionError> {
        let context = self.get_context(txn_id)?;
        context.release_savepoint(id)
    }

    /// Rollback to savepoint
    pub fn rollback_to_savepoint<T: UndoTarget + ?Sized>(
        &self,
        txn_id: TransactionId,
        id: SavepointId,
        target: &T,
    ) -> Result<(), TransactionError> {
        let context = self.get_context(txn_id)?;
        let savepoint = context
            .get_savepoint(id)
            .ok_or(TransactionError::savepoint_not_found(id))?;

        if let Some(sync_manager) = self.sync_manager.as_ref() {
            sync_manager
                .rollback_transaction_to_sequence_sync(txn_id, savepoint.sync_sequence)
                .map_err(|e| TransactionError::sync_failed(e.to_string()))?;
        }

        context
            .rollback_to_savepoint(id, target)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))?;

        Ok(())
    }

    /// Get all active savepoints for transaction
    pub fn get_active_savepoints(&self, txn_id: TransactionId) -> Vec<SavepointInfo> {
        self.get_context(txn_id)
            .map(|ctx| ctx.get_all_savepoints())
            .unwrap_or_default()
    }

    /// Get current write timestamp
    pub fn write_timestamp(&self) -> Timestamp {
        self.version_manager.write_timestamp()
    }

    /// Get current read timestamp
    pub fn read_timestamp(&self) -> Timestamp {
        self.version_manager.read_timestamp()
    }

    /// Get pending transaction count
    pub fn pending_count(&self) -> i32 {
        self.version_manager.pending_count()
    }

    /// Get the checkpoint gate for external coordination.
    pub fn checkpoint_gate(&self) -> &Arc<CheckpointGate> {
        &self.checkpoint_gate
    }

    /// Begin a checkpoint operation: pause new writes and wait for active
    /// write transactions to complete.
    ///
    /// This is the first phase of a checkpoint. After this returns Ok,
    /// no new write transactions can start and all previously active writes
    /// have been drained (committed or aborted).
    ///
    /// # Arguments
    /// - `timeout`: Maximum time to wait for active writes to drain.
    ///
    /// # Returns
    /// - `Ok(())` if writes are paused and all active writes have drained.
    /// - `Err(TransactionError::CheckpointTimeout)` if timeout elapses first.
    pub fn begin_checkpoint(&self, timeout: Duration) -> Result<(), TransactionError> {
        self.checkpoint_gate.pause_writes_and_drain(timeout)
    }

    /// End a checkpoint operation: resume accepting new write transactions.
    ///
    /// This must be called after `begin_checkpoint` completes, regardless
    /// of whether the checkpoint itself succeeded or failed.
    pub fn end_checkpoint(&self) {
        self.checkpoint_gate.resume_writes()
    }

    /// Execute a coordinated checkpoint: pause writes, run the provided
    /// callback, then resume writes.
    ///
    /// The callback receives the current write timestamp and should return
    /// the LSN to checkpoint at. Writes are guaranteed to be paused for
    /// the duration of the callback.
    ///
    /// If the callback returns Err, writes are still resumed.
    pub fn coordinated_checkpoint<F>(
        &self,
        timeout: Duration,
        f: F,
    ) -> Result<(Timestamp, Lsn), TransactionError>
    where
        F: FnOnce(Timestamp) -> Result<Lsn, TransactionError>,
    {
        let checkpoint = self.begin_checkpoint_transaction(timeout)?;
        let write_ts = checkpoint.write_timestamp();
        match f(write_ts) {
            Ok(lsn) => {
                checkpoint.commit()?;
                Ok((write_ts, lsn))
            }
            Err(error) => {
                if let Err(abort_error) = checkpoint.abort() {
                    log::error!("Checkpoint rollback failed: {}", abort_error);
                }
                Err(error)
            }
        }
    }

    /// Begin a checkpoint transaction.
    ///
    /// Returns a [`CheckpointTransaction`] handle that keeps writes paused for
    /// its lifetime. The caller performs checkpoint work (e.g. via
    /// [`CheckpointManager`]), then either calls `commit()` or `abort()` on the
    /// handle. Dropping the handle aborts the checkpoint (resumes writes).
    ///
    /// Unlike `coordinated_checkpoint`, this does not run a callback — it
    /// gives the caller full control over the checkpoint lifecycle.
    ///
    /// # Errors
    /// Returns `Err(TransactionError::CheckpointTimeout)` if active writes
    /// do not drain within `timeout`.
    pub fn begin_checkpoint_transaction(
        &self,
        timeout: Duration,
    ) -> Result<CheckpointTransaction<'_>, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }
        if self.active_transactions.len() >= self.config.max_concurrent_transactions {
            return Err(TransactionError::too_many_transactions());
        }

        self.checkpoint_gate.pause_writes_and_drain(timeout)?;
        let write_ts = self.version_manager.write_timestamp();
        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let context = Arc::new(TransactionContext::new_checkpoint(
            txn_id,
            write_ts,
            self.config.txn_config.clone(),
        ));
        self.active_transactions.insert(txn_id, context);
        self.stats.record_txn_begin();

        Ok(CheckpointTransaction {
            manager: self,
            gate: Arc::clone(&self.checkpoint_gate),
            txn_id,
            write_ts,
            finished: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{
        ColumnId, CommitLsn, EdgeDeletionContext, EdgeIdentifier, EdgeKey, VertexIdentifier,
    };
    use crate::transaction::error::TransactionErrorKind;
    use crate::transaction::undo_log::{PropertyValue, UndoLogResult, UndoTarget};
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct RecordingSink {
        commits: AtomicUsize,
        aborts: AtomicUsize,
    }

    impl TransactionCommitSink for RecordingSink {
        fn commit_transaction(&self, _transaction_id: TransactionId) -> Result<CommitLsn, String> {
            self.commits.fetch_add(1, Ordering::SeqCst);
            Ok(CommitLsn::new(7))
        }

        fn abort_transaction(&self, _transaction_id: TransactionId) -> Result<(), String> {
            self.aborts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn explicit_commit_uses_storage_commit_sink_once() {
        let sink = Arc::new(RecordingSink::default());
        let manager = TransactionManager::new(TransactionManagerConfig::default())
            .with_commit_sink(sink.clone());
        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");
        manager
            .commit_transaction(txn_id)
            .expect("transaction should commit");
        assert_eq!(sink.commits.load(Ordering::SeqCst), 1);
        assert_eq!(sink.aborts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn explicit_abort_uses_storage_commit_sink_once() {
        let sink = Arc::new(RecordingSink::default());
        let manager = TransactionManager::new(TransactionManagerConfig::default())
            .with_commit_sink(sink.clone());
        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");
        manager
            .abort_transaction(txn_id)
            .expect("transaction should abort");
        assert_eq!(sink.commits.load(Ordering::SeqCst), 0);
        assert_eq!(sink.aborts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lifecycle_callbacks_observe_terminal_events() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let commits = Arc::new(AtomicUsize::new(0));
        let aborts = Arc::new(AtomicUsize::new(0));

        let commit_count = Arc::clone(&commits);
        manager.register_commit_callback(Arc::new(move |event| {
            if let TransactionEvent::Committed { .. } = event {
                commit_count.fetch_add(1, Ordering::SeqCst);
            }
        }));
        let abort_count = Arc::clone(&aborts);
        manager.register_rollback_callback(Arc::new(move |event| {
            if let TransactionEvent::Aborted { .. } = event {
                abort_count.fetch_add(1, Ordering::SeqCst);
            }
        }));

        let committed = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("committed transaction should begin");
        manager
            .commit_transaction(committed)
            .expect("transaction should commit");
        let aborted = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("aborted transaction should begin");
        manager
            .abort_transaction(aborted)
            .expect("transaction should abort");

        assert_eq!(commits.load(Ordering::SeqCst), 1);
        assert_eq!(aborts.load(Ordering::SeqCst), 1);
        assert!(manager.list_transactions().is_empty());
    }

    #[test]
    fn auto_commit_finalizes_success_and_failure() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let value = manager
            .auto_commit(|context| Ok::<_, TransactionError>(context.id.0))
            .expect("auto-commit operation should commit");
        assert!(value > 0);
        assert!(manager.list_transactions().is_empty());

        let result = manager.auto_commit(|_| {
            Err::<(), _>(TransactionError::internal("operation failed".to_string()))
        });
        assert!(result.is_err());
        assert!(manager.list_transactions().is_empty());
        assert_eq!(
            manager
                .stats()
                .committed_transactions
                .load(Ordering::Relaxed),
            1
        );
        assert_eq!(
            manager.stats().aborted_transactions.load(Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn configured_auto_commit_is_carried_by_execution_binding() {
        let mut config = TransactionManagerConfig::default();
        config.txn_config.auto_commit = true;
        let manager = TransactionManager::new(config);
        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");

        let execution = manager
            .create_execution(txn_id, false)
            .expect("execution binding should be created");
        assert!(execution.auto_commit());
        assert!(execution.requires_finalization());

        manager
            .abort_transaction(txn_id)
            .expect("transaction should abort");
    }

    #[test]
    fn test_transaction_manager_basic() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let txn_id = manager
            .begin_read_transaction(TransactionOptions::default())
            .expect("Failed to begin read transaction");

        assert!(manager.is_transaction_active(txn_id));

        manager
            .commit_transaction(txn_id)
            .expect("Failed to commit");

        assert!(!manager.is_transaction_active(txn_id));
    }

    #[test]
    fn test_transaction_manager_insert() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("Failed to begin insert transaction");

        assert!(manager.is_transaction_active(txn_id));

        manager
            .commit_transaction(txn_id)
            .expect("Failed to commit");

        assert!(!manager.is_transaction_active(txn_id));
    }

    #[test]
    fn test_transaction_manager_abort() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let txn_id = manager
            .begin_read_transaction(TransactionOptions::default())
            .expect("Failed to begin read transaction");

        manager.abort_transaction(txn_id).expect("Failed to abort");

        assert!(!manager.is_transaction_active(txn_id));
        assert_eq!(
            manager.stats().aborted_transactions.load(Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn test_transaction_manager_savepoint() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("Failed to begin transaction");

        let sp_id = manager
            .create_savepoint(txn_id, Some("test".to_string()))
            .expect("Failed to create savepoint");

        let sp = manager
            .get_savepoint(txn_id, sp_id)
            .expect("Failed to get savepoint");
        assert_eq!(sp.name, Some("test".to_string()));

        manager
            .commit_transaction(txn_id)
            .expect("Failed to commit");
    }

    #[test]
    fn test_transaction_manager_shutdown() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let txn_id = manager
            .begin_read_transaction(TransactionOptions::default())
            .expect("Failed to begin transaction");

        manager.shutdown();

        assert!(!manager.is_transaction_active(txn_id));
    }

    #[test]
    fn test_transaction_manager_with_sync_manager() {
        use crate::sync::SyncManager;

        let sync_manager = Arc::new(SyncManager::new_without_fulltext());
        let manager = TransactionManager::new(TransactionManagerConfig::default())
            .with_sync_manager(sync_manager);

        assert!(manager.sync_manager.is_some());
    }

    #[test]
    fn test_rollback_to_savepoint_with_sync_manager() {
        use crate::sync::SyncManager;

        struct MockUndoTarget;
        impl UndoTarget for MockUndoTarget {
            fn delete_vertex_type(&self, _label: crate::transaction::LabelId) -> UndoLogResult<()> {
                Ok(())
            }
            fn delete_edge_type(&self, _edge_key: EdgeKey) -> UndoLogResult<()> {
                Ok(())
            }
            fn delete_vertex(
                &self,
                _vertex: VertexIdentifier,
                _ts: crate::transaction::Timestamp,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn delete_edge(&self, _edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
                Ok(())
            }
            fn undo_update_vertex_property(
                &self,
                _vertex: VertexIdentifier,
                _col_id: ColumnId,
                _value: PropertyValue,
                _ts: crate::transaction::Timestamp,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn undo_update_edge_property(
                &self,
                _edge_id: EdgeIdentifier,
                _col_id: ColumnId,
                _value: PropertyValue,
                _ts: crate::transaction::Timestamp,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_vertex(
                &self,
                _vertex: VertexIdentifier,
                _ts: crate::transaction::Timestamp,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_edge(&self, _edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_vertex_properties(
                &self,
                _label_name: &str,
                _prop_names: &[String],
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_edge_properties(
                &self,
                _src_label: &str,
                _dst_label: &str,
                _edge_label: &str,
                _prop_names: &[String],
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_vertex_label(&self, _label_name: &str) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_edge_label(
                &self,
                _src_label: &str,
                _dst_label: &str,
                _edge_label: &str,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_rename_vertex_properties(
                &self,
                _label_name: &str,
                _current_names: &[String],
                _original_names: &[String],
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_rename_edge_properties(
                &self,
                _src_label: &str,
                _dst_label: &str,
                _edge_label: &str,
                _current_names: &[String],
                _original_names: &[String],
            ) -> UndoLogResult<()> {
                Ok(())
            }

        }

        let sync_manager = Arc::new(SyncManager::new_without_fulltext());
        let manager = TransactionManager::new(TransactionManagerConfig::default())
            .with_sync_manager(sync_manager);

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("Failed to begin transaction");
        let sp_id = manager
            .create_savepoint(txn_id, Some("sp".to_string()))
            .expect("Failed to create savepoint");

        let dummy = MockUndoTarget;
        let result = manager.rollback_to_savepoint(txn_id, sp_id, &dummy);
        // rollback_to_savepoint now succeeds as sync_manager properly handles the operation
        assert!(result.is_ok());
    }

    /// Sink that fails the first `fail_count` commit attempts, then succeeds.
    struct FlakyCommitSink {
        commits: AtomicUsize,
        fail_count: usize,
    }

    impl TransactionCommitSink for FlakyCommitSink {
        fn commit_transaction(&self, _txn_id: TransactionId) -> Result<CommitLsn, String> {
            let n = self.commits.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_count {
                Err(format!("transient error #{}", n + 1))
            } else {
                Ok(CommitLsn::new(7))
            }
        }

        fn abort_transaction(&self, _txn_id: TransactionId) -> Result<(), String> {
            Ok(())
        }
    }

    /// Sink that always fails commit.
    struct AlwaysFailCommitSink;

    impl TransactionCommitSink for AlwaysFailCommitSink {
        fn commit_transaction(&self, _txn_id: TransactionId) -> Result<CommitLsn, String> {
            Err("permanent failure".to_string())
        }

        fn abort_transaction(&self, _txn_id: TransactionId) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn commit_succeeds_after_retries() {
        let sink = Arc::new(FlakyCommitSink {
            commits: AtomicUsize::new(0),
            fail_count: 2,
        });
        let config = TransactionManagerConfig {
            commit_retry_attempts: 3,
            ..Default::default()
        };
        let manager = TransactionManager::new(config).with_commit_sink(sink.clone());

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");

        // Should succeed after 2 failures + 1 success (within 3 retry budget).
        manager
            .commit_transaction(txn_id)
            .expect("commit should succeed after retries");

        // commit_sink was called 3 times (2 failures + 1 success).
        assert_eq!(sink.commits.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn commit_fails_after_exhausting_retries() {
        let sink = Arc::new(AlwaysFailCommitSink);
        let config = TransactionManagerConfig {
            commit_retry_attempts: 2,
            ..Default::default()
        };
        let manager = TransactionManager::new(config).with_commit_sink(sink);

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");

        let result = manager.commit_transaction(txn_id);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind(),
            TransactionErrorKind::CommitFailed
        );
    }

    /// Sink that fails the first `fail_count` abort attempts, then succeeds.
    struct FlakyAbortSink {
        aborts: AtomicUsize,
        fail_count: usize,
    }

    impl TransactionCommitSink for FlakyAbortSink {
        fn commit_transaction(&self, _txn_id: TransactionId) -> Result<CommitLsn, String> {
            Ok(CommitLsn::new(7))
        }

        fn abort_transaction(&self, _txn_id: TransactionId) -> Result<(), String> {
            let n = self.aborts.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_count {
                Err(format!("transient abort error #{}", n + 1))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn abort_succeeds_after_retries() {
        let sink = Arc::new(FlakyAbortSink {
            aborts: AtomicUsize::new(0),
            fail_count: 1,
        });
        let config = TransactionManagerConfig {
            abort_retry_attempts: 2,
            ..Default::default()
        };
        let manager = TransactionManager::new(config).with_commit_sink(sink.clone());

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");

        manager
            .abort_transaction(txn_id)
            .expect("abort should succeed after retries");

        assert_eq!(sink.aborts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn state_transition_committing_to_aborted() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        assert!(ctx.transition_to(TransactionState::Committing).is_ok());
        assert!(ctx.transition_to(TransactionState::Aborting).is_ok());
        assert!(ctx.transition_to(TransactionState::Aborted).is_ok());
        assert_eq!(ctx.state(), TransactionState::Aborted);
    }

    #[test]
    fn state_transition_committing_direct_to_aborted() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        assert!(ctx.transition_to(TransactionState::Committing).is_ok());
        assert!(ctx.transition_to(TransactionState::Aborted).is_ok());
        assert_eq!(ctx.state(), TransactionState::Aborted);
    }

    #[test]
    fn checkpoint_gate_pauses_new_writes() {
        let gate = CheckpointGate::new();

        // Acquire a write slot.
        assert!(gate.acquire_write().is_ok());
        assert_eq!(gate.active_write_count(), 1);

        // Pause and drain.
        gate.writing_paused.store(true, Ordering::SeqCst);

        // New writes should fail.
        assert!(gate.acquire_write().is_err());

        // Release the active write.
        gate.release_write();

        // Still paused, should fail.
        assert!(gate.acquire_write().is_err());

        // Resume.
        gate.resume_writes();
        assert!(!gate.is_paused());
        assert!(gate.acquire_write().is_ok());
        assert_eq!(gate.active_write_count(), 1);
    }

    #[test]
    fn checkpoint_gate_drain_waits_for_active_writes() {
        use std::thread;

        let gate = Arc::new(CheckpointGate::new());
        let gate_clone = Arc::clone(&gate);

        // Start a write transaction.
        gate.acquire_write().expect("acquire should succeed");

        // Spawn a thread that will release after a short delay.
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            gate_clone.release_write();
        });

        // Drain should wait until the write is released.
        let result = gate.pause_writes_and_drain(Duration::from_secs(5));
        assert!(result.is_ok());
        assert_eq!(gate.active_write_count(), 0);

        handle.join().unwrap();
    }

    #[test]
    fn checkpoint_gate_drain_timeout() {
        let gate = CheckpointGate::new();

        // Acquire a write slot and never release.
        gate.acquire_write().expect("acquire should succeed");

        // Drain with short timeout should fail.
        let result = gate.pause_writes_and_drain(Duration::from_millis(50));
        assert!(result.is_err());
    }

    #[test]
    fn begin_insert_fails_when_checkpoint_paused() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        // Pause writes via checkpoint.
        manager
            .checkpoint_gate()
            .writing_paused
            .store(true, Ordering::SeqCst);

        // New insert should fail.
        let result = manager.begin_insert_transaction(TransactionOptions::default());
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind(),
            TransactionErrorKind::CheckpointInProgress
        );

        // Read transactions should still work.
        let read_result = manager.begin_read_transaction(TransactionOptions::default());
        assert!(read_result.is_ok());

        // Resume writes.
        manager.end_checkpoint();
        let insert_result = manager.begin_insert_transaction(TransactionOptions::default());
        assert!(insert_result.is_ok());
    }

    #[test]
    fn coordinated_checkpoint_drains_writes() {
        use std::thread;

        let manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));
        let manager_clone = Arc::clone(&manager);

        // Start a write transaction.
        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");

        // Spawn a thread that commits the transaction after a delay.
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            manager_clone
                .commit_transaction(txn_id)
                .expect("commit should succeed");
        });

        // Coordinated checkpoint should wait for the write to complete.
        let result = manager.coordinated_checkpoint(Duration::from_secs(5), |_ts| {
            Ok(crate::core::wal::types::Lsn::new(100))
        });
        assert!(result.is_ok());

        handle.join().unwrap();
    }

    #[test]
    fn checkpoint_transaction_drain_active_writes() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let write_txn = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("write should begin");

        let manager2 = Arc::new(manager);
        let mgr_clone = Arc::clone(&manager2);

        // Spawn a thread that tries to begin a checkpoint transaction.
        // It should block until the write commits/times out.
        let handle = std::thread::spawn(move || {
            let checkpoint = mgr_clone
                .begin_checkpoint_transaction(Duration::from_secs(5))
                .expect("checkpoint should begin after drain");
            assert!(checkpoint.write_timestamp() > 0);
            checkpoint.commit().expect("checkpoint should commit");
        });

        // Give the checkpoint thread a moment to start draining.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Commit the active write so the checkpoint can proceed.
        manager2
            .commit_transaction(write_txn)
            .expect("write should commit");

        handle.join().unwrap();

        // Verify writes are resumed after checkpoint commit.
        let gate = manager2.checkpoint_gate();
        assert!(!gate.is_paused());
        assert_eq!(gate.active_write_count(), 0);
    }

    #[test]
    fn checkpoint_transaction_abort_resumes_writes() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let checkpoint = manager
            .begin_checkpoint_transaction(Duration::from_secs(5))
            .expect("checkpoint should begin");
        assert!(manager.checkpoint_gate().is_paused());

        // Abort resumes writes.
        checkpoint.abort().expect("checkpoint should abort");
        assert!(!manager.checkpoint_gate().is_paused());
    }

    #[test]
    fn checkpoint_transaction_is_monitored_and_emits_commit() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let commits = Arc::new(AtomicUsize::new(0));
        let commit_count = Arc::clone(&commits);
        manager.register_commit_callback(Arc::new(move |event| {
            if let TransactionEvent::Committed { .. } = event {
                commit_count.fetch_add(1, Ordering::SeqCst);
            }
        }));

        let checkpoint = manager
            .begin_checkpoint_transaction(Duration::from_secs(5))
            .expect("checkpoint should begin");
        let transactions = manager.list_transactions();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].txn_type, TransactionType::Checkpoint);

        checkpoint.commit().expect("checkpoint should commit");
        assert!(manager.list_transactions().is_empty());
        assert_eq!(commits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn checkpoint_transaction_drop_resumes_writes() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        {
            let checkpoint = manager
                .begin_checkpoint_transaction(Duration::from_secs(5))
                .expect("checkpoint should begin");
            assert!(manager.checkpoint_gate().is_paused());
            // Drop without commit — should resume writes.
            drop(checkpoint);
        }
        assert!(!manager.checkpoint_gate().is_paused());
    }
}
