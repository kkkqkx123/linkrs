//! Transaction Context
//!
//! Manages the state and resources of a single transaction.

use std::fmt;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

use crossbeam_utils::atomic::AtomicCell;
use parking_lot::{Mutex, RwLock};

use super::error::TransactionError;
use super::mutation_journal::MutationJournal;
use super::participant::TransactionMutationRecorder;
use super::types::*;
use super::undo_log::{UndoLogEntry, UndoLogManager};
use super::wal::buffer::LocalWalBuffer;
use super::wal::Timestamp;
use linkrs_core::types::VertexId;

/// Transaction Context
///
/// Manages the state and resources of a single transaction.
/// Uses MVCC timestamps for snapshot isolation.
pub struct TransactionContext {
    /// Transaction ID
    pub id: TransactionId,
    /// Logical category used by lifecycle and monitoring code.
    pub txn_type: TransactionType,
    /// Current state
    state: AtomicCell<TransactionState>,
    /// Start timestamp (MVCC)
    pub start_timestamp: Timestamp,
    /// Commit timestamp allocated at commit time (0 = not yet committed).
    ///
    /// Read visibility is ordered by this timestamp, never by
    /// `start_timestamp`: see `VersionManager::allocate_commit_timestamp`.
    commit_timestamp: AtomicU64,
    /// Snapshot timestamp for ReadCommitted statement refresh (None = use start_timestamp)
    refreshed_read_ts: RwLock<Option<Timestamp>>,
    /// Statement snapshot currently pinned in the global snapshot tracker.
    ///
    /// `begin_statement` / `refresh_statement_snapshot` pin the refreshed
    /// read timestamp here so GC cannot reclaim versions the running
    /// statement may still read. Released by `finish_statement`, commit
    /// and abort (see `take_statement_snapshot_pin`); replacing an
    /// existing pin releases the old one first.
    statement_snapshot_pin: RwLock<Option<Timestamp>>,
    /// Start time (for timeout tracking)
    pub start_time: Instant,
    /// Timeout duration
    timeout: Duration,
    /// Whether read-only
    pub read_only: bool,
    /// Whether query execution owns statement-level finalization.
    pub auto_commit: bool,
    /// Isolation level
    pub isolation_level: IsolationLevel,
    /// Query timeout duration
    pub query_timeout: Option<Duration>,
    /// Statement timeout duration
    pub statement_timeout: Option<Duration>,
    /// Idle timeout duration
    pub idle_timeout: Option<Duration>,
    /// Last activity timestamp
    last_activity: AtomicCell<Instant>,
    /// Start timestamp of the currently executing statement.
    statement_start: AtomicCell<Instant>,
    /// Query count
    query_count: AtomicU64,
    /// Durability level
    pub durability: DurabilityLevel,
    /// Modified tables
    modified_tables: Mutex<Vec<String>>,
    /// Savepoint manager
    savepoint_manager: RwLock<SavepointManager>,
    /// Undo log manager for rollback
    undo_logs: RwLock<UndoLogManager>,
    /// Write set for conflict detection
    write_set: Mutex<WriteSet>,
    /// Read set for Serializable certification.
    read_set: Mutex<WriteSet>,
    /// Materialized WAL cache derived from the mutation journal.
    ///
    /// The journal is the single source of truth; this buffer only holds a
    /// commit-time materialization (see `materialize_wal_buffer`) so the
    /// flush path can hand entries to the global WAL writer. It is never
    /// written in parallel with the journal.
    local_wal: Mutex<LocalWalBuffer>,
    /// Whether this transaction has passed write set conflict validation
    write_validated: AtomicCell<bool>,
    /// Whether a failed statement requires the transaction to be aborted.
    rollback_only: AtomicCell<bool>,
    /// Whether manager-owned resources have already been released.
    resources_released: AtomicCell<bool>,
    /// Estimated bytes staged by this transaction.
    staged_bytes: AtomicU64,
    /// Durable commit metadata retained while post-commit cleanup is retried.
    commit_published: AtomicCell<bool>,
    commit_lsn: AtomicU64,
    /// Session or API owner of this transaction.
    owner: RwLock<Option<String>>,
    /// Maximum mutations allowed (0 = unlimited).
    max_mutation_count: u64,
    /// Maximum undo bytes allowed (0 = unlimited).
    max_undo_bytes: u64,
    /// Current mutation count.
    mutation_count: AtomicU64,
    /// Current estimated undo bytes.
    undo_bytes: AtomicU64,
    /// Fraction of budget at which a warning is emitted (0.0–1.0).
    budget_warning_threshold: f64,
    /// Whether a budget warning has already been emitted for mutation count.
    mutation_warning_emitted: AtomicCell<bool>,
    /// Whether a budget warning has already been emitted for undo bytes.
    undo_warning_emitted: AtomicCell<bool>,
    /// Pending budget warnings queued for fan-out as `TransactionEvent::BudgetWarning`.
    budget_warnings: Mutex<Vec<TransactionEvent>>,
    /// Whether this transaction holds the pessimistic write exclusion lock.
    pessimistic_lock_held: AtomicCell<bool>,
    /// Concurrency mode used by this transaction.
    concurrency_mode: ConcurrencyMode,
    /// Schema catalog version — incremented on every DDL operation.
    /// Used by the query layer to invalidate stale plan caches.
    schema_catalog_version: AtomicU64,
    /// Serializable full-scan read-set threshold for this transaction.
    serializable_full_scan_threshold: Option<usize>,
    /// SSI (Serializable Snapshot Isolation) state for rw-dependency tracking.
    ssi_state: RwLock<super::types::SsiState>,
    /// Canonical mutation journal. Sequence is assigned here and every
    /// other log derives from the same logical entry.
    mutation_journal: RwLock<MutationJournal>,
}

