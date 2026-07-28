//! Transaction Management Type Definitions
//!
//! Provides core types and structures needed for transaction management

use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::core::types::{CommitLsn, EdgeIdentifier, Timestamp, VertexId};
use crate::transaction::undo_log::UndoLogEntry;
use crate::transaction::wal::TransactionWalEntry;

/// Transaction ID
pub use crate::core::types::TransactionId;

/// Savepoint ID
pub type SavepointId = u64;

/// Requested terminal action for a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionOutcome {
    Commit,
    Abort,
}

/// Entity identity captured by one logical mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationEntityKey {
    Vertex(VertexId),
    Edge(EdgeIdentifier),
}

/// Complete result of preparing and applying one storage mutation.
///
/// Storage participants can use this value to publish all transaction metadata
/// through one ordered operation: the entity write set is certified first,
/// then undo/redo records and external index intents are retained for the
/// transaction's commit or abort protocol.
#[derive(Debug, Clone, Default)]
pub struct MutationResult {
    pub entity_keys: Vec<MutationEntityKey>,
    pub undo_entry: Option<UndoLogEntry>,
    pub redo_entry: Option<TransactionWalEntry>,
    pub modified_table: Option<String>,
    pub index_intents: Vec<crate::core::wal::OutboxIntent>,
}

impl MutationResult {
    pub fn new(entity_key: MutationEntityKey) -> Self {
        Self {
            entity_keys: vec![entity_key],
            ..Self::default()
        }
    }

    pub fn with_undo(mut self, entry: UndoLogEntry) -> Self {
        self.undo_entry = Some(entry);
        self
    }

    pub fn with_redo(mut self, entry: TransactionWalEntry) -> Self {
        self.redo_entry = Some(entry);
        self
    }

    pub fn with_table(mut self, table: impl Into<String>) -> Self {
        self.modified_table = Some(table.into());
        self
    }

    pub fn with_index_intent(mut self, intent: crate::core::wal::OutboxIntent) -> Self {
        self.index_intents.push(intent);
        self
    }
}

/// Transaction Isolation Level
pub use crate::core::types::TransactionIsolationLevel as IsolationLevel;

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

/// Savepoint Info
#[derive(Debug, Clone)]
pub struct SavepointInfo {
    pub id: SavepointId,
    pub name: Option<String>,
    pub created_at: std::time::Instant,
    /// Explicit creation sequence number (independent from ID)
    /// This ensures stable ordering for rollback-to-savepoint semantics
    pub sequence: u64,
    /// Corresponding operation log index
    pub operation_log_index: usize,
    /// Corresponding undo log index
    pub undo_log_index: usize,
    /// Snapshot of the transaction-local sync sequence at savepoint creation
    pub sync_sequence: u64,
    /// Write set as of savepoint creation.
    pub write_set: WriteSet,
    /// Read set used by Serializable certification as of savepoint creation.
    pub read_set: WriteSet,
    /// Staged redo metadata boundary at savepoint creation.
    pub redo_log_index: usize,
    /// Modified-table metadata as of savepoint creation.
    pub modified_tables: Vec<String>,
}

/// Operation Log
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationLog {
    /// Canonical mutation boundary recorded by the transaction mutation
    /// recorder. The undo log remains the source of rollback actions.
    Mutation {
        entities: Vec<Vec<u8>>,
        table: Option<String>,
    },
    InsertVertex {
        space: String,
        vertex_id: Vec<u8>,
        previous_state: Option<Vec<u8>>,
    },
    UpdateVertex {
        space: String,
        vertex_id: Vec<u8>,
        previous_data: Vec<u8>,
    },
    DeleteVertex {
        space: String,
        vertex_id: Vec<u8>,
        vertex: Vec<u8>,
    },
    InsertEdge {
        space: String,
        edge_id: Vec<u8>,
        previous_state: Option<Vec<u8>>,
    },
    UpdateEdge {
        space: String,
        edge_id: Vec<u8>,
        previous_data: Vec<u8>,
    },
    DeleteEdge {
        space: String,
        edge_id: Vec<u8>,
        edge: Vec<u8>,
    },
}

