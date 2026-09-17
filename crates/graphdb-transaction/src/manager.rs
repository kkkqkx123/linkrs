//! Transaction Manager
//!
//! Manages the lifecycle of all transactions, providing operations such as
//! transaction start, commit, and abort. Uses MVCC version management for
//! snapshot isolation.
//!
//! This module is the orchestration facade: transaction lifecycle and
//! statement handling live here, while checkpoint coordination
//! ([`super::checkpoint`]), write-set certification ([`super::certify`]),
//! recovery ([`super::recovery`]) and cleanup ([`super::cleaner`]) are
//! delegated to dedicated modules.

mod abort;
mod checkpoint;
mod commit;
mod monitoring;
mod savepoint;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use dashmap::DashMap;

use super::certify::Certifier;
use super::checkpoint::CheckpointGate;
use super::cleaner::TransactionCleaner;
use super::context::TransactionContext;
use super::error::TransactionError;
use super::monitor::TransactionMonitor;
use super::mvcc::{VersionManager, VersionManagerConfig};
use super::participant::{TransactionCommitSink, TransactionMutationRecorder};
use super::recovery::RecoveryManager;
use super::types::*;
use graphdb_core::event_dispatch::{EventFilter, EventSubscriptions, SubscriptionId};
use graphdb_core::types::Timestamp;
use graphdb_metrics::StatsManager;
use graphdb_sync::SyncManager;

/// Transaction Manager
///
/// Manages the lifecycle of all transactions using MVCC version management.
/// Supports read and insert (write) transactions.
/// All write transactions run concurrently; conflicts are detected at commit time.
pub struct TransactionManager {
    /// Version manager for MVCC timestamps
    pub(super) version_manager: Arc<VersionManager>,
    /// Configuration
    pub(super) config: TransactionManagerConfig,
    /// Active transactions table
    pub(super) active_transactions: DashMap<TransactionId, Arc<TransactionContext>>,
    /// Transaction ID generator
    pub(super) id_generator: AtomicU64,
    /// Statistics
    pub(super) stats: Arc<TransactionStats>,
    // Unified registry for the whole `TransactionEvent` enum.
    // `register_commit/rollback_*` are compatibility vests with built-in
    // filters; `register_txn_callback` receives everything;
    // `BudgetWarning` has its own entry point and no longer rides the
    // commit channel.
    pub(super) txn_callbacks: Arc<EventSubscriptions<TransactionEvent>>,
    /// Pre-commit decision hooks (`register_commit_veto`). Evaluated after
    /// conflict certification and before any WAL I/O; first veto wins.
    /// Separate from the notification registry above: vetoes decide, they
    /// do not observe.
    pub(super) commit_vetoes: parking_lot::RwLock<Vec<(SubscriptionId, CommitVetoCallback)>>,
    pub(super) next_veto_id: AtomicU64,
    /// Whether shutdown
    pub(super) shutdown_flag: AtomicU64,
    /// Transaction monitor for metrics collection
    pub(super) monitor: TransactionMonitor,
    /// Optional sync manager for index cleanup and commit coordination
    pub(super) sync_manager: Option<Arc<SyncManager>>,
    pub(super) commit_sink: Option<Arc<dyn TransactionCommitSink>>,
    /// Checkpoint coordination gate
    pub(super) checkpoint_gate: Arc<CheckpointGate>,
    /// Write-set certifier for conflict detection and committed write-set APIs
    pub(super) certifier: Certifier,
    /// Transaction ID that currently holds the pessimistic write lock (0 = unlocked).
    pub(super) write_exclusion_owner: AtomicU64,
    /// Recovery of transactions whose finalization failed after the WAL was durable.
    pub(super) recovery: RecoveryManager,
    /// Cleanup of expired and idle transactions.
    pub(super) cleaner: TransactionCleaner,
    /// Committed write transactions since the last checkpoint. Compared
    /// against `auto_checkpoint_commit_threshold` by `should_auto_checkpoint`.
    commits_since_checkpoint: AtomicU64,
}