impl fmt::Debug for TransactionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransactionContext")
            .field("id", &self.id)
            .field("txn_type", &self.txn_type)
            .field("state", &self.state.load())
            .field("start_timestamp", &self.start_timestamp)
            .field("refreshed_read_ts", &self.effective_read_timestamp())
            .field("read_only", &self.read_only)
            .field("auto_commit", &self.auto_commit)
            .field("isolation_level", &self.isolation_level)
            .field("durability", &self.durability)
            .finish()
    }
}

mod budget;
mod certification;
mod journal;
mod lifecycle;
mod savepoint;
mod snapshot;

use savepoint::SavepointManager;

impl TransactionContext {
    /// Create a new transaction context
    pub fn new(id: TransactionId, start_timestamp: Timestamp, config: TransactionConfig) -> Self {
        let now = Instant::now();
        Self {
            id,
            txn_type: TransactionType::Write,
            state: AtomicCell::new(TransactionState::Active),
            start_timestamp,
            commit_timestamp: AtomicU64::new(0),
            refreshed_read_ts: RwLock::new(None),
            statement_snapshot_pin: RwLock::new(None),
            start_time: now,
            timeout: config.timeout,
            read_only: false,
            auto_commit: config.auto_commit,
            isolation_level: config.isolation_level,
            query_timeout: config.query_timeout,
            statement_timeout: config.statement_timeout,
            idle_timeout: config.idle_timeout,
            last_activity: AtomicCell::new(now),
            statement_start: AtomicCell::new(now),
            query_count: AtomicU64::new(0),
            durability: config.durability,
            modified_tables: Mutex::new(Vec::new()),
            savepoint_manager: RwLock::new(SavepointManager::new()),
            undo_logs: RwLock::new(UndoLogManager::new()),
            write_set: Mutex::new(WriteSet::new()),
            read_set: Mutex::new(WriteSet::new()),
            local_wal: Mutex::new(LocalWalBuffer::new()),
            write_validated: AtomicCell::new(false),
            rollback_only: AtomicCell::new(false),
            resources_released: AtomicCell::new(false),
            staged_bytes: AtomicU64::new(0),
            commit_published: AtomicCell::new(false),
            commit_lsn: AtomicU64::new(0),
            owner: RwLock::new(None),
            max_mutation_count: config.max_mutation_count,
            max_undo_bytes: config.max_undo_bytes,
            mutation_count: AtomicU64::new(0),
            undo_bytes: AtomicU64::new(0),
            budget_warning_threshold: config.budget_warning_threshold,
            mutation_warning_emitted: AtomicCell::new(false),
            undo_warning_emitted: AtomicCell::new(false),
            budget_warnings: Mutex::new(Vec::new()),
            pessimistic_lock_held: AtomicCell::new(false),
            concurrency_mode: config.concurrency_mode,
            schema_catalog_version: AtomicU64::new(0),
            serializable_full_scan_threshold: config.serializable_full_scan_threshold,
            ssi_state: RwLock::new(super::types::SsiState::new()),
            mutation_journal: RwLock::new(MutationJournal::new()),
        }
    }