/// Transaction State
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionState {
    /// Active state, can execute read-write operations
    Active,
    /// Commit in progress
    Committing,
    /// Abort in progress
    Aborting,
    /// Aborted (terminal)
    Aborted,
}

/// Logical transaction category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionType {
    ReadOnly,
    Write,
    Checkpoint,
}

/// Immutable lifecycle notification emitted after a transaction leaves the
/// active transaction table.
#[derive(Debug, Clone)]
pub enum TransactionEvent {
    Committed {
        txn_id: TransactionId,
        write_timestamp: Timestamp,
        write_set: Box<WriteSet>,
        schema_catalog_version: u64,
    },
    Aborted {
        txn_id: TransactionId,
        write_timestamp: Timestamp,
    },
    CommitDurableButUnfinalized {
        txn_id: TransactionId,
        write_timestamp: Timestamp,
        commit_lsn: CommitLsn,
    },
    BudgetWarning {
        txn_id: TransactionId,
        resource: String,
        current: u64,
        limit: u64,
    },
}

pub type CommitCallback = Arc<dyn Fn(&TransactionEvent) + Send + Sync>;
pub type RollbackCallback = Arc<dyn Fn(&TransactionEvent) + Send + Sync>;

impl TransactionState {
    /// Check if operation can be executed
    pub fn can_execute(&self) -> bool {
        matches!(self, TransactionState::Active)
    }

    /// Check if can commit
    pub fn can_commit(&self) -> bool {
        matches!(self, TransactionState::Active)
    }

    /// Check if can abort
    pub fn can_abort(&self) -> bool {
        matches!(
            self,
            TransactionState::Active | TransactionState::Committing | TransactionState::Aborting
        )
    }

    /// Check if has reached a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(self, TransactionState::Aborted)
    }
}

impl fmt::Display for TransactionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransactionState::Active => write!(f, "Active"),
            TransactionState::Committing => write!(f, "Committing"),
            TransactionState::Aborting => write!(f, "Aborting"),
            TransactionState::Aborted => write!(f, "Aborted"),
        }
    }
}

/// Transaction Options
#[derive(Debug, Clone, PartialEq)]
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

impl Default for TransactionOptions {
    fn default() -> Self {
        Self {
            timeout: None,
            read_only: false,
            durability: DurabilityLevel::Sync,
            isolation_level: IsolationLevel::default(),
            query_timeout: None,
            statement_timeout: None,
            idle_timeout: None,
        }
    }
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

/// Durability Level
pub use crate::core::types::DurabilityLevel;

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
            durability: DurabilityLevel::Sync,
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
    /// Default transaction timeout duration
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
        }
    }
}

/// Number of 1-second buckets for the conflict rate sliding window.
const CONFLICT_WINDOW_BUCKETS: usize = 60;

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

    pub fn record_resource_metrics(
        &self,
        _metrics: TransactionResourceMetrics,
    ) {
    }

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

/// Concurrency mode for write transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConcurrencyMode {
    /// Standard optimistic concurrency — conflict detection at commit time.
    #[default]
    Optimistic,
    /// Acquire an exclusive write lock at transaction begin time.
    /// Guarantees no conflicts at commit, but limits write concurrency to 1.
    Pessimistic,
}

/// Write Set - tracks entities modified by a transaction for conflict detection
#[derive(Debug, Clone, Default)]
pub struct WriteSet {
    /// Vertices modified (insert/update/delete)
    pub vertices: HashSet<VertexId>,
    /// Edges modified (insert/update/delete)
    pub edges: HashSet<EdgeIdentifier>,
    /// Vertex IDs used as edge endpoints (source/destination).
    /// Collected for O(1) endpoint lookup.
    pub edge_endpoints: HashSet<VertexId>,
    /// Vertices deleted by this transaction. This is narrower than `vertices`
    /// and is used for vertex-delete versus edge-write certification.
    pub deleted_vertices: HashSet<VertexId>,
    /// Schema resources changed by this transaction.
    pub schema_resources: HashSet<String>,
    /// Index resources changed by this transaction.
    pub index_resources: HashSet<String>,
    /// Predicate-based read ranges for Serializable phantom detection.
    pub read_ranges: Vec<ReadRange>,
}

