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
//! ```rust,ignore
//! use graphdb::transaction::{TransactionManager, TransactionOptions};
//!
//! // Create transaction manager
//! let manager = TransactionManager::new(Default::default());
//!
//! // Start read transaction
//! let txn_id = manager.begin_read_transaction(TransactionOptions::default())?;
//!
//! // Execute operations...
//!
//! // Commit transaction
//! manager.commit_transaction(txn_id)?;
//! ```

pub mod cleaner;
pub mod codec;
pub mod conflict;
pub mod context;
pub mod error;
pub mod manager;
pub mod monitor;
pub mod mvcc;
pub mod participant;
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

pub use self::mvcc::{
    InsertTimestampGuard, ReadTimestampGuard, VersionManager, VersionManagerConfig,
    VersionManagerError, VersionManagerResult, RELEASED_TIMESTAMP,
};
pub use self::snapshot_tracker::SnapshotTracker;
pub use cleaner::TransactionCleaner;
pub use conflict::{have_write_conflict, ConflictReport, WriteSetAnalyzer};
pub use context::TransactionContext;
pub use error::{
    RetryableTransactionError, TransactionError, TransactionErrorKind, TransactionResult,
};
pub use manager::{CheckpointGate, CheckpointTransaction, TransactionManager};
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
    AddEdgePropUndo, AddVertexPropUndo, CreateEdgeTypeUndo, CreateVertexTypeUndo,
    DeleteEdgePropUndo, DeleteEdgeTypeUndo, DeleteVertexPropUndo, DeleteVertexTypeUndo,
    FileBackedUndoLog, InsertEdgeUndo, InsertVertexUndo, PropertyValue, RelatedEdgeInfo,
    RemoveEdgeUndo, RemoveVertexUndo, RestoreEdgeUndo, SequenceCreateUndo, SequenceDropUndo,
    SequenceIncrementUndo, UndoLogConfig, UndoLogEntry, UndoLogError, UndoLogManager, UndoLogResult,
    UndoTarget, UpdateEdgePropUndo, UpdateVertexPropUndo,
};
pub use wal::{
    ColumnId, CreateEdgeTypeRedo, CreateVertexTypeRedo, DeleteEdgeRedo, DeleteVertexRedo, EdgeId,
    InsertEdgeRedo, InsertVertexRedo, LabelId, LocalWalParser, LocalWalWriter, Timestamp,
    UpdateEdgePropRedo, UpdateVertexPropRedo, UpdateWalUnit, VertexId, WalConfig, WalContentUnit,
    WalEntryIter, WalError, WalHeader, WalOpType, WalParser, WalParserFactory, WalResult,
    WalWriter,
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
