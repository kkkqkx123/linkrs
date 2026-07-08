//! Transaction Management Type Definitions
//!
//! Provides core types and structures needed for transaction management

use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::core::stats::{MetricType, StatsManager};
use crate::core::types::{EdgeIdentifier, VertexId};

/// Transaction ID
pub use crate::core::types::TransactionId;

/// Savepoint ID
pub type SavepointId = u64;

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
}

/// Operation Log
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationLog {
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
    /// Committing
    Committing,
    /// Committed
    Committed,
    /// Aborting
    Aborting,
    /// Aborted
    Aborted,
}

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
        matches!(self, TransactionState::Active)
    }

    /// Check if has ended
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TransactionState::Committed | TransactionState::Aborted
        )
    }
}

impl fmt::Display for TransactionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransactionState::Active => write!(f, "Active"),
            TransactionState::Committing => write!(f, "Committing"),
            TransactionState::Committed => write!(f, "Committed"),
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
    /// Whether to enable two-phase commit
    pub two_phase_commit: bool,
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
            two_phase_commit: false,
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
    /// Whether to enable two-phase commit
    pub two_phase_commit: bool,
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
            two_phase_commit: false,
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

    pub fn with_two_phase_commit(mut self, enabled: bool) -> Self {
        self.two_phase_commit = enabled;
        self
    }
}

/// Transaction Manager Configuration
#[derive(Debug, Clone)]
pub struct TransactionManagerConfig {
    /// Default transaction timeout duration
    pub default_timeout: Duration,
    /// Maximum concurrent transactions
    pub max_concurrent_transactions: usize,
    /// Whether to automatically cleanup expired transactions
    pub auto_cleanup: bool,
    /// Timeout for acquiring storage write lock when beginning a write transaction.
    /// If the write lock cannot be acquired within this duration, the begin operation fails.
    pub write_lock_timeout: Duration,
}

impl Default for TransactionManagerConfig {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            max_concurrent_transactions: 1000,
            auto_cleanup: true,
            write_lock_timeout: Duration::from_secs(10),
        }
    }
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
    /// Optional StatsManager for unified metrics
    stats_manager: Option<Arc<StatsManager>>,
}

impl Default for TransactionStats {
    fn default() -> Self {
        Self {
            total_transactions: AtomicU64::new(0),
            active_transactions: AtomicU64::new(0),
            committed_transactions: AtomicU64::new(0),
            aborted_transactions: AtomicU64::new(0),
            timeout_transactions: AtomicU64::new(0),
            stats_manager: None,
        }
    }
}

impl TransactionStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_stats_manager(stats_manager: Arc<StatsManager>) -> Self {
        Self {
            total_transactions: AtomicU64::new(0),
            active_transactions: AtomicU64::new(0),
            committed_transactions: AtomicU64::new(0),
            aborted_transactions: AtomicU64::new(0),
            timeout_transactions: AtomicU64::new(0),
            stats_manager: Some(stats_manager),
        }
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

    pub fn record_txn_begin(&self) {
        self.increment_total();
        self.increment_active();
        if let Some(ref sm) = self.stats_manager {
            sm.record_txn_begin();
        }
    }

    pub fn record_txn_commit(&self) {
        self.decrement_active();
        self.increment_committed();
        if let Some(ref sm) = self.stats_manager {
            sm.record_txn_commit();
        }
    }

    pub fn record_txn_rollback(&self) {
        self.decrement_active();
        self.increment_aborted();
        if let Some(ref sm) = self.stats_manager {
            sm.record_txn_rollback();
        }
    }

    pub fn record_txn_conflict(&self) {
        if let Some(ref sm) = self.stats_manager {
            sm.add_value(MetricType::TxnConflictCount);
        }
    }
}

/// Write Set - tracks entities modified by a transaction for conflict detection
#[derive(Debug, Clone, Default)]
pub struct WriteSet {
    /// Vertices modified (insert/update/delete)
    pub vertices: HashSet<VertexId>,
    /// Edges modified (insert/update/delete)
    pub edges: HashSet<EdgeIdentifier>,
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

    /// Record an edge write
    pub fn record_edge(&mut self, edge: EdgeIdentifier) {
        self.edges.insert(edge);
    }

    /// Check if write set is empty
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty() && self.edges.is_empty()
    }

    /// Get the number of modified entities
    pub fn size(&self) -> usize {
        self.vertices.len() + self.edges.len()
    }

    /// Check if two write sets have any conflicting entities
    pub fn has_conflict_with(&self, other: &WriteSet) -> bool {
        // Conflict if same vertex is modified
        if !self.vertices.is_disjoint(&other.vertices) {
            return true;
        }

        // Conflict if same edge is modified
        if !self.edges.is_disjoint(&other.edges) {
            return true;
        }

        // Conflict if edges share a source or destination vertex
        for edge1 in &self.edges {
            for edge2 in &other.edges {
                if edge1.src_vid == edge2.src_vid || edge1.dst_vid == edge2.dst_vid {
                    return true;
                }
            }
        }

        false
    }
}

/// Transaction Info (for monitoring)
#[derive(Debug, Clone)]
pub struct TransactionInfo {
    pub id: TransactionId,
    pub state: TransactionState,
    pub start_time: Instant,
    pub elapsed: Duration,
    pub is_read_only: bool,
    pub isolation_level: IsolationLevel,
    pub query_count: u64,
    pub modified_tables: Vec<String>,
    pub savepoint_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_state_transitions() {
        assert!(TransactionState::Active.can_execute());
        assert!(TransactionState::Active.can_commit());
        assert!(TransactionState::Active.can_abort());
        assert!(!TransactionState::Active.is_terminal());

        assert!(!TransactionState::Committed.can_execute());
        assert!(!TransactionState::Committed.can_commit());
        assert!(!TransactionState::Committed.can_abort());
        assert!(TransactionState::Committed.is_terminal());
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
