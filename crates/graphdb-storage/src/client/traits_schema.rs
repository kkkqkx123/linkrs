//! Schema evolution, context and recovery operations.

use graphdb_core::metadata::{IndexMetadataManager, SchemaManager};
use graphdb_core::types::{EdgeTypeInfo, Index, LabelId, PropertyDef, SpaceInfo, TagInfo};
use graphdb_core::{StorageError, StorageResult};
use graphdb_transaction::wal::recovery::{RecoveryConfig, RecoveryStats};
use std::sync::Arc;

/// Schema/space/tag/edge-type/index DDL operations.
pub trait StorageSchemaOps: Send + Sync + std::fmt::Debug {
    fn create_space(&mut self, space: &mut SpaceInfo) -> Result<bool, StorageError>;
    fn drop_space(&mut self, space: &str) -> Result<bool, StorageError>;
    fn clear_space(&mut self, space: &str) -> Result<bool, StorageError>;
    fn alter_space_comment(&mut self, space_id: u64, comment: String)
        -> Result<bool, StorageError>;

    fn create_tag(&mut self, space: &str, tag: &TagInfo) -> Result<u32, StorageError>;
    fn create_tag_with_estimate(
        &mut self,
        space: &str,
        tag: &TagInfo,
        estimated_rows: Option<u64>,
    ) -> Result<u32, StorageError> {
        let _ = estimated_rows;
        self.create_tag(space, tag)
    }
    fn alter_tag(
        &mut self,
        space: &str,
        tag: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> Result<bool, StorageError>;
    fn rename_vertex_property(
        &mut self,
        label: LabelId,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), StorageError>;
    fn rename_tag_property(
        &mut self,
        space: &str,
        tag: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<bool, StorageError>;
    fn rename_tag(
        &mut self,
        space: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<bool, StorageError>;
    fn drop_tag(&mut self, space: &str, tag: &str) -> Result<bool, StorageError>;

    fn create_edge_type(&mut self, space: &str, edge: &EdgeTypeInfo) -> Result<u32, StorageError>;
    fn alter_edge_type(
        &mut self,
        space: &str,
        edge_type: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> Result<bool, StorageError>;
    fn rename_edge_type(
        &mut self,
        space: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<bool, StorageError>;
    fn update_edge_endpoints(
        &mut self,
        space: &str,
        edge_type: &str,
        src_tag_name: &str,
        dst_tag_name: &str,
    ) -> Result<bool, StorageError>;
    fn drop_edge_type(&mut self, space: &str, edge_type: &str) -> Result<bool, StorageError>;

    fn create_tag_index(&mut self, space: &str, info: &Index) -> Result<bool, StorageError>;
    fn drop_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
    fn rebuild_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;

    fn create_edge_index(&mut self, space: &str, info: &Index) -> Result<bool, StorageError>;
    fn drop_edge_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
    fn rebuild_edge_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
}

/// Access to persistent schema context shared with higher-level components.
pub trait StorageSchemaContextOps: Send + Sync + std::fmt::Debug {
    fn get_schema_manager(&self) -> Option<Arc<SchemaManager>>;
    fn get_index_metadata_manager(&self) -> Option<Arc<dyn IndexMetadataManager>>;
}

/// WAL recovery operations.
pub trait StorageRecoveryOps: Send + Sync + std::fmt::Debug {
    fn needs_recovery(&self) -> bool;

    fn recover_from_wal(&self) -> StorageResult<RecoveryStats>;

    fn recover_from_wal_with_config(&self, config: RecoveryConfig) -> StorageResult<RecoveryStats>;

    fn init_with_recovery(&self) -> StorageResult<Option<RecoveryStats>> {
        if self.needs_recovery() {
            let stats = self.recover_from_wal()?;
            Ok(Some(stats))
        } else {
            Ok(None)
        }
    }
}