impl WriteSet {
    /// Create an empty write set
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a vertex write
    pub fn record_vertex(&mut self, vid: VertexId) {
        self.vertices.insert(vid);
    }

    /// Record a vertex deletion and retain the deletion kind for certification.
    pub fn record_vertex_delete(&mut self, vid: VertexId) {
        self.vertices.insert(vid);
        self.deleted_vertices.insert(vid);
    }

    /// Record an edge write
    pub fn record_edge(&mut self, edge: EdgeIdentifier) {
        self.edge_endpoints.insert(edge.src_vid);
        self.edge_endpoints.insert(edge.dst_vid);
        self.edges.insert(edge);
    }

    pub fn record_schema_resource(&mut self, resource: impl Into<String>) {
        self.schema_resources.insert(resource.into());
    }

    pub fn record_index_resource(&mut self, resource: impl Into<String>) {
        self.index_resources.insert(resource.into());
    }

    /// Record a predicate-based read range for Serializable phantom detection.
    pub fn record_read_range(&mut self, range: ReadRange) {
        self.read_ranges.push(range);
    }

    /// Check whether any committed write falls within a recorded read range.
    pub fn has_read_range_conflict_with(&self, committed: &WriteSet) -> bool {
        for range in &self.read_ranges {
            for vid in &committed.vertices {
                if range.contains(vid) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if write set is empty
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
            && self.edges.is_empty()
            && self.schema_resources.is_empty()
            && self.index_resources.is_empty()
    }

    /// Get the number of modified entities
    pub fn size(&self) -> usize {
        self.vertices.len() + self.edges.len()
    }

    /// Check if two write sets have any conflicting entities.
    ///
    /// Conflict is defined as: same vertex modified OR same edge modified.
    /// Edges sharing endpoints (source/destination) without actually modifying
    /// the same entity are NOT considered conflicting.
    pub fn has_conflict_with(&self, other: &WriteSet) -> bool {
        if !self.vertices.is_disjoint(&other.vertices) {
            return true;
        }
        if !self.edges.is_disjoint(&other.edges) {
            return true;
        }
        if !self.deleted_vertices.is_disjoint(&other.edge_endpoints)
            || !other.deleted_vertices.is_disjoint(&self.edge_endpoints)
        {
            return true;
        }
        if !self.schema_resources.is_disjoint(&other.schema_resources)
            || !self.index_resources.is_disjoint(&other.index_resources)
        {
            return true;
        }
        // Schema changes affect the physical data layout. Certify them
        // against concurrent data writes even when the entity keys differ.
        if (!self.schema_resources.is_empty()
            && (!other.vertices.is_empty() || !other.edges.is_empty()))
            || (!other.schema_resources.is_empty()
                && (!self.vertices.is_empty() || !self.edges.is_empty()))
        {
            return true;
        }
        false
    }
}

/// A predicate-based range of vertex IDs read by a Serializable transaction.
///
/// Used for phantom detection: if a concurrent write creates a vertex whose
/// ID falls within this range and matches the label, the Serializable
/// transaction is aborted to prevent phantoms.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadRange {
    /// Vertex label (vertex type name).
    pub label: String,
    /// Optional property column name for the indexed predicate.
    pub column: Option<String>,
    /// Lower bound (inclusive when `start_inclusive` is true).
    pub start: Option<VertexId>,
    /// Upper bound (inclusive when `end_inclusive` is true).
    pub end: Option<VertexId>,
}

impl ReadRange {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            column: None,
            start: None,
            end: None,
        }
    }

    pub fn with_column(mut self, column: impl Into<String>) -> Self {
        self.column = Some(column.into());
        self
    }

    pub fn with_start(mut self, start: VertexId) -> Self {
        self.start = Some(start);
        self
    }

    pub fn with_end(mut self, end: VertexId) -> Self {
        self.end = Some(end);
        self
    }

    /// Check whether the given `VertexId` falls within this range.
    pub fn contains(&self, vid: &VertexId) -> bool {
        if let Some(ref start) = self.start {
            let cmp = vid.as_bytes().cmp(start.as_bytes());
            if cmp == std::cmp::Ordering::Less {
                return false;
            }
        }
        if let Some(ref end) = self.end {
            let cmp = vid.as_bytes().cmp(end.as_bytes());
            if cmp == std::cmp::Ordering::Greater {
                return false;
            }
        }
        true
    }
}

