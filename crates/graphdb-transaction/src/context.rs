//! Transaction Context
//!
//! Manages the state and resources of a single transaction.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_utils::atomic::AtomicCell;
use parking_lot::{Mutex, RwLock};

use super::error::TransactionError;
use super::mutation_journal::{MutationJournal, MutationResource, TransactionMutationRecord};
use super::participant::TransactionMutationRecorder;
use super::rollback::CombinedRollback;
use super::types::*;
use super::undo_log::{UndoLogEntry, UndoLogManager, UndoTarget};
use super::wal::buffer::LocalWalBuffer;
use super::wal::Timestamp;
use graphdb_core::types::CommitLsn;
use graphdb_core::types::VertexId;

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
    /// Snapshot timestamp for time-travel reads (None = use start_timestamp)
    snapshot_timestamp: RwLock<Option<Timestamp>>,
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
    /// Operation log (using RwLock to optimize read-heavy write-light scenarios)
    operation_logs: RwLock<Vec<OperationLog>>,
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
    /// Redo metadata retained for savepoint and certification boundaries.
    redo_entries: RwLock<Vec<crate::wal::TransactionWalEntry>>,
    /// Two-tier WAL: in-memory local buffer flushed at commit.
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
            .field("snapshot_timestamp", &self.effective_snapshot_timestamp())
            .field("read_only", &self.read_only)
            .field("auto_commit", &self.auto_commit)
            .field("isolation_level", &self.isolation_level)
            .field("durability", &self.durability)
            .finish()
    }
}

/// Savepoint Manager
pub(crate) struct SavepointManager {
    savepoints: HashMap<SavepointId, SavepointInfo>,
    next_id: SavepointId,
    next_sequence: u64,
}

impl SavepointManager {
    fn new() -> Self {
        Self {
            savepoints: HashMap::new(),
            next_id: 1,
            next_sequence: 1,
        }
    }

    fn create_savepoint(&mut self, params: SavepointParams) -> SavepointId {
        let id = self.next_id;
        self.next_id += 1;
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let info = SavepointInfo {
            id,
            name: params.name,
            created_at: Instant::now(),
            sequence,
            operation_log_index: params.operation_log_index,
            undo_log_index: params.undo_log_index,
            sync_sequence: params.sync_sequence,
            write_set: params.write_set,
            read_set: params.read_set,
            redo_log_index: params.redo_log_index,
            local_wal_entry_len: params.local_wal_entry_len,
            local_wal_intent_len: params.local_wal_intent_len,
            modified_tables: params.modified_tables,
            journal_len: params.journal_len,
            journal_next_sequence: params.journal_next_sequence,
        };
        self.savepoints.insert(id, info);
        id
    }

    fn get_savepoint(&self, id: SavepointId) -> Option<&SavepointInfo> {
        self.savepoints.get(&id)
    }

    fn remove_savepoint(&mut self, id: SavepointId) -> Option<SavepointInfo> {
        self.savepoints.remove(&id)
    }

    fn clear(&mut self) {
        self.savepoints.clear();
    }

    fn find_by_name(&self, name: &str) -> Option<SavepointInfo> {
        self.savepoints
            .values()
            .filter(|sp| sp.name.as_deref() == Some(name))
            .max_by_key(|sp| sp.sequence)
            .cloned()
    }
}

