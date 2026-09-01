//! Transaction configuration types

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use graphdb_core::types::DurabilityLevel;
pub use graphdb_core::types::TransactionIsolationLevel as IsolationLevel;

/// Concurrency mode for write transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConcurrencyMode {
    /// Standard optimistic concurrency — conflict detection at commit time.
    #[default]
    Optimistic,
    /// Acquire an exclusive write lock at transaction begin time.
    /// Guarantees no conflicts at commit, but limits write concurrency to 1.
    SingleWriter,
}

/// Retry Configuration
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    /// Maximum number of retries
    pub max_retries: u32,
    /// Initial delay before first retry
    pub initial_delay: Duration,
    /// Backoff multiplier for exponential backoff
    pub backoff_multiplier: f64,
    /// Maximum delay between retries
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(100),
            backoff_multiplier: 2.0,
            max_delay: Duration::from_secs(10),
        }
    }
}

impl RetryConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    pub fn with_backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.backoff_multiplier = multiplier;
        self
    }

    pub fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }
}

/// Transaction Options
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactionOptions {
    /// Transaction timeout duration
    pub timeout: Option<Duration>,
    /// Whether read-only
    pub read_only: bool,
    /// Durability level
    pub durability: DurabilityLevel,
    /// Isolation level
    pub isolation_level: IsolationLevel,
    /// Query timeout duration
    pub query_timeout: Option<Duration>,
    /// Statement timeout duration
    pub statement_timeout: Option<Duration>,
    /// Idle timeout duration
    pub idle_timeout: Option<Duration>,
}

impl TransactionOptions {
    /// Create default options
    pub fn new() -> Self {
        Self::default()
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set to read-only
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Set durability level
    pub fn with_durability(mut self, durability: DurabilityLevel) -> Self {
        self.durability = durability;
        self
    }

    /// Set isolation level
    pub fn with_isolation_level(mut self, level: IsolationLevel) -> Self {
        self.isolation_level = level;
        self
    }

    /// Set query timeout
    pub fn with_query_timeout(mut self, timeout: Duration) -> Self {
        self.query_timeout = Some(timeout);
        self
    }

    /// Set statement timeout
    pub fn with_statement_timeout(mut self, timeout: Duration) -> Self {
        self.statement_timeout = Some(timeout);
        self
    }

    /// Set idle timeout
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = Some(timeout);
        self
    }
}

/// Transaction Configuration
#[derive(Debug, Clone)]
pub struct TransactionConfig {
    pub timeout: Duration,
    pub durability: DurabilityLevel,
    pub isolation_level: IsolationLevel,
    pub query_timeout: Option<Duration>,
    pub statement_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
    /// Whether execution bindings created from this configuration own
    /// statement-level transaction finalization.
    pub auto_commit: bool,
    /// Maximum number of mutations per transaction. 0 = unlimited.
    pub max_mutation_count: u64,
    /// Maximum undo log bytes a transaction may accumulate. 0 = unlimited.
    pub max_undo_bytes: u64,
    /// Fraction of budget limit at which a warning is emitted (0.0–1.0).
    /// 0.8 means warn at 80% of the limit. 0.0 disables warnings.
    pub budget_warning_threshold: f64,
    /// Concurrency control strategy for write transactions.
    pub concurrency_mode: ConcurrencyMode,
    /// Serializable full-scan read-set threshold.
    /// When a Serializable transaction's read set exceeds this many entries,
    /// conservative certification is used: any concurrent write since the
    /// transaction started causes an abort. `None` disables full-scan detection.
    pub serializable_full_scan_threshold: Option<usize>,
}

impl Default for TransactionConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            durability: DurabilityLevel::default(),
            isolation_level: IsolationLevel::default(),
            query_timeout: None,
            statement_timeout: None,
            idle_timeout: None,
            auto_commit: false,
            max_mutation_count: 100_000,
            max_undo_bytes: 128 * 1024 * 1024,
            budget_warning_threshold: 0.8,
            concurrency_mode: ConcurrencyMode::Optimistic,
            serializable_full_scan_threshold: Some(10_000),
        }
    }
}