/// Transaction Info (for monitoring)
#[derive(Debug, Clone)]
pub struct TransactionInfo {
    pub id: TransactionId,
    pub state: TransactionState,
    pub txn_type: TransactionType,
    pub start_time: Instant,
    pub elapsed: Duration,
    pub is_read_only: bool,
    pub isolation_level: IsolationLevel,
    pub query_count: u64,
    pub mutation_count: u64,
    pub modified_tables: Vec<String>,
    pub savepoint_count: usize,
    pub read_timestamp: Timestamp,
    pub write_timestamp: Timestamp,
    pub owner: Option<String>,
    pub last_activity: Duration,
    pub rollback_only: bool,
    pub blocking_reason: Option<String>,
    pub staged_bytes: u64,
    pub undo_bytes: u64,
}

/// Immutable execution binding for a single query request.
///
/// Created by `TransactionManager` and passed explicitly into the query layer.
/// Guarantees that every DML operation carries a single, consistent transaction
/// identity from API entry through storage/WAL.
#[derive(Debug, Clone)]
pub struct TransactionExecution {
    transaction_id: TransactionId,
    read_timestamp: Timestamp,
    write_timestamp: Option<Timestamp>,
    read_only: bool,
    auto_commit: bool,
    rollback_only: bool,
    owner: Option<String>,
    mutation_recorder: Option<Arc<dyn super::participant::TransactionMutationRecorder>>,
}

impl TransactionExecution {
    pub fn new(
        transaction_id: TransactionId,
        read_timestamp: Timestamp,
        write_timestamp: Option<Timestamp>,
        read_only: bool,
        auto_commit: bool,
        owner: Option<String>,
    ) -> Self {
        Self {
            transaction_id,
            read_timestamp,
            write_timestamp,
            read_only,
            auto_commit,
            rollback_only: false,
            owner,
            mutation_recorder: None,
        }
    }

    pub fn transaction_id(&self) -> TransactionId {
        self.transaction_id
    }

    pub fn read_timestamp(&self) -> Timestamp {
        self.read_timestamp
    }

    pub fn write_timestamp(&self) -> Option<Timestamp> {
        self.write_timestamp
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn auto_commit(&self) -> bool {
        self.auto_commit
    }

    pub fn rollback_only(&self) -> bool {
        self.rollback_only
    }

    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    pub fn mutation_recorder(
        &self,
    ) -> Option<Arc<dyn super::participant::TransactionMutationRecorder>> {
        self.mutation_recorder.clone()
    }

    pub fn with_mutation_recorder(
        mut self,
        recorder: Arc<dyn super::participant::TransactionMutationRecorder>,
    ) -> Self {
        self.mutation_recorder = Some(recorder);
        self
    }

    pub fn with_rollback_only(mut self, rollback_only: bool) -> Self {
        self.rollback_only = rollback_only;
        self
    }

    pub fn is_writable(&self) -> bool {
        !self.read_only && !self.rollback_only
    }

    pub fn requires_finalization(&self) -> bool {
        self.auto_commit
    }
}

/// Parameters for creating a savepoint.
#[derive(Debug, Clone)]
pub(crate) struct SavepointParams {
    pub name: Option<String>,
    pub operation_log_index: usize,
    pub undo_log_index: usize,
    pub sync_sequence: u64,
    pub write_set: WriteSet,
    pub read_set: WriteSet,
    pub redo_log_index: usize,
    pub modified_tables: Vec<String>,
}

/// Resource gauges collected from the transaction manager.
#[derive(Debug, Clone, Copy, Default)]
pub struct TransactionResourceMetrics {
    pub active_snapshots: u64,
    pub pending_writes: i32,
    pub committed_frontier_lag: Timestamp,
    pub staged_wal_bytes: u64,
    pub undo_bytes: u64,
    pub prepared_transactions: u64,
    pub checkpoint_drain_time: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_state_predicates() {
        assert!(TransactionState::Active.can_execute());
        assert!(TransactionState::Active.can_commit());
        assert!(TransactionState::Active.can_abort());
        assert!(!TransactionState::Active.is_terminal());

        assert!(!TransactionState::Committing.can_execute());
        assert!(!TransactionState::Committing.can_commit());
        assert!(TransactionState::Committing.can_abort());
        assert!(!TransactionState::Committing.is_terminal());

        assert!(!TransactionState::Aborting.can_execute());
        assert!(!TransactionState::Aborting.can_commit());
        assert!(TransactionState::Aborting.can_abort());
        assert!(!TransactionState::Aborting.is_terminal());

        assert!(!TransactionState::Aborted.can_execute());
        assert!(!TransactionState::Aborted.can_commit());
        assert!(!TransactionState::Aborted.can_abort());
        assert!(TransactionState::Aborted.is_terminal());
    }