impl TransactionContext {
    /// Create a new transaction context
    pub fn new(id: TransactionId, start_timestamp: Timestamp, config: TransactionConfig) -> Self {
        let now = Instant::now();
        Self {
            id,
            txn_type: TransactionType::Write,
            state: AtomicCell::new(TransactionState::Active),
            start_timestamp,
            snapshot_timestamp: RwLock::new(None),
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
            operation_logs: RwLock::new(Vec::new()),
            modified_tables: Mutex::new(Vec::new()),
            savepoint_manager: RwLock::new(SavepointManager::new()),
            undo_logs: RwLock::new(UndoLogManager::new()),
            write_set: Mutex::new(WriteSet::new()),
            read_set: Mutex::new(WriteSet::new()),
            redo_entries: RwLock::new(Vec::new()),
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
            snapshot_timestamp: RwLock::new(None),
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
            operation_logs: RwLock::new(Vec::new()),
            modified_tables: Mutex::new(Vec::new()),
            savepoint_manager: RwLock::new(SavepointManager::new()),
            undo_logs: RwLock::new(UndoLogManager::new()),
            write_set: Mutex::new(WriteSet::new()),
            read_set: Mutex::new(WriteSet::new()),
            redo_entries: RwLock::new(Vec::new()),
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

    pub fn get_type(&self) -> TransactionType {
        self.txn_type
    }

    pub fn get_concurrency_mode(&self) -> ConcurrencyMode {
        self.concurrency_mode
    }

    /// Get current state
    pub fn state(&self) -> TransactionState {
        self.state.load()
    }

    /// Get the MVCC timestamp
    pub fn timestamp(&self) -> Timestamp {
        self.start_timestamp
    }

    /// Get the effective snapshot timestamp for reads
    pub fn effective_snapshot_timestamp(&self) -> Timestamp {
        self.snapshot_timestamp
            .read()
            .unwrap_or(self.start_timestamp)
    }

    /// Set the snapshot timestamp for time-travel reads
    pub fn set_snapshot_timestamp(&self, ts: Timestamp) {
        *self.snapshot_timestamp.write() = Some(ts);
    }

    /// Check if transaction has expired
    pub fn is_expired(&self) -> bool {
        self.start_time.elapsed() > self.timeout
    }

    /// Check if query timeout has been exceeded
    pub fn is_query_timeout(&self) -> bool {
        if let Some(query_timeout) = self.query_timeout {
            self.statement_start.load().elapsed() > query_timeout
        } else {
            false
        }
    }

    /// Check if statement timeout has been exceeded
    pub fn is_statement_timeout(&self, statement_start: Instant) -> bool {
        if let Some(statement_timeout) = self.statement_timeout {
            statement_start.elapsed() > statement_timeout
        } else {
            false
        }
    }

    /// Check if idle timeout has been exceeded
    pub fn is_idle_timeout(&self) -> bool {
        if let Some(idle_timeout) = self.idle_timeout {
            self.last_activity.load().elapsed() > idle_timeout
        } else {
            false
        }
    }

    /// Check if any timeout has been exceeded
    pub fn check_timeouts(&self) -> Result<(), TransactionError> {
        if self.is_expired() {
            return Err(TransactionError::transaction_timeout());
        }

        if self.is_idle_timeout() {
            return Err(TransactionError::transaction_timeout());
        }

        Ok(())
    }

    /// Update last activity timestamp
    pub fn update_activity(&self) {
        self.last_activity.store(Instant::now());
    }

    /// Begin one statement and return its monotonic start time.
    pub fn begin_statement(&self) -> Result<Instant, TransactionError> {
        self.can_execute()?;
        self.check_timeouts()?;
        let start = Instant::now();
        self.statement_start.store(start);
        self.increment_query_count();
        Ok(start)
    }

    /// Finish one statement and enforce query and statement timeouts.
    pub fn finish_statement(&self, statement_start: Instant) -> Result<(), TransactionError> {
        let query_timed_out = self
            .query_timeout
            .is_some_and(|timeout| statement_start.elapsed() > timeout);
        let statement_timed_out = self.is_statement_timeout(statement_start);
        self.statement_start.store(Instant::now());
        self.update_activity();
        if query_timed_out || statement_timed_out {
            self.mark_rollback_only();
            return Err(TransactionError::transaction_timeout());
        }
        Ok(())
    }

    pub fn mark_rollback_only(&self) {
        self.rollback_only.store(true);
    }

    pub fn is_rollback_only(&self) -> bool {
        self.rollback_only.load()
    }

    pub fn set_owner(&self, owner: impl Into<String>) {
        *self.owner.write() = Some(owner.into());
    }

    pub fn owner(&self) -> Option<String> {
        self.owner.read().clone()
    }

    pub fn owner_matches(&self, owner: Option<&str>) -> bool {
        match (self.owner.read().as_deref(), owner) {
            (None, _) => true,
            (Some(expected), Some(actual)) => expected == actual,
            (Some(_), None) => false,
        }
    }

    pub fn mark_resources_released(&self) -> bool {
        self.resources_released
            .compare_exchange(false, true)
            .is_ok()
    }

    pub fn resources_released(&self) -> bool {
        self.resources_released.load()
    }

    pub fn staged_bytes(&self) -> u64 {
        self.staged_bytes.load(Ordering::Relaxed)
    }

    pub fn mark_commit_published(&self, commit_lsn: CommitLsn) {
        self.commit_lsn.store(commit_lsn.get(), Ordering::Release);
        self.commit_published.store(true);
    }

    pub fn commit_published(&self) -> bool {
        self.commit_published.load()
    }

    pub fn commit_lsn(&self) -> CommitLsn {
        CommitLsn::new(self.commit_lsn.load(Ordering::Acquire))
    }

    pub fn add_staged_bytes(&self, bytes: u64) {
        self.staged_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Increment query count
    pub fn increment_query_count(&self) {
        self.query_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get query count
    pub fn query_count(&self) -> u64 {
        self.query_count.load(Ordering::Relaxed)
    }

    /// Get the current schema catalog version for this transaction.
    /// The version is incremented on every DDL operation and can be used by the
    /// query layer to detect schema changes and invalidate stale plan caches.
    pub fn schema_catalog_version(&self) -> u64 {
        self.schema_catalog_version.load(Ordering::Relaxed)
    }

    /// Increment the schema catalog version after a DDL operation.
    /// Returns the new version.
    pub fn bump_schema_catalog_version(&self) -> u64 {
        self.schema_catalog_version.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn set_pessimistic_lock(&self) {
        self.pessimistic_lock_held.store(true);
    }

    pub fn has_pessimistic_lock(&self) -> bool {
        self.pessimistic_lock_held.load()
    }

    pub fn clear_pessimistic_lock(&self) {
        self.pessimistic_lock_held.store(false);
    }

    pub fn serializable_full_scan_threshold(&self) -> Option<usize> {
        self.serializable_full_scan_threshold
    }

    /// Record a resource read for SSI rw-dependency tracking.
    pub fn record_ssi_read(&self, resource: super::types::ResourceId) {
        self.ssi_state.write().record_read(resource);
    }

    /// Record a resource write for SSI rw-dependency tracking.
    pub fn record_ssi_write(&self, resource: super::types::ResourceId) {
        self.ssi_state.write().record_write(resource);
    }

    /// Get the set of resources read by this transaction (for SSI).
    pub fn get_ssi_read_resources(&self) -> HashSet<super::types::ResourceId> {
        self.ssi_state.read().read_resources().clone()
    }

    /// Get the set of resources written by this transaction (for SSI).
    pub fn get_ssi_write_resources(&self) -> HashSet<super::types::ResourceId> {
        self.ssi_state.read().write_resources().clone()
    }

    /// Clear SSI read locks (called on commit/abort).
    pub fn clear_ssi_state(&self) {
        let mut state = self.ssi_state.write();
        *state = super::types::SsiState::new();
    }

    /// Get remaining time
    pub fn remaining_time(&self) -> Duration {
        let elapsed = self.start_time.elapsed();
        if elapsed >= self.timeout {
            Duration::from_secs(0)
        } else {
            self.timeout - elapsed
        }
    }

    /// State transition
    ///
    /// Valid transitions form a DAG:
    ///   Active → Committing | Aborting
    ///   Committing → Aborting | Aborted
    ///   Aborting → Aborted
    pub fn transition_to(&self, new_state: TransactionState) -> Result<(), TransactionError> {
        loop {
            let current = self.state.load();

            let valid_transition = matches!(
                (current, new_state),
                (
                    TransactionState::Active,
                    TransactionState::Committing | TransactionState::Aborting
                ) | (
                    TransactionState::Committing,
                    TransactionState::Aborting | TransactionState::Aborted
                ) | (TransactionState::Aborting, TransactionState::Aborted)
            );

            if !valid_transition {
                return Err(TransactionError::invalid_state_transition(
                    current, new_state,
                ));
            }

            if self.state.compare_exchange(current, new_state).is_ok() {
                return Ok(());
            }
        }
    }

    /// Get the write set for this transaction
    pub fn get_write_set(&self) -> WriteSet {
        self.write_set.lock().clone()
    }

    /// Check if write set is empty
    pub fn is_write_set_empty(&self) -> bool {
        self.write_set.lock().is_empty()
    }

    /// Get write set size (number of modified entities)
    pub fn write_set_size(&self) -> usize {
        self.write_set.lock().size()
    }

    /// Mark this transaction as having passed write set validation
    pub fn mark_write_validated(&self) {
        self.write_validated.store(true);
    }

    /// Check if this transaction has passed write set validation
    pub fn is_write_validated(&self) -> bool {
        self.write_validated.load()
    }

    /// Check if this transaction's write set conflicts with another
    pub fn has_write_conflict_with(&self, other: &TransactionContext) -> bool {
        let ws1 = self.write_set.lock();
        let ws2 = other.write_set.lock();
        ws1.has_conflict_with(&ws2)
    }

    /// Get the read set captured by this transaction.
    pub fn get_read_set(&self) -> WriteSet {
        self.read_set.lock().clone()
    }

    pub fn record_vertex_read(&self, vid: VertexId) {
        self.read_set.lock().record_vertex(vid);
    }

    pub fn record_edge_read(&self, edge: graphdb_core::types::EdgeIdentifier) {
        self.read_set.lock().record_edge(edge);
    }

    pub fn record_schema_read(&self, resource: &str) {
        self.read_set.lock().record_schema_resource(resource);
    }

    pub fn record_index_read(&self, resource: &str) {
        self.read_set.lock().record_index_resource(resource);
    }

    pub fn record_read_range(&self, range: ReadRange) {
        self.read_set.lock().record_read_range(range);
    }

    pub fn redo_log_len(&self) -> usize {
        self.redo_entries.read().len()
    }

    pub fn truncate_redo_log(&self, index: usize) {
        self.redo_entries.write().truncate(index);
    }

    /// Access the two-tier local WAL buffer for direct manipulation.
    pub fn local_wal_buffer(&self) -> parking_lot::MutexGuard<'_, LocalWalBuffer> {
        self.local_wal.lock()
    }

    /// Number of buffered local WAL bytes (for metrics / backpressure).
    pub fn local_wal_bytes(&self) -> usize {
        self.local_wal.lock().buffered_bytes()
    }

    /// Whether the local WAL buffer is empty.
    pub fn is_local_wal_empty(&self) -> bool {
        self.local_wal.lock().is_empty()
    }

    /// Append a redo entry to both the canonical redo log and the local WAL buffer.
    pub fn append_local_wal_entry(&self, entry: crate::wal::TransactionWalEntry) {
        self.redo_entries.write().push(entry.clone());
        let _ = self
            .local_wal
            .lock()
            .append_entry(entry.op_type, entry.timestamp, entry.payload);
    }

    /// Flush the local WAL buffer to a global writer (used by storage commit path).
    pub fn flush_local_wal(
        &self,
        writer: &mut crate::wal::LocalWalWriter,
    ) -> Result<CommitLsn, TransactionError> {
        let mut buf = self.local_wal.lock();
        buf.flush_to_writer(writer, self.id, self.durability)
            .map_err(|e| TransactionError::internal(e.to_string()))
    }

    /// Check if operation can be executed
    pub fn can_execute(&self) -> Result<(), TransactionError> {
        let state = self.state.load();

        if !state.can_execute() {
            return Err(TransactionError::invalid_state_for_execution(state));
        }

        if self.is_rollback_only() {
            return Err(TransactionError::invalid_state_for_execution(state));
        }

        if self.is_expired() {
            return Err(TransactionError::transaction_expired());
        }

        Ok(())
    }

    /// Get transaction info
    pub fn info(&self) -> TransactionInfo {
        let modified_tables = self.get_modified_tables();
        let savepoint_count = self.get_all_savepoints().len();
        TransactionInfo {
            id: self.id,
            state: self.state.load(),
            txn_type: self.txn_type,
            start_time: self.start_time,
            elapsed: self.start_time.elapsed(),
            is_read_only: self.read_only,
            isolation_level: self.isolation_level,
            query_count: self.query_count.load(Ordering::Relaxed),
            mutation_count: self.mutation_count.load(Ordering::Relaxed),
            modified_tables,
            savepoint_count,
            read_timestamp: self.effective_snapshot_timestamp(),
            write_timestamp: if self.read_only { 0 } else { self.timestamp() },
            owner: self.owner(),
            last_activity: self.last_activity.load().elapsed(),
            rollback_only: self.is_rollback_only(),
            blocking_reason: if self.is_rollback_only() {
                Some("transaction is marked rollback-only".to_string())
            } else {
                None
            },
            staged_bytes: self.staged_bytes(),
            undo_bytes: self.undo_log_len() as u64,
        }
    }

    /// Add operation log
    pub fn add_operation_log(&self, operation: OperationLog) {
        let mut logs = self.operation_logs.write();
        logs.push(operation);
    }

    /// Batch add operation logs
    pub fn add_operation_logs(&self, operations: Vec<OperationLog>) {
        let mut logs = self.operation_logs.write();
        logs.extend(operations);
    }

    /// Get operation logs
    pub fn get_operation_logs(&self) -> Vec<OperationLog> {
        let logs = self.operation_logs.read();
        logs.clone()
    }

    /// Get operation log length
    pub fn operation_log_len(&self) -> usize {
        let logs = self.operation_logs.read();
        logs.len()
    }

    /// Get operation log at specified index
    pub fn get_operation_log(&self, index: usize) -> Option<OperationLog> {
        let logs = self.operation_logs.read();
        logs.get(index).cloned()
    }

    /// Get operation logs in specified range
    pub fn get_operation_logs_range(&self, start: usize, end: usize) -> Vec<OperationLog> {
        let logs = self.operation_logs.read();
        if start >= logs.len() {
            return Vec::new();
        }
        let end = end.min(logs.len());
        logs[start..end].to_vec()
    }

    /// Truncate operation logs to specified index
    pub fn truncate_operation_log(&self, index: usize) {
        let mut logs = self.operation_logs.write();
        if index < logs.len() {
            logs.truncate(index);
        }
    }

    /// Record a vertex write in the write set
    pub fn record_vertex_write(&self, vid: VertexId) {
        self.write_set.lock().record_vertex(vid);
    }

    pub fn record_vertex_delete(&self, vid: VertexId) {
        self.write_set.lock().record_vertex_delete(vid);
    }

    /// Record an edge write for conflict certification.
    pub fn record_edge_write(&self, edge: graphdb_core::types::EdgeIdentifier) {
        self.write_set.lock().record_edge(edge);
    }

    pub fn record_schema_write(&self, resource: &str) -> Result<(), TransactionError> {
        self.write_set.lock().record_schema_resource(resource);
        Ok(())
    }

    pub fn record_index_write(&self, resource: &str) {
        self.write_set.lock().record_index_resource(resource);
    }

    /// Publish a complete mutation result in the canonical metadata order.
    /// The journal assigns the sequence and is the single source of truth;
    /// all derived logs are populated from that entry in a fixed order.
    pub fn record_mutation(&self, mutation: MutationResult) -> Result<(), TransactionError> {
        self.can_execute()?;
        let new_count = self.mutation_count.fetch_add(1, Ordering::Relaxed) + 1;
        if self.max_mutation_count > 0 && new_count > self.max_mutation_count {
            return Err(TransactionError::transaction_budget_exceeded(
                "mutation count",
                new_count,
                self.max_mutation_count,
            ));
        }

        if self.budget_warning_threshold > 0.0
            && self.max_mutation_count > 0
            && !self.mutation_warning_emitted.load()
            && new_count as f64 >= self.max_mutation_count as f64 * self.budget_warning_threshold
        {
            self.mutation_warning_emitted.store(true);
            log::warn!(
                "Transaction {} mutation count ({}) exceeds {:.0}% of limit ({})",
                self.id,
                new_count,
                self.budget_warning_threshold * 100.0,
                self.max_mutation_count,
            );
        }

        let undo_estimate = self.undo_bytes.fetch_add(64, Ordering::Relaxed) + 64;
        if self.max_undo_bytes > 0 && undo_estimate > self.max_undo_bytes {
            return Err(TransactionError::transaction_budget_exceeded(
                "undo bytes",
                undo_estimate,
                self.max_undo_bytes,
            ));
        }

        if self.budget_warning_threshold > 0.0
            && self.max_undo_bytes > 0
            && !self.undo_warning_emitted.load()
            && undo_estimate as f64 >= self.max_undo_bytes as f64 * self.budget_warning_threshold
        {
            self.undo_warning_emitted.store(true);
            log::warn!(
                "Transaction {} undo bytes ({}) exceeds {:.0}% of limit ({})",
                self.id,
                undo_estimate,
                self.budget_warning_threshold * 100.0,
                self.max_undo_bytes,
            );
        }

        let resource = if mutation.resource != MutationResource::Unknown {
            mutation.resource
        } else if mutation.modified_table.is_some() {
            MutationResource::from_modified_table(mutation.modified_table.as_deref())
        } else if let Some(ref redo) = mutation.redo_entry {
            MutationResource::from_wal_op(redo.op_type)
        } else if !mutation.index_intents.is_empty() {
            MutationResource::SyncIntent
        } else {
            MutationResource::Unknown
        };

        let entity_keys = mutation.entity_keys.clone();
        let redo_with_seq = mutation.redo_entry.map(|mut e| {
            let seq = self.mutation_journal.read().next_sequence();
            e.transaction_id = Some(self.id);
            e.mutation_sequence = Some(seq);
            e
        });
        let journal_len_before = self.mutation_journal.read().len() as u64;
        let record = TransactionMutationRecord {
            sequence: journal_len_before,
            transaction_id: self.id,
            entity_keys: entity_keys.clone(),
            resource,
            undo: mutation.undo_entry.clone(),
            redo: redo_with_seq.clone(),
            index_intents: mutation.index_intents.clone(),
            modified_table: mutation.modified_table.clone(),
            write_timestamp: self.start_timestamp,
            commit_timestamp: None,
        };
        {
            let mut journal = self.mutation_journal.write();
            journal.push(record);
            if cfg!(debug_assertions) {
                if let Err(e) = journal.check_invariants() {
                    log::error!("journal invariant violated: {}", e);
                }
            }
        }

        let sequence = self
            .mutation_journal
            .read()
            .records()
            .last()
            .map(|r| r.sequence);
        let operation_log = OperationLog::Mutation {
            entities: entity_keys
                .iter()
                .map(|entity| format!("{entity:?}").into_bytes())
                .collect(),
            table: mutation.modified_table.clone(),
            sequence,
        };
        self.add_operation_log(operation_log);

        for entity in entity_keys {
            match entity {
                MutationEntityKey::Vertex(vertex_id) => self.record_vertex_write(vertex_id),
                MutationEntityKey::Edge(edge) => self.record_edge_write(edge),
            }
        }
        if resource == MutationResource::Schema {
            if let Some(ref table) = mutation.modified_table {
                self.write_set.lock().record_schema_resource(table);
            } else {
                self.write_set.lock().record_schema_resource("schema");
            }
        }
        if resource == MutationResource::Index || resource == MutationResource::SyncIntent {
            for intent in &mutation.index_intents {
                self.record_index_write(&format!("{}", intent.mutation.ordering_key));
            }
        }
        if let Some(entry) = mutation.undo_entry {
            self.add_undo_log(entry)?;
        }
        if let Some(entry) = redo_with_seq {
            let cloned = entry.clone();
            self.redo_entries.write().push(entry);
            let _ = self.local_wal.lock().append_full_entry(cloned);
        }
        {
            let mut local = self.local_wal.lock();
            for intent in mutation.index_intents {
                let _ = local.append_intent(intent);
            }
        }
        if let Some(table) = mutation.modified_table {
            self.record_table_modification(&table);
        }
        self.write_validated.store(false);
        Ok(())
    }

    pub fn mutation_journal_len(&self) -> usize {
        self.mutation_journal.read().len()
    }

    pub fn next_mutation_sequence(&self) -> u64 {
        self.mutation_journal.read().next_sequence()
    }

    pub fn check_journal_invariants(&self) -> Result<(), String> {
        self.mutation_journal.read().check_invariants()
    }

    /// Publish the commit timestamp into the journal so every mutation that
    /// carried a write timestamp now also carries its commit timestamp.
    /// Must be called after the commit frontier has advanced and before
    /// storage finalization so pending vs committed history can be distinguished.
    pub fn publish_commit_timestamp(&self, commit_ts: Timestamp) {
        self.mutation_journal
            .write()
            .publish_commit_timestamp(commit_ts);
    }

    pub fn build_commit_descriptor(&self) -> crate::participant::TransactionCommitDescriptor {
        let ws = self.get_write_set();
        let rs = self.get_read_set();
        let journal = self.mutation_journal.read();
        let entry_count = journal.total_redo_entries();
        let intent_count = journal.total_intents();
        let first_sequence = journal.records().first().map(|r| r.sequence).unwrap_or(0);
        let range = 0..journal.len();
        drop(journal);
        let mut desc = crate::participant::TransactionCommitDescriptor::new(
            self.id,
            self.timestamp(),
            self.durability,
            ws,
        );
        desc.read_set = rs;
        desc.first_sequence = first_sequence;
        desc.entry_count = entry_count;
        desc.intent_count = intent_count;
        desc.journal_range = range;
        desc
    }

    /// Replace the write set after a savepoint rollback.
    pub fn restore_write_set(&self, write_set: WriteSet) {
        *self.write_set.lock() = write_set;
        self.write_validated.store(false);
    }

    pub fn restore_read_set(&self, read_set: WriteSet) {
        *self.read_set.lock() = read_set;
    }

    /// Clear certification state after a partial rollback.
    pub fn clear_write_validation(&self) {
        self.write_validated.store(false);
    }

    /// Clear operation logs
    pub fn clear_operation_log(&self) {
        let mut logs = self.operation_logs.write();
        logs.clear();
    }

    /// Record table modification
    pub fn record_table_modification(&self, table_name: &str) {
        let mut tables = self.modified_tables.lock();
        if !tables.contains(&table_name.to_string()) {
            tables.push(table_name.to_string());
        }
    }

    /// Get modified tables
    pub fn get_modified_tables(&self) -> Vec<String> {
        let tables = self.modified_tables.lock();
        tables.clone()
    }

    /// Create savepoint
    pub fn create_savepoint(&self, name: Option<String>, sync_sequence: u64) -> SavepointId {
        let (local_entry_len, local_intent_len) = {
            let buf = self.local_wal.lock();
            (buf.entry_count(), buf.intent_count())
        };
        let (journal_len, journal_next) = {
            let j = self.mutation_journal.read();
            (j.len(), j.next_sequence())
        };
        let params = SavepointParams {
            name,
            operation_log_index: self.operation_log_len(),
            undo_log_index: self.undo_log_len(),
            sync_sequence,
            write_set: self.get_write_set(),
            read_set: self.get_read_set(),
            redo_log_index: self.redo_log_len(),
            local_wal_entry_len: local_entry_len,
            local_wal_intent_len: local_intent_len,
            modified_tables: self.get_modified_tables(),
            journal_len,
            journal_next_sequence: journal_next,
        };
        let mut manager = self.savepoint_manager.write();
        manager.create_savepoint(params)
    }

    /// Get savepoint info
    pub fn get_savepoint(&self, id: SavepointId) -> Option<SavepointInfo> {
        let manager = self.savepoint_manager.read();
        manager.get_savepoint(id).cloned()
    }

    /// Find savepoint by ID (alias for get_savepoint for API clarity)
    pub fn find_savepoint_by_id(&self, id: SavepointId) -> Option<SavepointInfo> {
        self.get_savepoint(id)
    }

    /// Get all savepoints
    pub fn get_all_savepoints(&self) -> Vec<SavepointInfo> {
        let manager = self.savepoint_manager.read();
        manager.savepoints.values().cloned().collect()
    }

    /// Find savepoint by name
    pub fn find_savepoint_by_name(&self, name: &str) -> Option<SavepointInfo> {
        let manager = self.savepoint_manager.read();
        manager.find_by_name(name)
    }

    /// Release savepoint
    pub fn release_savepoint(&self, id: SavepointId) -> Result<(), TransactionError> {
        let mut manager = self.savepoint_manager.write();
        manager
            .remove_savepoint(id)
            .map(|_| ())
            .ok_or(TransactionError::savepoint_not_found(id))
    }

    /// Rollback to savepoint
    pub fn rollback_to_savepoint<T: UndoTarget + ?Sized>(
        &self,
        id: SavepointId,
        target: &T,
    ) -> Result<(), TransactionError> {
        let state = self.state.load();
        if !state.can_execute() {
            return Err(TransactionError::invalid_state_for_abort(state));
        }

        if self.is_expired() {
            return Err(TransactionError::transaction_expired());
        }

        let savepoint_info = {
            let manager = self.savepoint_manager.read();
            manager
                .get_savepoint(id)
                .cloned()
                .ok_or(TransactionError::savepoint_not_found(id))?
        };

        // Validate journal position is the authoritative savepoint boundary.
        {
            let journal = self.mutation_journal.read();
            let position = crate::mutation_journal::MutationJournalPosition {
                journal_len: savepoint_info.journal_len,
                next_sequence: savepoint_info.journal_next_sequence,
                operation_log_index: savepoint_info.operation_log_index,
                undo_log_index: savepoint_info.undo_log_index,
                redo_log_index: savepoint_info.redo_log_index,
                local_wal_entry_len: savepoint_info.local_wal_entry_len,
                local_wal_intent_len: savepoint_info.local_wal_intent_len,
                modified_tables: savepoint_info.modified_tables.clone(),
                write_set_snapshot: savepoint_info.write_set.clone(),
                read_set_snapshot: savepoint_info.read_set.clone(),
                sync_sequence: savepoint_info.sync_sequence,
                savepoint_sequence: savepoint_info.sequence,
            };
            if let Err(e) = position.validate_against(&journal) {
                return Err(TransactionError::rollback_failed(format!(
                    "savepoint journal invariant violated: {}",
                    e
                )));
            }
            // Enforce that OperationLog length is derived from journal, not independent.
            if savepoint_info.operation_log_index != savepoint_info.journal_len
                && savepoint_info.operation_log_index > journal.len()
            {
                return Err(TransactionError::rollback_failed(
                    "savepoint OperationLog index inconsistent with journal",
                ));
            }
        }

        // Use CombinedRollback for comprehensive savepoint rollback
        let rollback = CombinedRollback::new(self);
        rollback
            .rollback_operation_log_to_index(savepoint_info.operation_log_index)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))?;

        {
            let mut manager = self.savepoint_manager.write();
            // Delete savepoints created AFTER the target savepoint using
            // explicit sequence number (not ID). This ensures stable ordering
            // even if IDs are not assigned in strict creation order.
            let target_sequence = savepoint_info.sequence;
            let savepoints_to_remove: Vec<SavepointId> = manager
                .savepoints
                .iter()
                .filter(|(_, sp)| sp.sequence > target_sequence)
                .map(|(&id, _)| id)
                .collect();

            for sp_id in savepoints_to_remove {
                manager.remove_savepoint(sp_id);
            }
        }

        rollback
            .execute_undo_rollback_from_index(
                target,
                self.start_timestamp,
                savepoint_info.undo_log_index,
            )
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))?;

        self.restore_write_set(savepoint_info.write_set);
        self.restore_read_set(savepoint_info.read_set);
        self.truncate_redo_log(savepoint_info.redo_log_index);
        self.local_wal.lock().truncate(
            savepoint_info.local_wal_entry_len,
            savepoint_info.local_wal_intent_len,
        );
        {
            let mut tables = self.modified_tables.lock();
            *tables = savepoint_info.modified_tables.clone();
        }
        {
            let mut journal = self.mutation_journal.write();
            if savepoint_info.journal_len > journal.len() {
                return Err(TransactionError::rollback_failed(
                    "savepoint journal length exceeds current journal",
                ));
            }
            journal.truncate(savepoint_info.journal_len);
            if cfg!(debug_assertions) {
                if let Err(e) = journal.check_invariants() {
                    log::error!("journal invariant after savepoint rollback: {}", e);
                }
            }
        }
        // Recompute mutation count and undo bytes from remaining journal to keep
        // budgets consistent after truncation.
        {
            let journal = self.mutation_journal.read();
            self.mutation_count
                .store(journal.len() as u64, Ordering::Relaxed);
            // undo_bytes is estimated; reset proportionally to remaining entries
            let remaining = journal.len() as u64 * 64;
            self.undo_bytes.store(remaining, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Add undo log
    pub fn add_undo_log(&self, log: UndoLogEntry) -> Result<(), TransactionError> {
        let mut undo_logs = self.undo_logs.write();
        undo_logs
            .add(log)
            .map_err(|error| TransactionError::internal(error.to_string()))
    }

    /// Get undo log length
    pub fn undo_log_len(&self) -> usize {
        let undo_logs = self.undo_logs.read();
        undo_logs.len()
    }

    /// Clear undo logs
    pub fn clear_undo_logs(&self) -> Result<(), TransactionError> {
        let mut undo_logs = self.undo_logs.write();
        undo_logs
            .clear()
            .map_err(|error| TransactionError::internal(error.to_string()))
    }

    /// Execute undo logs for rollback
    pub fn execute_undo_logs<T: UndoTarget + ?Sized>(
        &self,
        target: &T,
    ) -> Result<(), TransactionError> {
        let mut undo_logs = self.undo_logs.write();
        undo_logs
            .execute_undo(target, self.start_timestamp)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))
    }

    /// Execute undo logs starting from a specific index.
    pub fn execute_undo_logs_from_index<T: UndoTarget + ?Sized>(
        &self,
        target: &T,
        start_index: usize,
    ) -> Result<(), TransactionError> {
        let mut undo_logs = self.undo_logs.write();
        undo_logs
            .execute_undo_from_index(target, self.start_timestamp, start_index)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))
    }

    /// Clear all state
    pub fn clear(&self) -> Result<(), TransactionError> {
        self.clear_undo_logs()?;
        self.clear_operation_log();
        {
            let mut write_set = self.write_set.lock();
            *write_set = WriteSet::new();
        }
        self.write_validated.store(false);
        self.rollback_only.store(false);
        self.resources_released.store(false);
        self.staged_bytes.store(0, Ordering::Relaxed);
        {
            let mut tables = self.modified_tables.lock();
            tables.clear();
        }
        {
            let mut manager = self.savepoint_manager.write();
            manager.clear();
        }
        self.truncate_redo_log(0);
        self.local_wal.lock().clear();
        self.restore_read_set(WriteSet::new());
        self.mutation_journal.write().truncate(0);
        self.mutation_count.store(0, Ordering::Relaxed);
        self.undo_bytes.store(0, Ordering::Relaxed);
        Ok(())
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

    fn record_edge_write(&self, edge: graphdb_core::types::EdgeIdentifier) {
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

    fn record_edge_read(&self, edge: graphdb_core::types::EdgeIdentifier) {
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

    #[test]
    fn test_transaction_context_operation_log() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        ctx.add_operation_log(OperationLog::Mutation {
            entities: vec![vec![1, 2, 3]],
            table: Some("vertex".to_string()),
            sequence: Some(0),
        });

        assert_eq!(ctx.operation_log_len(), 1);
        assert!(ctx.get_operation_log(0).is_some());
        assert!(ctx.get_operation_log(0).unwrap().is_canonical());
    }
}
