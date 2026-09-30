//! Transaction lifecycle and resource gauges.
use super::core::StatsManager;
use super::metric_type::MetricType;

/// Transaction resource metrics
#[derive(Debug, Clone, Copy)]
pub struct TxnResourceMetrics {
    pub active_statements: u64,
    pub active_snapshots: u64,
    pub pending_writes: u64,
    pub frontier_lag: u64,
    pub staged_wal_bytes: u64,
    pub undo_bytes: u64,
    pub checkpoint_drain_time_ms: u64,
}

impl StatsManager {
    /// Record transaction begin
    pub fn record_txn_begin(&self) {
        self.add_value(MetricType::TxnBeginCount);
        self.add_value(MetricType::TxnActiveCount);
    }

    /// Record transaction commit
    pub fn record_txn_commit(&self) {
        self.add_value(MetricType::TxnCommitCount);
        self.dec_value(MetricType::TxnActiveCount);
    }

    /// Record transaction rollback
    pub fn record_txn_rollback(&self) {
        self.add_value(MetricType::TxnRollbackCount);
        self.dec_value(MetricType::TxnActiveCount);
    }

    /// Record transaction conflict
    pub fn record_txn_conflict(&self) {
        self.add_value(MetricType::TxnConflictCount);
    }

    /// Record transaction timeout.
    pub fn record_txn_timeout(&self) {
        self.add_value(MetricType::TxnTimeoutCount);
    }

    /// Record transaction disconnect.
    pub fn record_txn_disconnect(&self) {
        self.add_value(MetricType::TxnDisconnectCount);
    }

    /// Record transaction recovery abort.
    pub fn record_txn_recovery_abort(&self) {
        self.add_value(MetricType::TxnRecoveryAbortCount);
    }

    /// Record transaction cleanup failure.
    pub fn record_txn_cleanup_failure(&self) {
        self.add_value(MetricType::TxnCleanupFailureCount);
    }

    /// Transaction resource metrics
    pub fn set_txn_resource_metrics(&self, metrics: TxnResourceMetrics) {
        self.set_value(MetricType::TxnActiveStatements, metrics.active_statements);
        self.set_value(MetricType::TxnActiveSnapshots, metrics.active_snapshots);
        self.set_value(MetricType::TxnPendingWrites, metrics.pending_writes);
        self.set_value(MetricType::TxnFrontierLag, metrics.frontier_lag);
        self.set_value(MetricType::TxnStagedWalBytes, metrics.staged_wal_bytes);
        self.set_value(MetricType::TxnUndoBytes, metrics.undo_bytes);
        self.set_value(
            MetricType::TxnCheckpointDrainTimeMs,
            metrics.checkpoint_drain_time_ms,
        );
    }
}