impl TransactionManager {
    pub(super) fn with_components(
        config: TransactionManagerConfig,
        stats: Arc<TransactionStats>,
        version_manager: Arc<VersionManager>,
    ) -> Self {
        let monitor = TransactionMonitor::new(Arc::clone(&stats));
        let checkpoint_gate = Arc::new(CheckpointGate::new());
        let cleaner = TransactionCleaner::new(Arc::clone(&version_manager), Arc::clone(&stats));
        let manager = Self {
            version_manager,
            config: config.clone(),
            active_transactions: DashMap::new(),
            id_generator: AtomicU64::new(1),
            commits_since_checkpoint: AtomicU64::new(0),
            stats,
            txn_callbacks: Arc::new(EventSubscriptions::new()),
            commit_vetoes: parking_lot::RwLock::new(Vec::new()),
            next_veto_id: AtomicU64::new(1),
            shutdown_flag: AtomicU64::new(0),
            monitor,
            sync_manager: None,
            commit_sink: None,
            checkpoint_gate,
            certifier: Certifier::new(),
            write_exclusion_owner: AtomicU64::new(0),
            recovery: RecoveryManager::new(),
            cleaner,
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

    fn is_commit_event(event: &TransactionEvent) -> bool {
        matches!(
            event,
            TransactionEvent::Committed { .. }
                | TransactionEvent::CommitDurableButUnfinalized { .. }
        )
    }

    fn is_rollback_event(event: &TransactionEvent) -> bool {
        matches!(event, TransactionEvent::Aborted { .. })
    }

    fn is_budget_warning_event(event: &TransactionEvent) -> bool {
        matches!(event, TransactionEvent::BudgetWarning { .. })
    }

    /// Register an observer for every transaction lifecycle event.
    pub fn register_txn_callback(&self, callback: TxnCallback) -> SubscriptionId {
        self.txn_callbacks.add(callback)
    }

    /// Register a filtered observer over every transaction lifecycle event.
    pub fn register_txn_callback_filtered(
        &self,
        callback: TxnCallback,
        filter: EventFilter<TransactionEvent>,
    ) -> SubscriptionId {
        self.txn_callbacks.add_filtered(callback, Some(filter))
    }

    /// Remove a previously registered transaction observer. Returns true if present.
    pub fn unregister_txn_callback(&self, id: SubscriptionId) -> bool {
        self.txn_callbacks.remove(id)
    }

    /// Total number of observers on the unified transaction registry (all kinds).
    pub fn txn_callback_count(&self) -> usize {
        self.txn_callbacks.len()
    }

    /// Shared transaction-event registry behind this manager.
    pub fn shared_txn_callbacks(&self) -> Arc<EventSubscriptions<TransactionEvent>> {
        Arc::clone(&self.txn_callbacks)
    }

    /// Register a pre-commit decision hook.
    ///
    /// Evaluated synchronously in registration order after conflict
    /// certification and before any WAL I/O. The first veto blocks the
    /// commit; a panicking hook is logged and treated as allow. A vetoed
    /// commit fails with a `CommitVetoed` error and leaves the transaction
    /// active: the caller must roll it back.
    pub fn register_commit_veto(&self, callback: CommitVetoCallback) -> SubscriptionId {
        let id = self.next_veto_id.fetch_add(1, Ordering::SeqCst);
        self.commit_vetoes.write().push((id, callback));
        id
    }

    /// Remove a previously registered commit veto. Returns true if present.
    pub fn unregister_commit_veto(&self, id: SubscriptionId) -> bool {
        let mut vetoes = self.commit_vetoes.write();
        let before = vetoes.len();
        vetoes.retain(|(entry_id, _)| *entry_id != id);
        vetoes.len() != before
    }

    /// Number of registered commit vetoes.
    pub fn commit_veto_count(&self) -> usize {
        self.commit_vetoes.read().len()
    }

    /// Evaluate vetoes against `context`, returning the first veto reason.
    ///
    /// Snapshots the hook list under the lock, then invokes callbacks
    /// without holding it. Never vetoes when no hook is registered
    /// (zero overhead beyond one lock acquisition).
    pub(super) fn eval_commit_vetoes(&self, context: &TransactionContext) -> Option<String> {
        let vetoes: Vec<CommitVetoCallback> = {
            let guard = self.commit_vetoes.read();
            if guard.is_empty() {
                return None;
            }
            guard.iter().map(|(_, cb)| Arc::clone(cb)).collect()
        };
        let view = CommitVetoContext {
            txn_id: context.id,
            write_timestamp: context.timestamp(),
        };
        for (index, callback) in vetoes.iter().enumerate() {
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(&view)));
            match outcome {
                Ok(decision) if decision.veto => {
                    let reason = decision
                        .reason
                        .unwrap_or_else(|| "commit vetoed".to_string());
                    log::warn!(
                        "commit veto #{} blocked transaction {}: {}",
                        index,
                        view.txn_id,
                        reason,
                    );
                    return Some(reason);
                }
                Ok(_) => {}
                Err(_) => {
                    log::error!("commit veto #{} panicked; treating as allow", index,);
                }
            }
        }
        None
    }

