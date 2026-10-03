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
mod begin;
mod checkpoint;
mod commit;
mod event_registry;
mod leases_checkpoint;
mod monitoring;
mod savepoint;
mod snapshot_lease;
mod statement_scope;

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

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
use graphdb_core::event_dispatch::{EventSubscriptions, SubscriptionId};
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
    /// Declared long reads holding snapshot leases, keyed by transaction.
    /// Short transactions never appear here; every termination path
    /// (commit, abort, timeout) removes the entry.
    pub(super) long_read_leases: DashMap<TransactionId, crate::snapshot_lease::LongReadLease>,
    /// Shared observability sink mirroring lifecycle events into metrics.
    pub(super) observable: Option<Arc<StatsManager>>,
}

impl TransactionManager {
    pub(super) fn with_components(
        config: TransactionManagerConfig,
        stats: Arc<TransactionStats>,
        version_manager: Arc<VersionManager>,
    ) -> Self {
        Self::with_components_and_observable(config, stats, version_manager, None)
    }

    pub(super) fn with_components_and_observable(
        config: TransactionManagerConfig,
        stats: Arc<TransactionStats>,
        version_manager: Arc<VersionManager>,
        observable: Option<Arc<StatsManager>>,
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
            long_read_leases: DashMap::new(),
            observable,
        };
        let commit_stats = Arc::clone(&manager.stats);
        let commit_observable = manager.observable.clone();
        manager.register_commit_callback(Arc::new(move |event| match event {
            TransactionEvent::Committed { .. } => {
                commit_stats.record_txn_commit();
                if let Some(observable) = &commit_observable {
                    observable.record_txn_commit();
                }
            }
            TransactionEvent::CommitDurableButUnfinalized { .. } => {
                commit_stats.record_txn_commit();
                commit_stats.increment_cleanup_failure();
                if let Some(observable) = &commit_observable {
                    observable.record_txn_commit();
                    observable.record_txn_cleanup_failure();
                }
            }
            TransactionEvent::Aborted { .. } => {}
            TransactionEvent::BudgetWarning { .. } => {}
        }));
        let rollback_stats = Arc::clone(&manager.stats);
        let rollback_observable = manager.observable.clone();
        manager.register_rollback_callback(Arc::new(move |event| {
            if let TransactionEvent::Aborted { .. } = event {
                rollback_stats.record_txn_rollback();
                if let Some(observable) = &rollback_observable {
                    observable.record_txn_rollback();
                }
            }
        }));
        manager
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
        stats_manager: Arc<StatsManager>,
    ) -> Self {
        let stats = Arc::new(TransactionStats::new());
        Self::with_components_and_observable(
            config,
            stats,
            Arc::new(VersionManager::new()),
            Some(stats_manager),
        )
    }

    /// Create a transaction manager using the storage engine's MVCC clock.
    pub fn with_shared_version_manager(
        config: TransactionManagerConfig,
        stats_manager: Arc<StatsManager>,
        version_manager: Arc<VersionManager>,
    ) -> Self {
        let stats = Arc::new(TransactionStats::new());
        Self::with_components_and_observable(config, stats, version_manager, Some(stats_manager))
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

    /// Get the version manager
    pub fn version_manager(&self) -> &Arc<VersionManager> {
        &self.version_manager
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

    /// Mark a transaction as disconnected and require rollback before commit.
    pub fn mark_disconnect(&self, txn_id: TransactionId) -> Result<(), TransactionError> {
        let context = self.get_context(txn_id)?;
        context.mark_rollback_only();
        self.stats.increment_disconnect();
        if let Some(observable) = &self.observable {
            observable.record_txn_disconnect();
        }
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

    /// Get configuration
    pub fn config(&self) -> TransactionManagerConfig {
        self.config.clone()
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
    use crate::undo_log::{UndoLogError, UndoLogResult, UndoTarget};
    use graphdb_core::types::{
        ColumnId, CommitLsn, EdgeDeletionContext, EdgeIdentifier, EdgeKey, StagedWriteMark,
        VertexIdentifier,
    };
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

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
            .create_savepoint(txn_id, Some("test".to_string()), None)
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
            fn undo_update_edge_property(
                &self,
                _edge_id: EdgeIdentifier,
                _col_id: ColumnId,
                _value: graphdb_core::Value,
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
            fn staged_write_mark(&self, _txn_id: TransactionId) -> Option<StagedWriteMark> {
                None
            }
            fn rollback_staged_writes(
                &self,
                _txn_id: TransactionId,
                mark: StagedWriteMark,
            ) -> UndoLogResult<()> {
                if mark.is_empty() {
                    Ok(())
                } else {
                    Err(UndoLogError::UndoFailed(
                        "mock undo target holds no staged writes".to_string(),
                    ))
                }
            }
        }

        let sync_manager = Arc::new(SyncManager::new_without_fulltext());
        let manager = TransactionManager::new(TransactionManagerConfig::default())
            .with_sync_manager(sync_manager);

        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("Failed to begin transaction");
        let sp_id = manager
            .create_savepoint(txn_id, Some("sp".to_string()), None)
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
