//! Transaction Management Type Definitions
//!
//! Provides core types and structures needed for transaction management

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::undo_log::UndoLogEntry;
use crate::wal::TransactionWalEntry;
use graphdb_core::types::{EdgeIdentifier, VertexId};

pub mod config;
pub mod events;
pub mod execution;
pub mod stats;
pub mod writeset;

pub use config::{
    ConcurrencyMode, DurabilityLevel, IsolationLevel, RetryConfig, TransactionConfig,
    TransactionManagerConfig, TransactionOptions,
};
pub use events::{CommitCallback, RollbackCallback, TransactionEvent};
pub(crate) use execution::SavepointParams;
pub use execution::{SavepointId, SavepointInfo, TransactionExecution, TransactionInfo};
pub use stats::{TransactionMetrics, TransactionResourceMetrics, TransactionStats};
pub use writeset::{ReadRange, ResourceId, SsiState, WriteSet};

/// Transaction ID
pub use graphdb_core::types::TransactionId;

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
    pub index_intents: Vec<graphdb_core::wal::OutboxIntent>,
    pub resource: crate::mutation_journal::MutationResource,
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
        if self.resource == crate::mutation_journal::MutationResource::Unknown {
            if let Some(ref e) = self.redo_entry {
                self.resource = crate::mutation_journal::MutationResource::from_wal_op(e.op_type);
            }
        }
        self
    }

    pub fn with_table(mut self, table: impl Into<String>) -> Self {
        self.modified_table = Some(table.into());
        if self.resource == crate::mutation_journal::MutationResource::Unknown {
            self.resource = crate::mutation_journal::MutationResource::from_modified_table(
                self.modified_table.as_deref(),
            );
        }
        self
    }

    pub fn with_resource(mut self, resource: crate::mutation_journal::MutationResource) -> Self {
        self.resource = resource;
        self
    }

    pub fn with_index_intent(mut self, intent: graphdb_core::wal::OutboxIntent) -> Self {
        self.index_intents.push(intent);
        if self.resource == crate::mutation_journal::MutationResource::Unknown {
            self.resource = crate::mutation_journal::MutationResource::SyncIntent;
        }
        self
    }
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
///
/// The first three variants are user-visible. `Recovery` and `Dummy` are
/// internal lifecycle markers that must not be treated as regular user
/// transactions: `Recovery` is reserved for WAL replay, `Dummy` for
/// operations without a user transaction context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionType {
    ReadOnly,
    Write,
    Checkpoint,
    Recovery,
    Dummy,
}

impl TransactionType {
    pub fn is_user_transaction(&self) -> bool {
        matches!(self, TransactionType::ReadOnly | TransactionType::Write)
    }
    pub fn is_system(&self) -> bool {
        matches!(
            self,
            TransactionType::Checkpoint | TransactionType::Recovery | TransactionType::Dummy
        )
    }
    pub fn requires_wal(&self) -> bool {
        matches!(self, TransactionType::Write | TransactionType::Checkpoint)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use graphdb_core::types::VertexId;

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
