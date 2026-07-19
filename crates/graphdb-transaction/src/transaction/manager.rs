//! Transaction Manager
//!
//! Manages the lifecycle of all transactions, providing operations such as
//! transaction start, commit, and abort. Uses MVCC version management for
//! snapshot isolation.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use dashmap::DashMap;

use super::cleaner::TransactionCleaner;
use super::context::TransactionContext;
use super::error::TransactionError;
use super::monitor::TransactionMonitor;
use super::mvcc::{VersionManager, VersionManagerConfig};
use super::participant::TransactionCommitSink;
use super::rollback::UndoLogRollback;
use super::types::*;
use super::undo_log::UndoTarget;
use crate::core::stats::StatsManager;
use crate::core::types::Timestamp;
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
                return Err(TransactionError::checkpoint_timeout(active));
            }
            let remaining = timeout - elapsed;
            let result = self.condvar.wait_for(&mut guard, remaining);
            if result.timed_out() {
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
pub struct CheckpointTransaction {
    gate: Arc<CheckpointGate>,
    write_ts: Timestamp,
    committed: bool,
}

impl CheckpointTransaction {
    /// Begin a checkpoint transaction.
    ///
    /// Pauses new write transactions and waits for active writes to drain
    /// (up to `timeout`). Returns `Err(TransactionError::CheckpointTimeout)` if
    /// the drain does not complete in time.
    pub fn begin(
        gate: Arc<CheckpointGate>,
        write_ts: Timestamp,
        timeout: Duration,
    ) -> Result<Self, TransactionError> {
        gate.pause_writes_and_drain(timeout)?;
        Ok(Self {
            gate,
            write_ts,
            committed: false,
        })
    }

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
    pub fn commit(mut self) {
        self.committed = true;
        self.gate.resume_writes();
    }

    /// Abort the checkpoint without writing a WAL marker. Writes are resumed.
    pub fn abort(mut self) {
        self.committed = false;
        self.gate.resume_writes();
    }
}

impl Drop for CheckpointTransaction {
    fn drop(&mut self) {
        if !self.committed {
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
    /// Whether shutdown
    shutdown_flag: AtomicU64,
    /// Transaction monitor for metrics collection
    monitor: TransactionMonitor,
    /// Transaction cleaner for expired transaction cleanup
    cleaner: TransactionCleaner,
    /// Optional sync manager for index cleanup and commit coordination
    sync_manager: Option<Arc<SyncManager>>,
    commit_sink: Option<Arc<dyn TransactionCommitSink>>,
    /// Checkpoint coordination gate
    checkpoint_gate: Arc<CheckpointGate>,
}

impl TransactionManager {
    /// Create a new transaction manager
    pub fn new(config: TransactionManagerConfig) -> Self {
        let stats = Arc::new(TransactionStats::new());
        let monitor = TransactionMonitor::new(Arc::clone(&stats));
        let version_manager = Arc::new(VersionManager::new());
        let cleaner =
            TransactionCleaner::new(None, Arc::clone(&version_manager), Arc::clone(&stats));

        Self {
            version_manager,
            config,
            active_transactions: DashMap::new(),
            id_generator: AtomicU64::new(1),
            stats,
            shutdown_flag: AtomicU64::new(0),
            monitor,
            cleaner,
            sync_manager: None,
            commit_sink: None,
            checkpoint_gate: Arc::new(CheckpointGate::new()),
        }
    }

    /// Create a new transaction manager with version manager config
    pub fn with_version_config(
        config: TransactionManagerConfig,
        vm_config: VersionManagerConfig,
    ) -> Self {
        let stats = Arc::new(TransactionStats::new());
        let monitor = TransactionMonitor::new(Arc::clone(&stats));
        let version_manager = Arc::new(VersionManager::with_config(vm_config));
        let cleaner =
            TransactionCleaner::new(None, Arc::clone(&version_manager), Arc::clone(&stats));

        Self {
            version_manager,
            config,
            active_transactions: DashMap::new(),
            id_generator: AtomicU64::new(1),
            stats,
            shutdown_flag: AtomicU64::new(0),
            monitor,
            cleaner,
            sync_manager: None,
            commit_sink: None,
            checkpoint_gate: Arc::new(CheckpointGate::new()),
        }
    }

    /// Create a new transaction manager with StatsManager integration
    pub fn with_stats_manager(
        config: TransactionManagerConfig,
        stats_manager: Arc<StatsManager>,
    ) -> Self {
        let stats = Arc::new(TransactionStats::with_stats_manager(stats_manager));
        let monitor = TransactionMonitor::new(Arc::clone(&stats));
        let version_manager = Arc::new(VersionManager::new());
        let cleaner =
            TransactionCleaner::new(None, Arc::clone(&version_manager), Arc::clone(&stats));

        Self {
            version_manager,
            config,
            active_transactions: DashMap::new(),
            id_generator: AtomicU64::new(1),
            stats,
            shutdown_flag: AtomicU64::new(0),
            monitor,
            cleaner,
            sync_manager: None,
            commit_sink: None,
            checkpoint_gate: Arc::new(CheckpointGate::new()),
        }
    }

    /// Attach a sync manager after construction.
    pub fn set_sync_manager(&mut self, sync_manager: Arc<SyncManager>) {
        self.cleaner = TransactionCleaner::new(
            Some(sync_manager.clone()),
            Arc::clone(&self.version_manager),
            Arc::clone(&self.stats),
        );
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

        self.cleanup_expired_transactions();

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
            two_phase_commit: options.two_phase_commit,
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
        snapshot_ts: u32,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }

        self.cleanup_expired_transactions();

        let active_count = self.active_transactions.len();
        if active_count >= self.config.max_concurrent_transactions {
            return Err(TransactionError::too_many_transactions());
        }

        let current_write_ts = self.version_manager.next_write_timestamp();
        if snapshot_ts > current_write_ts.saturating_sub(1) {
            return Err(TransactionError::internal(format!(
                "Snapshot timestamp {} is too recent (max: {})",
                snapshot_ts,
                current_write_ts.saturating_sub(1)
            )));
        }

        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let timestamp = self
            .version_manager
            .acquire_read_timestamp()
            .map_err(|e| TransactionError::internal(e.to_string()))?;
        let timeout = options.timeout.unwrap_or(self.config.default_timeout);

        let config = TransactionConfig {
            timeout,
            durability: DurabilityLevel::Async,
            isolation_level: IsolationLevel::RepeatableRead,
            query_timeout: options.query_timeout,
            statement_timeout: options.statement_timeout,
            idle_timeout: options.idle_timeout,
            two_phase_commit: false,
        };

        let mut context = TransactionContext::new_readonly(txn_id, timestamp, config);
        context.set_snapshot_timestamp(snapshot_ts);

        self.active_transactions.insert(txn_id, Arc::new(context));
        self.stats.record_txn_begin();

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

        self.cleanup_expired_transactions();

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
            two_phase_commit: options.two_phase_commit,
        };

        let context = Arc::new(TransactionContext::new(txn_id, timestamp, config));

        self.active_transactions.insert(txn_id, context);
        self.stats.record_txn_begin();

        Ok(txn_id)
    }

    /// Check for write-set based conflicts with active transactions
    ///
    /// This method checks if a transaction's write set conflicts with any other
    /// write transactions that have already passed validation.
    /// After a successful check, the transaction is marked as validated.
    ///
    /// Returns Ok(()) if no conflicts, or Err if conflicts are detected.
    pub fn check_write_set_conflict(&self, txn_id: TransactionId) -> Result<(), TransactionError> {
        let ctx = self
            .active_transactions
            .get(&txn_id)
            .ok_or_else(|| TransactionError::transaction_not_found(txn_id))?;

        if ctx.read_only {
            return Ok(());
        }

        let txn_write_set = ctx.get_write_set();
        if txn_write_set.is_empty() {
            return Ok(());
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

            if ctx.has_write_conflict_with(other_ctx) {
                self.stats.record_txn_conflict();
                return Err(TransactionError::write_transaction_conflict());
            }
        }

        ctx.mark_write_validated();
        Ok(())
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

    /// Check if transaction exists and is active
    pub fn is_transaction_active(&self, txn_id: TransactionId) -> bool {
        self.active_transactions
            .get(&txn_id)
            .map(|entry| entry.value().state().can_execute())
            .unwrap_or(false)
    }

    /// Commit transaction
    ///
    /// Follows atomic commit protocol:
    /// 1. Check state and timeout (transaction still active)
    /// 2. Transition to Committing (marks in-progress, prevents concurrent operations)
    /// 3. Persist through the configured storage commit sink. On retryable failure,
    ///    transitions to CommitRetry, backs off, and retries up to N times.
    /// 4. Release timestamp
    /// 5. Remove from active_transactions (only after all steps succeed)
    /// 6. Transition to Committed
    /// 7. Update stats
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

            if ctx.is_expired() {
                self.stats.increment_timeout();
                self.rollback_context_timestamp(&ctx);
                if !ctx.read_only {
                    self.checkpoint_gate.release_write();
                }
                self.active_transactions.remove(&txn_id);
                return Err(TransactionError::transaction_timeout());
            }

            ctx
        };

        context.transition_to(TransactionState::Committing)?;

        if let Some(ref commit_sink) = self.commit_sink {
            let max_retries = self.config.commit_retry_attempts;
            let mut last_error = None;

            for attempt in 0..=max_retries {
                match commit_sink.commit_transaction(txn_id) {
                    Ok(_) => {
                        last_error = None;
                        break;
                    }
                    Err(e) => {
                        last_error = Some(e);
                        if attempt < max_retries {
                            context.transition_to(TransactionState::CommitRetry)?;
                            std::thread::sleep(Self::backoff_delay(attempt));
                            context.transition_to(TransactionState::Committing)?;
                        }
                    }
                }
            }

            if let Some(err) = last_error {
                self.rollback_context_timestamp(&context);
                if !context.read_only {
                    self.checkpoint_gate.release_write();
                }
                self.active_transactions.remove(&txn_id);
                let _ = context.transition_to(TransactionState::Aborted);
                self.stats.record_txn_rollback();
                return Err(TransactionError::commit_failed(format!(
                    "Failed to persist transaction {} after {} retries: {}",
                    txn_id, max_retries, err
                )));
            }
        }

        if context.read_only {
            self.version_manager.release_read_timestamp();
        } else {
            self.version_manager
                .release_write_timestamp(context.timestamp());
            self.checkpoint_gate.release_write();
        }

        self.active_transactions.remove(&txn_id);

        context.transition_to(TransactionState::Committed)?;

        self.stats.record_txn_commit();

        Ok(())
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

        // Transition to Aborting first, then execute undo logs.
        // If undo fails, we're in Aborting state (not Active) — state machine remains consistent.
        context.transition_to(TransactionState::Aborting)?;

        let rollback = UndoLogRollback::new(&*context);
        rollback
            .execute_rollback(target, context.timestamp())
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))?;
        rollback.clear_logs();

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
        context: &TransactionContext,
    ) -> Result<(), TransactionError> {
        context.transition_to(TransactionState::Aborting)?;
        self.execute_abort_internal(context)
    }

    /// Execute abort steps (transition already done by caller).
    fn execute_abort_internal(&self, context: &TransactionContext) -> Result<(), TransactionError> {
        let max_retries = self.config.abort_retry_attempts;

        if let Some(ref commit_sink) = self.commit_sink {
            let mut last_error = None;
            for attempt in 0..=max_retries {
                match commit_sink.abort_transaction(context.id) {
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
                if !context.read_only {
                    self.checkpoint_gate.release_write();
                }
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
                self.rollback_context_timestamp(context);
                if !context.read_only {
                    self.checkpoint_gate.release_write();
                }
                self.active_transactions.remove(&context.id);
                let _ = context.transition_to(TransactionState::Aborted);
                return Err(TransactionError::sync_failed(format!(
                    "Failed to rollback sync data for transaction {} after {} retries: {}",
                    context.id, max_retries, e
                )));
            }
        }

        if context.read_only {
            self.version_manager.release_read_timestamp();
        } else {
            self.version_manager
                .release_write_timestamp(context.timestamp());
            self.checkpoint_gate.release_write();
        }

        self.active_transactions.remove(&context.id);

        context.transition_to(TransactionState::Aborted)?;

        self.stats.record_txn_rollback();

        Ok(())
    }

    fn rollback_context_timestamp(&self, context: &TransactionContext) {
        if context.read_only {
            self.version_manager.release_read_timestamp();
        } else {
            self.version_manager
                .release_write_timestamp(context.timestamp());
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

    /// Get statistics
    pub fn stats(&self) -> &TransactionStats {
        self.monitor.stats()
    }

    /// Cleanup expired transactions
    pub fn cleanup_expired_transactions(&self) {
        self.cleaner
            .cleanup_expired_transactions(&self.active_transactions);
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
            let _ = self.abort_transaction(txn_id);
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
        Ok(context.create_savepoint(name, 0))
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
    pub fn write_timestamp(&self) -> u32 {
        self.version_manager.write_timestamp()
    }

    /// Get current read timestamp
    pub fn read_timestamp(&self) -> u32 {
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
        self.begin_checkpoint(timeout)?;
        let write_ts = self.version_manager.write_timestamp();
        let result = f(write_ts);
        self.end_checkpoint();
        result.map(|lsn| (write_ts, lsn))
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
    ) -> Result<CheckpointTransaction, TransactionError> {
        let write_ts = self.version_manager.write_timestamp();
        CheckpointTransaction::begin(Arc::clone(&self.checkpoint_gate), write_ts, timeout)
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
                _oe_offset: i32,
                _ie_offset: i32,
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
    fn state_transition_commit_retry_roundtrip() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        assert!(ctx.transition_to(TransactionState::Committing).is_ok());
        assert!(ctx.transition_to(TransactionState::CommitRetry).is_ok());
        assert!(ctx.transition_to(TransactionState::Committing).is_ok());
        assert!(ctx.transition_to(TransactionState::Committed).is_ok());
        assert_eq!(ctx.state(), TransactionState::Committed);
    }

    #[test]
    fn state_transition_commit_retry_to_aborted() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        assert!(ctx.transition_to(TransactionState::Committing).is_ok());
        assert!(ctx.transition_to(TransactionState::CommitRetry).is_ok());
        assert!(ctx.transition_to(TransactionState::Aborted).is_ok());
        assert_eq!(ctx.state(), TransactionState::Aborted);
    }

    #[test]
    fn state_transition_commit_retry_from_aborting() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        // CommitRetry is only reachable from Committing.
        assert!(ctx.transition_to(TransactionState::Committing).is_ok());
        // Cannot go Aborting from CommitRetry.
        assert!(ctx.transition_to(TransactionState::Aborting).is_err());
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
            checkpoint.commit();
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
        checkpoint.abort();
        assert!(!manager.checkpoint_gate().is_paused());
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