    /// Register a commit observer (compatibility vest: only
    /// `Committed | CommitDurableButUnfinalized` pass the built-in filter).
    pub fn register_commit_callback(&self, callback: CommitCallback) -> SubscriptionId {
        self.txn_callbacks
            .add_filtered(callback, Some(Arc::new(Self::is_commit_event)))
    }

    /// Register a filtered commit observer invoked only when both the
    /// built-in commit filter and `filter` return true.
    pub fn register_commit_callback_filtered(
        &self,
        callback: CommitCallback,
        filter: EventFilter<TransactionEvent>,
    ) -> SubscriptionId {
        let combined: EventFilter<TransactionEvent> =
            Arc::new(move |event| Self::is_commit_event(event) && filter(event));
        self.txn_callbacks.add_filtered(callback, Some(combined))
    }

    /// Remove a previously registered commit observer. Returns true if present.
    pub fn unregister_commit_callback(&self, id: SubscriptionId) -> bool {
        self.txn_callbacks.remove(id)
    }

    /// Total observers on the unified registry (all kinds, not only commits).
    pub fn commit_callback_count(&self) -> usize {
        self.txn_callbacks.len()
    }

    /// Register a rollback observer (compatibility vest: only `Aborted`
    /// passes the built-in filter).
    pub fn register_rollback_callback(&self, callback: RollbackCallback) -> SubscriptionId {
        self.txn_callbacks
            .add_filtered(callback, Some(Arc::new(Self::is_rollback_event)))
    }

    /// Register a filtered rollback observer invoked only when both the
    /// built-in rollback filter and `filter` return true.
    pub fn register_rollback_callback_filtered(
        &self,
        callback: RollbackCallback,
        filter: EventFilter<TransactionEvent>,
    ) -> SubscriptionId {
        let combined: EventFilter<TransactionEvent> =
            Arc::new(move |event| Self::is_rollback_event(event) && filter(event));
        self.txn_callbacks.add_filtered(callback, Some(combined))
    }

    /// Remove a previously registered rollback observer. Returns true if present.
    pub fn unregister_rollback_callback(&self, id: SubscriptionId) -> bool {
        self.txn_callbacks.remove(id)
    }

    /// Total observers on the unified registry (all kinds, not only rollbacks).
    pub fn rollback_callback_count(&self) -> usize {
        self.txn_callbacks.len()
    }

    /// Register a budget-warning observer (only `BudgetWarning` passes).
    pub fn register_budget_warning_callback(&self, callback: TxnCallback) -> SubscriptionId {
        self.txn_callbacks
            .add_filtered(callback, Some(Arc::new(Self::is_budget_warning_event)))
    }

