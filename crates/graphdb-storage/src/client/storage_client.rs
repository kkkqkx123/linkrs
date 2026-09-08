use super::{
    StorageAdmin, StorageAuthOps, StorageCommitOps, StorageGcOps, StorageOperationContextOps,
    StoragePersistenceOps, StorageReader, StorageRecoveryOps, StorageSchemaContextOps,
    StorageSchemaOps, StorageWriter,
};
use crate::stats_reader::ColumnStatsReader;
use crate::{AutoCommitBatchOps, AutoCommitGroupOps, UndoTarget};

/// Combined storage interface with full read/write/schema/auth/admin capabilities.
///
/// Runtime context accessors such as schema, transaction, and sync context are kept
/// as separate traits so higher-level components only depend on them when necessary.
pub trait StorageClient:
    StorageReader
    + StorageWriter
    + StorageSchemaOps
    + StorageSchemaContextOps
    + StorageOperationContextOps
    + StorageCommitOps
    + StorageAuthOps
    + StorageAdmin
    + StoragePersistenceOps
    + StorageRecoveryOps
    + StorageGcOps
    + UndoTarget
    + ColumnStatsReader
    + AutoCommitBatchOps
    + AutoCommitGroupOps
    + Send
    + Sync
    + std::fmt::Debug
{
}

impl<T> StorageClient for T where
    T: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + StorageSchemaContextOps
        + StorageOperationContextOps
        + StorageCommitOps
        + StorageAuthOps
        + StorageAdmin
        + StoragePersistenceOps
        + StorageRecoveryOps
        + StorageGcOps
        + UndoTarget
        + ColumnStatsReader
        + AutoCommitBatchOps
        + AutoCommitGroupOps
        + Send
        + Sync
        + std::fmt::Debug
{
}
