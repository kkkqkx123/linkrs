//! Transaction execution bindings and savepoint types

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::writeset::WriteSet;
use graphdb_core::types::{Timestamp, TransactionId};

/// Savepoint ID
pub type SavepointId = u64;

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
    /// Local WAL buffer entry count at savepoint creation.
    pub local_wal_entry_len: usize,
    /// Local WAL buffer intent count at savepoint creation.
    pub local_wal_intent_len: usize,
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

/// Transaction Info (for monitoring)
#[derive(Debug, Clone)]
pub struct TransactionInfo {
    pub id: TransactionId,
    pub state: super::TransactionState,
    pub txn_type: super::TransactionType,
    pub start_time: Instant,
    pub elapsed: Duration,
    pub is_read_only: bool,
    pub isolation_level: super::config::IsolationLevel,
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
    isolation_level: super::config::IsolationLevel,
    owner: Option<String>,
    mutation_recorder: Option<Arc<dyn crate::participant::TransactionMutationRecorder>>,
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
            isolation_level: super::config::IsolationLevel::default(),
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

    /// The isolation level of the originating transaction.
    pub fn isolation_level(&self) -> super::config::IsolationLevel {
        self.isolation_level
    }

    pub fn with_isolation_level(mut self, isolation_level: super::config::IsolationLevel) -> Self {
        self.isolation_level = isolation_level;
        self
    }

    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    pub fn mutation_recorder(
        &self,
    ) -> Option<Arc<dyn crate::participant::TransactionMutationRecorder>> {
        self.mutation_recorder.clone()
    }

    pub fn with_mutation_recorder(
        mut self,
        recorder: Arc<dyn crate::participant::TransactionMutationRecorder>,
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
    pub local_wal_entry_len: usize,
    pub local_wal_intent_len: usize,
    pub modified_tables: Vec<String>,
}