    /// Register a filtered budget-warning observer invoked only when both
    /// the built-in budget-warning filter and `filter` return true.
    pub fn register_budget_warning_callback_filtered(
        &self,
        callback: TxnCallback,
        filter: EventFilter<TransactionEvent>,
    ) -> SubscriptionId {
        let combined: EventFilter<TransactionEvent> =
            Arc::new(move |event| Self::is_budget_warning_event(event) && filter(event));
        self.txn_callbacks.add_filtered(callback, Some(combined))
    }

    /// Remove a previously registered budget-warning observer.
    pub fn unregister_budget_warning_callback(&self, id: SubscriptionId) -> bool {
        self.txn_callbacks.remove(id)
    }

    pub(super) fn emit_commit_event(&self, event: TransactionEvent) {
        if self.txn_callbacks.is_empty() {
            return;
        }
        let outcome = self.txn_callbacks.dispatch_detailed("commit", &event);
        self.stats
            .record_hook_dispatch(outcome.delivered, outcome.panics);
        for _ in 0..outcome.panics {
            self.stats.increment_cleanup_failure();
        }
    }

    pub(super) fn emit_rollback_event(&self, event: TransactionEvent) {
        if self.txn_callbacks.is_empty() {
            return;
        }
        let outcome = self.txn_callbacks.dispatch_detailed("rollback", &event);
        self.stats
            .record_hook_dispatch(outcome.delivered, outcome.panics);
        for _ in 0..outcome.panics {
            self.stats.increment_cleanup_failure();
        }
    }

    pub(super) fn emit_budget_warning_event(&self, event: TransactionEvent) {
        if self.txn_callbacks.is_empty() {
            return;
        }
        let outcome = self.txn_callbacks.dispatch_detailed("txn-budget", &event);
        self.stats
            .record_hook_dispatch(outcome.delivered, outcome.panics);
        for _ in 0..outcome.panics {
            self.stats.increment_cleanup_failure();
        }
    }