impl TransactionConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_durability(mut self, durability: DurabilityLevel) -> Self {
        self.durability = durability;
        self
    }

    pub fn with_isolation_level(mut self, level: IsolationLevel) -> Self {
        self.isolation_level = level;
        self
    }

    pub fn with_query_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.query_timeout = timeout;
        self
    }

    pub fn with_statement_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.statement_timeout = timeout;
        self
    }

    pub fn with_idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.idle_timeout = timeout;
        self
    }

    pub fn with_auto_commit(mut self, auto_commit: bool) -> Self {
        self.auto_commit = auto_commit;
        self
    }

    pub fn with_max_mutation_count(mut self, max: u64) -> Self {
        self.max_mutation_count = max;
        self
    }

    pub fn with_max_undo_bytes(mut self, max: u64) -> Self {
        self.max_undo_bytes = max;
        self
    }

    pub fn with_budget_warning_threshold(mut self, threshold: f64) -> Self {
        self.budget_warning_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    pub fn with_concurrency_mode(mut self, mode: ConcurrencyMode) -> Self {
        self.concurrency_mode = mode;
        self
    }

    pub fn with_serializable_full_scan_threshold(mut self, threshold: Option<usize>) -> Self {
        self.serializable_full_scan_threshold = threshold;
        self
    }
}

/// Transaction Manager Configuration
#[derive(Debug, Clone)]
pub struct TransactionManagerConfig {
    /// Default transaction timeout duration.
    ///
    /// This value is referenced by the conflict-rate sliding window
    /// (`CONFLICT_WINDOW_BUCKETS` = 60 s) — the window is sized at 2× this
    /// timeout to capture at least two complete timeout cycles. If this
    /// timeout is customised, consider adjusting the window size accordingly
    /// so that `CONFLICT_WINDOW_BUCKETS ≥ default_timeout.as_secs()`.
    pub default_timeout: Duration,
    /// Maximum concurrent transactions (reads + writes)
    pub max_concurrent_transactions: usize,
    /// Whether to automatically cleanup expired transactions
    pub auto_cleanup: bool,
    /// Maximum number of retry attempts for commit sink failures before aborting the transaction.
    pub commit_retry_attempts: u32,
    /// Maximum number of retry attempts for abort/sync rollback failures before reporting failure.
    pub abort_retry_attempts: u32,
    /// Default per-transaction resource budget. Individual transactions inherit
    /// these limits unless overridden.
    pub txn_config: TransactionConfig,
    /// When true, WAL writes and checkpoint coordination are skipped.
    /// Useful for unit tests, benchmarks and temporary in-memory graphs.
    pub in_memory: bool,
    /// When true, WAL group commit is used to batch fsync across concurrent commits.
    pub group_commit_enabled: bool,
    /// Timeout for group commit follower wait.
    pub group_commit_timeout: Duration,
    /// Number of certification shards for write-set conflict detection.
    /// Must be a power of two. Default 64.
    pub cert_shard_count: usize,
    /// Whether to automatically trigger a checkpoint after a successful write
    /// transaction commit when WAL thresholds are exceeded. The checkpoint is
    /// initiated via the [`TransactionCommitSink::auto_checkpoint_if_needed`]
    /// method, which is non-blocking and delegated to the storage layer.
    pub auto_checkpoint_after_commit: bool,
}

impl Default for TransactionManagerConfig {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            max_concurrent_transactions: 1000,
            auto_cleanup: true,
            commit_retry_attempts: 3,
            abort_retry_attempts: 3,
            txn_config: TransactionConfig::default(),
            in_memory: false,
            group_commit_enabled: true,
            group_commit_timeout: Duration::from_secs(30),
            cert_shard_count: 64,
            auto_checkpoint_after_commit: true,
        }
    }
}

impl TransactionManagerConfig {
    /// Enable in-memory mode (skip WAL and checkpoint).
    pub fn with_in_memory(mut self, in_memory: bool) -> Self {
        self.in_memory = in_memory;
        self
    }

    pub fn with_group_commit(mut self, enabled: bool) -> Self {
        self.group_commit_enabled = enabled;
        self
    }

    pub fn with_group_commit_timeout(mut self, timeout: Duration) -> Self {
        self.group_commit_timeout = timeout;
        self
    }

    pub fn with_cert_shard_count(mut self, count: usize) -> Self {
        assert!(
            count.is_power_of_two(),
            "cert_shard_count must be power of two"
        );
        assert!(
            count > 0 && count <= 256,
            "cert_shard_count must be 1..=256"
        );
        self.cert_shard_count = count;
        self
    }

    pub fn with_auto_checkpoint_after_commit(mut self, enabled: bool) -> Self {
        self.auto_checkpoint_after_commit = enabled;
        self
    }

    pub fn is_in_memory(&self) -> bool {
        self.in_memory
    }
}