    /// Create a new read-only transaction context
    pub fn new_readonly(
        id: TransactionId,
        start_timestamp: Timestamp,
        config: TransactionConfig,
    ) -> Self {
        let now = Instant::now();
        Self {
            id,
            txn_type: TransactionType::ReadOnly,
            state: AtomicCell::new(TransactionState::Active),
            start_timestamp,
            commit_timestamp: AtomicU64::new(0),
            refreshed_read_ts: RwLock::new(None),
            statement_snapshot_pin: RwLock::new(None),
            start_time: now,
            timeout: config.timeout,
            read_only: true,
            auto_commit: config.auto_commit,
            isolation_level: config.isolation_level,
            query_timeout: config.query_timeout,
            statement_timeout: config.statement_timeout,
            idle_timeout: config.idle_timeout,
            last_activity: AtomicCell::new(now),
            statement_start: AtomicCell::new(now),
            query_count: AtomicU64::new(0),
            durability: config.durability,
            modified_tables: Mutex::new(Vec::new()),
            savepoint_manager: RwLock::new(SavepointManager::new()),
            undo_logs: RwLock::new(UndoLogManager::new()),
            write_set: Mutex::new(WriteSet::new()),
            read_set: Mutex::new(WriteSet::new()),
            local_wal: Mutex::new(LocalWalBuffer::new()),
            write_validated: AtomicCell::new(false),
            rollback_only: AtomicCell::new(false),
            resources_released: AtomicCell::new(false),
            staged_bytes: AtomicU64::new(0),
            commit_published: AtomicCell::new(false),
            commit_lsn: AtomicU64::new(0),
            owner: RwLock::new(None),
            max_mutation_count: config.max_mutation_count,
            max_undo_bytes: config.max_undo_bytes,
            mutation_count: AtomicU64::new(0),
            undo_bytes: AtomicU64::new(0),
            budget_warning_threshold: config.budget_warning_threshold,
            mutation_warning_emitted: AtomicCell::new(false),
            undo_warning_emitted: AtomicCell::new(false),
            budget_warnings: Mutex::new(Vec::new()),
            pessimistic_lock_held: AtomicCell::new(false),
            concurrency_mode: config.concurrency_mode,
            schema_catalog_version: AtomicU64::new(0),
            serializable_full_scan_threshold: config.serializable_full_scan_threshold,
            ssi_state: RwLock::new(super::types::SsiState::new()),
            mutation_journal: RwLock::new(MutationJournal::new()),
        }
    }

    /// Create a checkpoint context without acquiring an MVCC read or write slot.
    pub fn new_checkpoint(
        id: TransactionId,
        write_timestamp: Timestamp,
        config: TransactionConfig,
    ) -> Self {
        let mut context = Self::new(id, write_timestamp, config);
        context.txn_type = TransactionType::Checkpoint;
        context
    }

