//! Administration, maintenance and statistics operations.

use crate::engine::background_freeze::FreezeStats;
use graphdb_core::types::{CompactConfig, PasswordInfo, UserAlterInfo, UserInfo};
use graphdb_core::{Edge, RoleType, StorageError, StorageResult};
use std::sync::Arc;

use super::StorageOperationContext;

/// Authentication and authorization operations.
pub trait StorageAuthOps: Send + Sync + std::fmt::Debug {
    fn change_password(&mut self, info: &PasswordInfo) -> Result<bool, StorageError>;
    fn create_user(&mut self, info: &UserInfo) -> Result<bool, StorageError>;
    fn alter_user(&mut self, info: &UserAlterInfo) -> Result<bool, StorageError>;
    fn drop_user(&mut self, username: &str) -> Result<bool, StorageError>;
    fn user_exists(&self, username: &str) -> bool;
    fn list_users(&self) -> Vec<String>;
    fn get_user(&self, username: &str) -> Option<UserInfo>;
    fn update_last_login(&self, username: &str) -> Result<bool, StorageError>;
    fn list_user_roles(&self, username: &str) -> Vec<(i64, RoleType)>;
    fn list_all_user_roles(&self) -> Vec<(String, Vec<(i64, RoleType)>)>;
    fn grant_role(
        &mut self,
        username: &str,
        space_id: i64,
        role: RoleType,
    ) -> Result<bool, StorageError>;
    fn revoke_role(&mut self, username: &str, space_id: i64) -> Result<bool, StorageError>;
}

/// Administrative operations: stats, maintenance, optional components.
pub trait StorageAdmin: Send + Sync + std::fmt::Debug {
    fn load_from_disk(&mut self) -> Result<(), StorageError>;
    fn save_to_disk(&self) -> Result<(), StorageError>;
    fn get_storage_stats(&self) -> StorageStats;

    fn find_dangling_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;
    fn repair_dangling_edges(&mut self, space: &str) -> Result<usize, StorageError>;

    fn get_db_path(&self) -> &str;
}

/// Persistence operations for flushing, checkpointing, and compaction.
pub trait StoragePersistenceOps: Send + Sync + std::fmt::Debug {
    fn flush(&self) -> StorageResult<()>;

    fn create_checkpoint(&self) -> StorageResult<Option<crate::CheckpointStats>>;

    fn verify_snapshot(&self, snapshot_id: u64) -> StorageResult<bool>;

    fn cleanup_snapshots(&self) -> StorageResult<usize>;

    fn snapshot_stats(&self) -> crate::SnapshotStats;

    fn persistence_diagnostics(&self) -> Option<crate::PersistenceDiagnostics>;

    fn compact(&self, config: &CompactConfig) -> StorageResult<()>;

    fn save_data(&self) -> StorageResult<()> {
        self.flush()
    }

    fn save_data_to_dir(&self, dir: &std::path::Path) -> StorageResult<()>;

    fn auto_flush_if_needed(&self) -> StorageResult<bool> {
        if self.should_flush() {
            self.flush()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn auto_checkpoint_if_needed(&self) -> StorageResult<Option<crate::CheckpointStats>> {
        if self.should_checkpoint() {
            self.create_checkpoint()
        } else {
            Ok(None)
        }
    }

    fn should_flush(&self) -> bool;

    fn should_checkpoint(&self) -> bool;

    fn set_outbox_materialized_lsn_provider(
        &self,
        _provider: Arc<
            dyn Fn() -> StorageResult<Option<graphdb_core::types::CommitLsn>> + Send + Sync,
        >,
    ) {
    }
}

/// Creates an immutable storage handle bound to a single operation context.
pub trait StorageOperationContextOps: Send + Sync + std::fmt::Debug {
    fn bind_auto_commit_context(&self) -> StorageResult<Self>
    where
        Self: Sized;

    /// Bind a read-only statement context with a fixed snapshot timestamp.
    ///
    /// Read statements get a consistent statement-level snapshot: every
    /// storage access observes the same `read_timestamp`, and per-table MVCC
    /// snapshots are lazily registered on first table access so GC keeps the
    /// versions the statement may still read. The snapshot is unregistered by
    /// [`finalize_operation`](Self::finalize_operation) (or on Drop as a
    /// backstop). The bound `(space, snapshot_ts)` pair is also the
    /// serialization boundary for distributed reads.
    ///
    /// The default implementation returns `not_supported`; engines without a
    /// native read context fall back to the unbound handle.
    fn bind_read_operation_context(&self) -> StorageResult<Self>
    where
        Self: Sized,
    {
        Err(StorageError::not_supported(
            "Read operation context binding is not supported by this storage implementation",
        ))
    }

    fn bind_operation_context(&self, context: StorageOperationContext) -> Self
    where
        Self: Sized;

    fn operation_context(&self) -> Option<Arc<StorageOperationContext>>;

    /// Finalize an operation-owned auto-commit timestamp.
    ///
    /// Explicit transaction contexts are finalized by `TransactionManager`
    /// and therefore treat this as a no-op.
    fn finalize_operation(&self, _committed: bool) -> StorageResult<()> {
        Ok(())
    }
}

/// Access to sync runtime context shared with higher-level components.
pub trait StorageSyncContextOps: Send + Sync + std::fmt::Debug {
    fn get_sync_manager(&self) -> Option<Arc<graphdb_sync::SyncManager>>;
}

/// Index GC operations.
pub trait StorageGcOps: Send + Sync + std::fmt::Debug {
    fn is_index_gc_running(&self) -> bool;

    fn start_index_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle>;

    fn stop_index_gc(&self);
}

/// Background freeze operations.
pub trait StorageSnapshotOps: Send + Sync + std::fmt::Debug {
    fn get_freeze_stats(&self) -> Option<FreezeStats>;
    fn trigger_background_freeze(&self) -> StorageResult<()>;
}

/// Storing statistical information
///
/// Size semantics: `total_size_bytes` is allocated bytes (vertex tables plus
/// edge tables, including holes and encoding overhead); `data_size_bytes` is
/// the live-data estimate; `index_size_bytes` is the derived residual
/// `total - data` (fragmentation plus overhead), not an independent index
/// measurement. Row counts mix censuses: vertices count allocated slots while
/// edges count live rows; use the optimizer snapshots for consistent live
/// counts.
#[derive(Debug, Clone)]
pub struct StorageStats {
    pub total_vertices: usize,
    pub total_edges: usize,
    pub total_spaces: usize,
    pub total_tags: usize,
    pub total_edge_types: usize,
    /// Total allocated storage size in bytes (vertex tables + edge tables + indexes)
    pub total_size_bytes: u64,
    /// Data size in bytes (vertex + edge data, excluding index structures)
    pub data_size_bytes: u64,
    /// Property index structure size in bytes
    pub index_size_bytes: u64,
}