    /// Drain pending budget warnings from a context and fan them out as
    /// `TransactionEvent::BudgetWarning` through the unified registry
    /// (budget-warning observers only).
    pub fn drain_context_budget_warnings(&self, context: &TransactionContext) {
        for event in context.drain_budget_warnings() {
            self.emit_budget_warning_event(event);
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

    /// Whether this manager runs in in-memory mode (WAL and checkpoint skipped).
    pub fn is_in_memory(&self) -> bool {
        self.config.in_memory
    }

    /// Create a manager in in-memory mode (skip WAL/checkpoint for testing).
    pub fn in_memory(mut self) -> Self {
        self.config.in_memory = true;
        self
    }

    /// Enable or disable in-memory mode.
    pub fn with_in_memory(mut self, in_memory: bool) -> Self {
        self.config.in_memory = in_memory;
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

    pub(super) fn maybe_cleanup_expired_transactions(&self) {
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
        // In-memory mode skips the gate entirely (no WAL/checkpoint).
        if !self.config.in_memory {
            self.checkpoint_gate.acquire_write()?;
        }

        self.maybe_cleanup_expired_transactions();

        let active_count = self.active_transactions.len();
        if active_count >= self.config.max_concurrent_transactions {
            if !self.config.in_memory {
                self.checkpoint_gate.release_write();
            }
            return Err(TransactionError::too_many_transactions());
        }

        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let timestamp = self
            .version_manager
            .acquire_insert_timestamp()
            .map_err(|e| {
                if !self.config.in_memory {
                    self.checkpoint_gate.release_write();
                }
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
            // Use compare_exchange so a failed contender does not overwrite the
            // legitimate owner's id (swap would corrupt ownership tracking).
            // The timestamp acquired above must be released on contention,
            // otherwise the write frontier stays pinned by an orphaned Pending slot.
            if self
                .write_exclusion_owner
                .compare_exchange(0, txn_id.0, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                self.version_manager.abort_write_timestamp(timestamp);
                if !self.config.in_memory {
                    self.checkpoint_gate.release_write();
                }
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

    /// Check for write-set based conflicts with active transactions
    ///
    /// This method checks if a transaction's write set conflicts with any other
    /// write transactions that have already passed validation.
    /// After a successful check, the transaction is marked as validated.
    ///
    /// Returns Ok(()) if no conflicts, or Err if conflicts are detected.
    pub fn check_write_set_conflict(&self, txn_id: TransactionId) -> Result<(), TransactionError> {
        self.certifier
            .check_write_set_conflict(txn_id, &self.active_transactions, &self.stats)
    }

    /// Prune committed write sets that are no longer needed by any active
    /// transaction. Entries with commit timestamps <= `oldest_active_ts`
    /// are safe to remove.
    pub(super) fn prune_committed_write_sets(&self, oldest_active_ts: Timestamp) {
        self.certifier.prune(oldest_active_ts);
    }

    /// Start a new transaction
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
    ///
    /// Single entry point (no statement-kind flag): the statement kind is not
    /// knowable here — the query is planned after the statement scope opens —
    /// so read-only enforcement lives downstream instead of an intent
    /// parameter: the query layer rejects write operators in read-only scopes
    /// before execution (`reject_writes_outside_transaction_scope`),
    /// `TransactionContext::record_mutation` rejects journal writes on
    /// read-only transactions if they ever reach the journal, and commit
    /// certification rejects them at commit time.
    pub fn begin_statement(
        &self,
        txn_id: TransactionId,
    ) -> Result<(Arc<TransactionContext>, std::time::Instant), TransactionError> {
        let context = self.get_context(txn_id)?;
        if context.isolation_level == IsolationLevel::ReadCommitted {
            let committed = self.version_manager.read_timestamp();
            let snapshot = committed.max(context.timestamp());
            context.set_refreshed_read_ts(snapshot);
            let start = context.begin_statement()?;
            self.pin_statement_snapshot(&context, snapshot)?;
            self.stats.begin_statement();
            Ok((context, start))
        } else {
            let start = context.begin_statement()?;
            self.pin_statement_snapshot(&context, context.timestamp())?;
            self.stats.begin_statement();
            Ok((context, start))
        }
    }

    /// Refresh a transaction's statement snapshot without opening a
    /// materialized statement scope. This is used by lazy result streams.
    ///
    /// Same single-entry contract as `begin_statement`: no statement-kind
    /// flag, read-only enforcement lives in the query layer, the mutation
    /// recorder, and commit certification.
    pub fn refresh_statement_snapshot(
        &self,
        txn_id: TransactionId,
    ) -> Result<Arc<TransactionContext>, TransactionError> {
        let context = self.get_context(txn_id)?;
        context.can_execute()?;
        context.check_timeouts()?;
        if context.isolation_level == IsolationLevel::ReadCommitted {
            let committed = self.version_manager.read_timestamp();
            let snapshot = committed.max(context.timestamp());
            context.set_refreshed_read_ts(snapshot);
            self.pin_statement_snapshot(&context, snapshot)?;
        } else {
            self.pin_statement_snapshot(&context, context.timestamp())?;
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
        self.release_statement_snapshot_pin(context);
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
            context.effective_read_timestamp(),
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
        .with_isolation_level(context.isolation_level)
        .with_mutation_recorder(recorder))
    }

    /// Exponential backoff delay: 100ms * 2^attempt, capped at 10s.
    pub(super) fn backoff_delay(attempt: u32) -> std::time::Duration {
        let ms = 100u64.saturating_mul(2u64.pow(attempt.min(10)));
        std::time::Duration::from_millis(ms.min(10_000))
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

    /// Release a write transaction's manager-owned leases exactly once.
    ///
    /// Single release point shared by the commit path (lease released before
    /// storage finalization so checkpoints do not wait for it) and every
    /// abort path. The one-way `mark_resources_released` flag makes a second
    /// call a no-op: without it, a commit that released the gate and then
    /// failed finalization (or commit-timestamp allocation) would release the
    /// gate a second time when the still-`Committing` transaction is aborted,
    /// underflowing the gate counter and wedging checkpoint drain until
    /// timeout. Timestamp retirement needs no such guard — retiring an
    /// already-terminal slot is a no-op by state check.
    ///
    /// Returns true if this call performed the release.
    pub(super) fn release_write_lease(&self, context: &Arc<TransactionContext>) -> bool {
        if !context.mark_resources_released() {
            return false;
        }
        if context.txn_type != TransactionType::Write {
            return true;
        }
        if context.has_pessimistic_lock() {
            self.write_exclusion_owner.store(0, Ordering::SeqCst);
        }
        if !self.config.in_memory {
            self.checkpoint_gate.release_write();
        }
        true
    }

    /// Pin a statement snapshot in the global snapshot tracker.
    ///
    /// Registers `snapshot` and installs it as the context pin, releasing
    /// any previous pin first so refresh-replace never leaks. GC derives
    /// its cutoff from the tracker, so a pinned statement snapshot cannot
    /// be reclaimed while the statement runs.
    fn pin_statement_snapshot(
        &self,
        context: &Arc<TransactionContext>,
        snapshot: Timestamp,
    ) -> Result<(), TransactionError> {
        if let Err(error) = self
            .version_manager
            .snapshot_tracker()
            .add_snapshot(snapshot)
        {
            return Err(TransactionError::internal(format!(
                "Failed to pin statement snapshot {}: {}",
                snapshot, error
            )));
        }
        if let Some(previous) = context.set_statement_snapshot_pin(snapshot) {
            let _ = self
                .version_manager
                .snapshot_tracker()
                .release_snapshot(previous);
        }
        Ok(())
    }

    /// Release the statement snapshot pin held by `context`, if any.
    ///
    /// Called on every terminal path (statement finish, commit, abort,
    /// recovery re-drive) so pins never outlive their statement. Releasing
    /// a missing entry is ignored: the pin may already have been replaced.
    pub(super) fn release_statement_snapshot_pin(&self, context: &TransactionContext) {
        if let Some(pinned) = context.take_statement_snapshot_pin() {
            if let Err(error) = self
                .version_manager
                .snapshot_tracker()
                .release_snapshot(pinned)
            {
                log::debug!(
                    "Statement snapshot {} release skipped for txn={:?}: {}",
                    pinned,
                    context.id,
                    error
                );
            }
        }
    }

    /// Release the single-writer exclusion when its owner is gone.
    ///
    /// The exclusion is an ownership flag, not a scope guard: if the owner
    /// transaction crashed or was reaped without running the release path,
    /// new writers would wedge forever. The cleaner calls this after every
    /// sweep; a live owner is never disturbed.
    fn release_dead_write_owner(&self) {
        let owner = self.write_exclusion_owner.load(Ordering::SeqCst);
        if owner == 0 {
            return;
        }
        let alive = self
            .active_transactions
            .get(&TransactionId(owner))
            .is_some_and(|entry| {
                entry.value().has_pessimistic_lock() && entry.value().state().can_execute()
            });
        if !alive
            && self
                .write_exclusion_owner
                .compare_exchange(owner, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            log::warn!(
                "Released single-writer exclusion held by dead owner {}",
                owner
            );
        }
    }

    /// Whether a checkpoint should be triggered after the latest commit.
    ///
    /// Threshold-based (commit count since the last checkpoint) instead of a
    /// plain boolean: with the default threshold of 1 the behavior matches
    /// the legacy `auto_checkpoint_after_commit` flag (checkpoint offered
    /// after every write commit); higher thresholds batch the offers. The
    /// decision is only an offer — the commit sink decides via
    /// `auto_checkpoint_if_needed` whether its WAL pressure warrants one.
    /// WAL-byte pressure itself lives in the storage layer
    /// (`should_checkpoint` on WAL size), not here: this counter only
    /// throttles how often the offer is made. In-memory mode never checkpoints.
    pub fn should_auto_checkpoint(&self) -> bool {
        if !self.config.auto_checkpoint_after_commit || self.config.in_memory {
            return false;
        }
        let threshold = self.config.auto_checkpoint_commit_threshold.max(1);
        self.commits_since_checkpoint.load(Ordering::Relaxed) >= threshold
    }

    /// Reset the commits-since-checkpoint counter (called when a checkpoint
    /// begins).
    pub(super) fn reset_checkpoint_commit_counter(&self) {
        self.commits_since_checkpoint.store(0, Ordering::Relaxed);
    }

    /// Number of `Active` write transactions currently in the table.
    ///
    /// Together with `CheckpointGate::active_write_count` this is the
    /// observable pair for lease-leak tests: at quiescence (no in-flight
    /// lifecycle transitions) both must be zero. They are intentionally not
    /// asserted against each other on lifecycle paths because acquire and
    /// table insertion (or state transition and release) cannot be atomic
    /// under concurrent commits.
    pub fn active_write_count(&self) -> usize {
        self.active_transactions
            .iter()
            .filter(|entry| {
                entry.value().txn_type == TransactionType::Write
                    && entry.value().state() == TransactionState::Active
            })
            .count()
    }

    /// Get configuration
    pub fn config(&self) -> TransactionManagerConfig {
        self.config.clone()
    }

    /// Cleanup expired transactions
    pub fn cleanup_expired_transactions(&self) {
        self.cleaner
            .cleanup_expired_transactions_with(&self.active_transactions, |txn_id| {
                self.abort_transaction(txn_id)
            });
        self.release_dead_write_owner();
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
            // Committed transactions are terminal and must never be aborted;
            // they normally leave the table on commit, this is defense in depth.
            if let Some(entry) = self.active_transactions.get(&txn_id) {
                if entry.value().state() == TransactionState::Committed {
                    continue;
                }
            }
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
}

pub(super) fn rollback_context_timestamp(
    version_manager: &VersionManager,
    context: &TransactionContext,
) {
    if !context.txn_type.uses_mvcc() {
        return;
    }
    match context.txn_type {
        TransactionType::ReadOnly => {
            version_manager.release_read_timestamp_at(context.start_timestamp)
        }
        TransactionType::Write => version_manager.abort_write_timestamp(context.timestamp()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TransactionErrorKind;
    use crate::undo_log::{UndoLogResult, UndoTarget};
    use graphdb_core::types::{
        ColumnId, CommitLsn, EdgeDeletionContext, EdgeIdentifier, EdgeKey, VertexIdentifier,
    };
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
    fn commit_veto_blocks_commit_without_aborting() {
        use crate::error::TransactionErrorKind;

        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let commits = Arc::new(AtomicUsize::new(0));
        let commit_count = Arc::clone(&commits);
        manager.register_commit_callback(Arc::new(move |event| {
            if let TransactionEvent::Committed { .. } = event {
                commit_count.fetch_add(1, Ordering::SeqCst);
            }
        }));

        let veto_id = manager.register_commit_veto(Arc::new(|_| VetoDecision::veto("test veto")));
        assert_eq!(manager.commit_veto_count(), 1);

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");
        let err = manager
            .commit_transaction(txn_id)
            .expect_err("vetoed commit must fail");
        assert_eq!(err.kind(), TransactionErrorKind::CommitVetoed);
        // No commit event observed, and the transaction is still active.
        assert_eq!(commits.load(Ordering::SeqCst), 0);
        assert!(manager.get_context(txn_id).is_ok());
        manager
            .abort_transaction(txn_id)
            .expect("vetoed transaction rolls back");

        // After removal the same flow commits normally.
        assert!(manager.unregister_commit_veto(veto_id));
        assert!(!manager.unregister_commit_veto(veto_id));
        assert_eq!(manager.commit_veto_count(), 0);
        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");
        manager
            .commit_transaction(txn_id)
            .expect("transaction should commit");
        assert_eq!(commits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn panicking_veto_is_treated_as_allow() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());
        manager.register_commit_veto(Arc::new(|_| panic!("veto boom")));
        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");
        manager
            .commit_transaction(txn_id)
            .expect("panicking veto must not block commit");
    }

    #[test]
    fn unregistered_callback_stops_receiving_events() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let commits = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&commits);
        let id = manager.register_commit_callback(Arc::new(move |event| {
            if let TransactionEvent::Committed { .. } = event {
                probe.fetch_add(1, Ordering::SeqCst);
            }
        }));
        assert_eq!(manager.commit_callback_count(), 3);
        assert!(manager.unregister_commit_callback(id));
        assert!(!manager.unregister_commit_callback(id));

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");
        manager
            .commit_transaction(txn_id)
            .expect("transaction should commit");
        assert_eq!(commits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unified_registry_delivers_commit_and_abort_to_single_subscription() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let commits = Arc::new(AtomicUsize::new(0));
        let aborts = Arc::new(AtomicUsize::new(0));
        let commit_probe = Arc::clone(&commits);
        let abort_probe = Arc::clone(&aborts);
        // One unified subscription receives both halves.
        manager.register_txn_callback(Arc::new(move |event| match event {
            TransactionEvent::Committed { .. } => {
                commit_probe.fetch_add(1, Ordering::SeqCst);
            }
            TransactionEvent::Aborted { .. } => {
                abort_probe.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
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
    }

    #[test]
    fn commit_vest_filters_out_aborts_and_budget_warnings() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let hits = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&hits);
        manager.register_commit_callback(Arc::new(move |_| {
            probe.fetch_add(1, Ordering::SeqCst);
        }));
        let warnings = Arc::new(AtomicUsize::new(0));
        let warning_probe = Arc::clone(&warnings);
        manager.register_budget_warning_callback(Arc::new(move |_| {
            warning_probe.fetch_add(1, Ordering::SeqCst);
        }));

        // Budget warnings reach only the budget entry point, never the commit vest.
        manager.emit_budget_warning_event(TransactionEvent::BudgetWarning {
            txn_id: TransactionId(1),
            resource: "memory".to_string(),
            current: 8,
            limit: 10,
        });
        assert_eq!(warnings.load(Ordering::SeqCst), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 0);

        // Aborts never leak into the commit vest either.
        let aborted = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("aborted transaction should begin");
        manager
            .abort_transaction(aborted)
            .expect("transaction should abort");
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn committed_event_carries_replay_flag() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let replayed = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&replayed);
        manager.register_commit_callback(Arc::new(move |event| {
            if let TransactionEvent::Committed { replayed: true, .. } = event {
                probe.fetch_add(1, Ordering::SeqCst);
            }
        }));

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");
        manager
            .commit_transaction(txn_id)
            .expect("transaction should commit");
        assert_eq!(replayed.load(Ordering::SeqCst), 0);
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
        use graphdb_sync::SyncManager;

        let sync_manager = Arc::new(SyncManager::new_without_fulltext());
        let manager = TransactionManager::new(TransactionManagerConfig::default())
            .with_sync_manager(sync_manager);

        assert!(manager.sync_manager.is_some());
    }

    #[test]
    fn test_rollback_to_savepoint_with_sync_manager() {
        use graphdb_sync::SyncManager;

        struct MockUndoTarget;
        impl UndoTarget for MockUndoTarget {
            fn delete_vertex_type(&self, _label: crate::LabelId) -> UndoLogResult<()> {
                Ok(())
            }
            fn delete_edge_type(&self, _edge_key: EdgeKey) -> UndoLogResult<()> {
                Ok(())
            }
            fn delete_vertex(
                &self,
                _vertex: VertexIdentifier,
                _ts: crate::Timestamp,
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
                _value: graphdb_core::Value,
                _ts: crate::Timestamp,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn undo_update_edge_property(
                &self,
                _edge_id: EdgeIdentifier,
                _col_id: ColumnId,
                _value: graphdb_core::Value,
                _ts: crate::Timestamp,
            ) -> UndoLogResult<()> {
                Ok(())
            }
            fn revert_delete_vertex(
                &self,
                _vertex: VertexIdentifier,
                _ts: crate::Timestamp,
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
}