    #[test]
    fn test_transaction_options_builder() {
        let options = TransactionOptions::new()
            .with_timeout(Duration::from_secs(60))
            .read_only()
            .with_durability(DurabilityLevel::None);

        assert_eq!(options.timeout, Some(Duration::from_secs(60)));
        assert!(options.read_only);
        assert_eq!(options.durability, DurabilityLevel::None);
    }

    #[test]
    fn test_transaction_stats() {
        let stats = TransactionStats::new();

        stats.increment_total();
        stats.increment_active();

        assert_eq!(stats.total_transactions.load(Ordering::Relaxed), 1);
        assert_eq!(stats.active_transactions.load(Ordering::Relaxed), 1);

        stats.decrement_active();
        stats.increment_committed();

        assert_eq!(stats.active_transactions.load(Ordering::Relaxed), 0);
        assert_eq!(stats.committed_transactions.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_conflict_rate_tracking() {
        let stats = TransactionStats::new();

        // 4 total, 1 conflict => 25% rate
        for _ in 0..4 {
            stats.increment_total();
        }
        stats.record_txn_conflict();

        assert_eq!(stats.conflict_transactions.load(Ordering::Relaxed), 1);
        assert!((stats.conflict_rate() - 0.25).abs() < f64::EPSILON);

        // No transactions => 0.0
        let empty = TransactionStats::new();
        assert_eq!(empty.conflict_rate(), 0.0);
    }

    #[test]
    fn test_conflict_rate_windowed() {
        let stats = TransactionStats::new();

        // Empty window => 0.0
        assert_eq!(stats.conflict_rate_windowed(), 0.0);

        // Record 10 conflicts in the current bucket
        for _ in 0..10 {
            stats.record_txn_conflict();
        }

        // 10 conflicts / 60 buckets = ~0.167 conf/sec average
        let rate = stats.conflict_rate_windowed();
        assert!((rate - 10.0 / 60.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_write_set_empty() {
        let ws = WriteSet::new();
        assert!(ws.is_empty());
        assert_eq!(ws.size(), 0);
    }

    #[test]
    fn test_write_set_record_vertex() {
        let mut ws = WriteSet::new();
        let vid = VertexId::from_int64(1);

        ws.record_vertex(vid);
        assert!(!ws.is_empty());
        assert_eq!(ws.size(), 1);
        assert!(ws.vertices.contains(&vid));
    }

    #[test]
    fn test_write_set_conflict_same_vertex() {
        let vid = VertexId::from_int64(1);

        let mut ws1 = WriteSet::new();
        ws1.record_vertex(vid);

        let mut ws2 = WriteSet::new();
        ws2.record_vertex(vid);

        assert!(ws1.has_conflict_with(&ws2));
        assert!(ws2.has_conflict_with(&ws1));
    }

    #[test]
    fn test_write_set_no_conflict_different_vertices() {
        let vid1 = VertexId::from_int64(1);
        let vid2 = VertexId::from_int64(2);

        let mut ws1 = WriteSet::new();
        ws1.record_vertex(vid1);

        let mut ws2 = WriteSet::new();
        ws2.record_vertex(vid2);

        assert!(!ws1.has_conflict_with(&ws2));
        assert!(!ws2.has_conflict_with(&ws1));
    }
}
