//! Transaction statistics and metrics

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use graphdb_core::types::Timestamp;

use super::execution::TransactionInfo;

/// Number of 1-second buckets for the conflict rate sliding window.
///
/// The window duration (60 s) is 2× the default transaction timeout (30 s,
/// see `TransactionManagerConfig::default_timeout`). This ensures:
///   - At least 2 complete timeout cycles are captured,
///     providing a stable conflict-rate signal even under moderate load.
///   - Stale conflict counts from bursts older than 60 s are automatically
///     evicted, keeping the metric responsive to current conditions.
///
/// If the transaction timeout is customised, users should ensure the window
/// remains ≥ the timeout to avoid discarding conflict data before the
/// conflicting transaction has a chance to commit or time out.
const CONFLICT_WINDOW_BUCKETS: usize = 60;

/// Transaction Metrics
#[derive(Debug, Default)]
pub struct TransactionMetrics {
    /// Average transaction duration
    pub avg_duration: Duration,
    /// 50th percentile duration
    pub p50_duration: Duration,
    /// 95th percentile duration
    pub p95_duration: Duration,
    /// 99th percentile duration
    pub p99_duration: Duration,
    /// Long transactions (duration > 10s)
    pub long_transactions: Vec<TransactionInfo>,
    /// Total number of transactions
    pub total_count: u64,
    /// Cumulative conflict rate (0.0 to 1.0)
    pub conflict_rate: f64,
    /// Windowed conflicts per second (60s sliding window)
    pub conflict_rate_windowed: f64,
    pub active_transactions: u64,
    pub committed_transactions: u64,
    pub aborted_transactions: u64,
    pub timeout_transactions: u64,
    pub disconnect_transactions: u64,
    pub cleanup_failure_transactions: u64,
    pub active_statements: u64,
}

impl TransactionMetrics {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Resource gauges collected from the transaction manager.
#[derive(Debug, Clone, Copy, Default)]
pub struct TransactionResourceMetrics {
    pub active_snapshots: u64,
    pub pending_writes: i32,
    pub committed_frontier_lag: Timestamp,
    pub staged_wal_bytes: u64,
    pub undo_bytes: u64,
    pub checkpoint_drain_time: Duration,
}

/// Transaction Statistics
#[derive(Debug)]
pub struct TransactionStats {
    /// Total transactions
    pub total_transactions: AtomicU64,
    /// Active transactions
    pub active_transactions: AtomicU64,
    /// Committed transactions
    pub committed_transactions: AtomicU64,
    /// Aborted transactions
    pub aborted_transactions: AtomicU64,
    /// Timeout transactions
    pub timeout_transactions: AtomicU64,
    /// Transactions aborted due to write-set conflicts
    pub conflict_transactions: AtomicU64,
    /// Transactions aborted after a client disconnect.
    pub disconnect_transactions: AtomicU64,
    /// Transactions aborted by recovery processing.
    pub recovery_abort_transactions: AtomicU64,
    /// Transactions whose cleanup protocol reported an error.
    pub cleanup_failure_transactions: AtomicU64,
    /// Number of statements currently being executed.
    pub active_statements: AtomicU64,
    /// Sliding window of conflict counts per second (circular buffer).
    /// `window_buckets[i]` holds the count for the second at index `(window_head + i) % N`.
    window_buckets: Vec<AtomicU64>,
    /// Current head index into the circular buffer, updated by `record_txn_conflict`.
    window_head: AtomicU64,
    /// Last second timestamp when the window was updated.
    window_last_epoch_sec: AtomicU64,
}

impl Default for TransactionStats {
    fn default() -> Self {
        Self {
            total_transactions: AtomicU64::new(0),
            active_transactions: AtomicU64::new(0),
            committed_transactions: AtomicU64::new(0),
            aborted_transactions: AtomicU64::new(0),
            timeout_transactions: AtomicU64::new(0),
            conflict_transactions: AtomicU64::new(0),
            disconnect_transactions: AtomicU64::new(0),
            recovery_abort_transactions: AtomicU64::new(0),
            cleanup_failure_transactions: AtomicU64::new(0),
            active_statements: AtomicU64::new(0),
            window_buckets: (0..CONFLICT_WINDOW_BUCKETS)
                .map(|_| AtomicU64::new(0))
                .collect(),
            window_head: AtomicU64::new(0),
            window_last_epoch_sec: AtomicU64::new(0),
        }
    }
}

impl TransactionStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn increment_total(&self) {
        self.total_transactions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_active(&self) {
        self.active_transactions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_active(&self) {
        self.active_transactions.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn increment_committed(&self) {
        self.committed_transactions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_aborted(&self) {
        self.aborted_transactions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_timeout(&self) {
        self.timeout_transactions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_timeout(&self) {
        self.increment_timeout();
    }

    pub fn increment_disconnect(&self) {
        self.disconnect_transactions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_cleanup_failure(&self) {
        self.cleanup_failure_transactions
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn begin_statement(&self) {
        self.active_statements.fetch_add(1, Ordering::Relaxed);
    }

    pub fn end_statement(&self) {
        let _ =
            self.active_statements
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_sub(1)
                });
    }

    pub fn record_resource_metrics(&self, _metrics: TransactionResourceMetrics) {}

    pub fn record_txn_begin(&self) {
        self.increment_total();
        self.increment_active();
    }

    pub fn record_txn_commit(&self) {
        self.decrement_active();
        self.increment_committed();
    }

    pub fn record_txn_rollback(&self) {
        self.decrement_active();
        self.increment_aborted();
    }

    pub fn record_txn_conflict(&self) {
        self.conflict_transactions.fetch_add(1, Ordering::Relaxed);
        self.record_conflict_in_window();
    }

    fn record_conflict_in_window(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let last = self.window_last_epoch_sec.load(Ordering::Relaxed);
        let mut head = self.window_head.load(Ordering::Relaxed) as usize;

        if now > last {
            let elapsed = (now - last) as usize;
            let n = CONFLICT_WINDOW_BUCKETS;
            if elapsed >= n {
                for bucket in &self.window_buckets {
                    bucket.store(0, Ordering::Relaxed);
                }
                head = 0;
            } else {
                for i in 0..elapsed {
                    let idx = (head + i) % n;
                    self.window_buckets[idx].store(0, Ordering::Relaxed);
                }
                head = (head + elapsed) % n;
            }
            self.window_head.store(head as u64, Ordering::Relaxed);
            self.window_last_epoch_sec.store(now, Ordering::Relaxed);
        }

        let idx = head % CONFLICT_WINDOW_BUCKETS;
        self.window_buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Get the cumulative conflict rate as a ratio (0.0 to 1.0).
    /// Returns total_conflicts / total_transactions.
    pub fn conflict_rate(&self) -> f64 {
        let total = self.total_transactions.load(Ordering::Relaxed);
        if total == 0 {
            0.0
        } else {
            let conflicts = self.conflict_transactions.load(Ordering::Relaxed);
            conflicts as f64 / total as f64
        }
    }

    /// Get the conflict rate over the sliding window (conflicts per second averaged).
    /// Sums all buckets and divides by window size in seconds.
    pub fn conflict_rate_windowed(&self) -> f64 {
        let total: u64 = self
            .window_buckets
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .sum();
        total as f64 / CONFLICT_WINDOW_BUCKETS as f64
    }
}