    pub fn new_recovery(
        id: TransactionId,
        write_timestamp: Timestamp,
        config: TransactionConfig,
    ) -> Self {
        let mut ctx = Self::new(id, write_timestamp, config);
        ctx.txn_type = TransactionType::Recovery;
        ctx
    }

    pub fn new_dummy(
        id: TransactionId,
        write_timestamp: Timestamp,
        config: TransactionConfig,
    ) -> Self {
        let mut ctx = Self::new(id, write_timestamp, config);
        ctx.txn_type = TransactionType::Dummy;
        ctx
    }
}

impl TransactionMutationRecorder for TransactionContext {
    fn record_mutation(&self, mutation: MutationResult) -> Result<(), TransactionError> {
        self.record_mutation(mutation)
    }

    fn record_vertex_write(&self, vertex_id: VertexId) {
        self.record_vertex_write(vertex_id);
    }

    fn record_vertex_delete(&self, vertex_id: VertexId) {
        self.record_vertex_delete(vertex_id);
    }

    fn record_edge_write(&self, edge: linkrs_core::types::EdgeIdentifier) {
        self.record_edge_write(edge);
    }

    fn add_undo_log(&self, entry: UndoLogEntry) -> Result<(), TransactionError> {
        self.add_undo_log(entry)
    }

    fn record_table_modification(&self, table_name: &str) {
        self.record_table_modification(table_name);
    }

    fn record_schema_write(&self, resource: &str) -> Result<(), TransactionError> {
        self.record_schema_write(resource)
    }

    fn record_index_write(&self, resource: &str) {
        self.record_index_write(resource);
    }

    fn record_vertex_read(&self, vertex_id: VertexId) {
        self.record_vertex_read(vertex_id);
    }

    fn record_edge_read(&self, edge: linkrs_core::types::EdgeIdentifier) {
        self.record_edge_read(edge);
    }

    fn record_schema_read(&self, resource: &str) {
        self.record_schema_read(resource);
    }

    fn record_index_read(&self, resource: &str) {
        self.record_index_read(resource);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_warnings_drain_exactly_once() {
        let config = TransactionConfig::default().with_max_mutation_count(10);
        let ctx = TransactionContext::new(TransactionId(1), 1, config);
        // Threshold defaults to 0.8, so the 8th mutation queues a warning.
        for _ in 0..8 {
            ctx.record_mutation(MutationResult::default()).unwrap();
        }
        assert_eq!(ctx.pending_budget_warnings(), 1);
        let drained = ctx.drain_budget_warnings();
        assert_eq!(drained.len(), 1);
        assert!(matches!(
            &drained[0],
            TransactionEvent::BudgetWarning { resource, current: 8, limit: 10, .. }
            if resource == "mutation count"
        ));
        assert!(ctx.drain_budget_warnings().is_empty());
    }

    #[test]
    fn test_transaction_context_basic() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        assert_eq!(ctx.id, TransactionId(1));
        assert_eq!(ctx.timestamp(), 1);
        assert_eq!(ctx.state(), TransactionState::Active);
        assert!(!ctx.read_only);
    }

    #[test]
    fn test_transaction_context_readonly() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new_readonly(TransactionId(1), 1, config);

        assert!(ctx.read_only);
    }

    #[test]
    fn test_transaction_context_state_transition() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        assert!(ctx.transition_to(TransactionState::Committing).is_ok());
        assert_eq!(ctx.state(), TransactionState::Committing);
        assert!(ctx.transition_to(TransactionState::Aborting).is_ok());
        assert_eq!(ctx.state(), TransactionState::Aborting);
        assert!(ctx.transition_to(TransactionState::Aborted).is_ok());
        assert_eq!(ctx.state(), TransactionState::Aborted);
    }

    #[test]
    fn test_transaction_context_savepoint() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        let sp_id = ctx.create_savepoint(Some("test".to_string()), 0);
        assert!(ctx.get_savepoint(sp_id).is_some());

        let sp = ctx.get_savepoint(sp_id).unwrap();
        assert_eq!(sp.name, Some("test".to_string()));
    }
}
