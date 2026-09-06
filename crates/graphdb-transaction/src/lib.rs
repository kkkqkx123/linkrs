//! Transaction Management Module
//!
//! Provides transaction management functionality for GraphDB, including:
//! - Transaction lifecycle management (start, commit, abort)
//! - Transaction statistics and monitoring
//! - MVCC version management
//! - Write-Ahead Log (WAL) for durability
//! - Undo Log for transaction rollback
//!
//! ## Usage Example
//!
//! ```rust
//! use graphdb_transaction::{
//!     TransactionManager, TransactionManagerConfig, TransactionOptions,
//! };
//!
//! let manager = TransactionManager::new(TransactionManagerConfig::default());
//! let txn_id = manager
//!     .begin_read_transaction(TransactionOptions::default())
//!     .unwrap();
//! // execute operations...
//! manager.commit_transaction(txn_id).unwrap();
//! ```

pub mod certify;
pub mod checkpoint;
pub mod cleaner;
pub mod conflict;
pub mod connection;
pub mod context;
pub mod error;
pub mod manager;
pub mod monitor;
pub mod mutation_journal;
pub mod mvcc;
pub mod mvcc_watermarks;
pub mod participant;
pub mod recovery;
pub mod rollback;
pub mod snapshot_tracker;
pub mod types;
pub mod undo_log;
pub mod wal;

#[cfg(test)]
pub mod conflict_integration_test;
#[cfg(test)]
pub mod context_test;
#[cfg(test)]
pub mod manager_test;

pub use self::mutation_journal::{
    MutationJournal, MutationJournalPosition, MutationResource, TransactionMutationRecord,
};
pub use self::mvcc::{
    ReadTimestampGuard, VersionManager, VersionManagerConfig, VersionManagerError,
    VersionManagerResult, RELEASED_TIMESTAMP,
};
pub use self::mvcc_watermarks::{capture_watermarks, MvccWatermarks, NO_ACTIVE_SNAPSHOT};
pub use self::snapshot_tracker::SnapshotTracker;
pub use checkpoint::{CheckpointGate, CheckpointTransaction};
pub use cleaner::TransactionCleaner;
pub use conflict::{have_write_conflict, ConflictReport, WriteSetAnalyzer};
pub use connection::{ConnectionContext, ConnectionId, ConnectionManager, TransactionMode};
pub use context::TransactionContext;
pub use error::{
    RetryableTransactionError, TransactionError, TransactionErrorKind, TransactionResult,
};
pub use manager::TransactionManager;
pub use monitor::TransactionMonitor;
pub use participant::{
    TransactionAbortDescriptor, TransactionCommitDescriptor, TransactionCommitSink,
    TransactionMutationRecorder,
};
pub use rollback::{
    CreateRemoveEdgeUndoParams, CreateRemoveVertexUndoParams, CreateUpdateEdgePropUndoParams,
    RollbackHelper,
};
pub use types::*;
pub use undo_log::{
    CreateEdgeTypeUndo, CreateVertexTypeUndo, FileBackedUndoLog, InsertEdgeUndo, InsertVertexUndo,
    RelatedEdgeInfo, RemoveEdgeUndo, RemoveVertexUndo, RestoreEdgeUndo, UndoLogConfig,
    UndoLogEntry, UndoLogError, UndoLogManager, UndoLogResult, UndoTarget, UpdateEdgePropUndo,
    UpdateVertexPropUndo,
};
pub use wal::{
    dry_replay, ColumnId, CreateEdgeTypeRedo, CreateVertexTypeRedo, DeleteEdgeRedo,
    DeleteVertexRedo, DryReplayResult, DryReplayStats, EdgeId, InsertEdgeRedo, InsertVertexRedo,
    LabelId, LocalWalBuffer, LocalWalBufferConfig, LocalWalParser, LocalWalWriter, Timestamp,
    UpdateEdgePropRedo, UpdateVertexPropRedo, VertexId, WalConfig, WalEntryIter, WalError,
    WalHeader, WalOpType, WalParser, WalParserFactory, WalResult, WalWriter,
};

/// Transaction Management Module Version
pub const VERSION: &str = "2.0.0";

/// Create transaction manager with default configuration
pub fn create_transaction_manager() -> TransactionManager {
    TransactionManager::new(TransactionManagerConfig::default())
}

/// Create transaction manager with custom configuration
pub fn create_transaction_manager_with_config(
    config: TransactionManagerConfig,
) -> TransactionManager {
    TransactionManager::new(config)
}

/// Create read-only transaction options
pub fn readonly_options() -> TransactionOptions {
    TransactionOptions::new().read_only()
}

/// Create high-performance write transaction options (does not guarantee immediate durability)
pub fn high_performance_write_options() -> TransactionOptions {
    TransactionOptions::new().with_durability(DurabilityLevel::None)
}

/// Create repeatable read transaction options
pub fn repeatable_read_options() -> TransactionOptions {
    TransactionOptions::new().with_isolation_level(IsolationLevel::RepeatableRead)
}

/// Create default retry configuration
pub fn default_retry_config() -> RetryConfig {
    RetryConfig::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_version() {
        assert_eq!(VERSION, "2.0.0");
    }

    #[test]
    fn test_create_transaction_manager() {
        let manager = create_transaction_manager();

        let txn_id = manager
            .begin_transaction(TransactionOptions::default())
            .expect("Failed to begin transaction");

        manager
            .commit_transaction(txn_id)
            .expect("Failed to commit transaction");
    }

    #[test]
    fn test_readonly_options() {
        let manager = create_transaction_manager();

        let options = readonly_options();
        let txn_id = manager
            .begin_transaction(options)
            .expect("Failed to begin readonly transaction");

        let ctx = manager
            .get_context(txn_id)
            .expect("Failed to get transaction context");
        assert!(ctx.read_only);

        manager
            .commit_transaction(txn_id)
            .expect("Failed to commit transaction");
    }

    #[test]
    fn test_high_performance_options() {
        let options = high_performance_write_options();
        assert_eq!(options.durability, DurabilityLevel::None);
    }
}
