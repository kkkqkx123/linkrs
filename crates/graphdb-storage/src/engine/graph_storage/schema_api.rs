use graphdb_core::types::{EdgeTypeInfo, Index, LabelId, PropertyDef, SpaceInfo, TagInfo};
use graphdb_core::StorageError;

use crate::{StorageOperationContext, StorageSchemaOps};

use super::GraphStorage;
use super::{index_manager, ops, reader, schema_engine, schema_writer};

impl StorageSchemaOps for GraphStorage {
    fn create_space(&mut self, space: &mut SpaceInfo) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::create_space(&self.ctx, space)
    }

    fn drop_space(&mut self, space: &str) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::drop_space(&self.ctx, space)
    }

    fn clear_space(&mut self, space: &str) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::clear_space(&self.ctx, space)
    }

    fn alter_space_comment(
        &mut self,
        space_id: u64,
        comment: String,
    ) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::alter_space_comment(&self.ctx, space_id, comment)
    }

    fn create_tag(&mut self, space: &str, tag: &TagInfo) -> Result<u32, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::create_tag(&self.ctx, space, tag)
    }

    fn alter_tag(
        &mut self,
        space: &str,
        tag_name: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::alter_tag(&self.ctx, space, tag_name, additions, deletions)
    }

    fn rename_vertex_property(
        &mut self,
        label: LabelId,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), StorageError> {
        self.ctx.check_write_admission()?;
        schema_engine::rename_vertex_property(&self.ctx, label, old_name, new_name)
    }

    fn rename_tag_property(
        &mut self,
        space: &str,
        tag: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        let renamed = self
            .ctx
            .schema_manager()
            .rename_tag_property(space, tag, old_name, new_name)?;
        if renamed {
            let label_id = ops::tag_label_id(&self.ctx, space, tag)?
                .ok_or_else(|| StorageError::label_not_found(tag.to_string()))?;
            schema_engine::rename_vertex_property(&self.ctx, label_id, old_name, new_name)?;
        }
        Ok(renamed)
    }

    fn drop_tag(&mut self, space: &str, tag: &str) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::drop_tag(&self.ctx, space, tag)
    }

    fn create_edge_type(
        &mut self,
        space: &str,
        edge_type: &EdgeTypeInfo,
    ) -> Result<u32, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::create_edge_type(&self.ctx, space, edge_type)
    }

    fn alter_edge_type(
        &mut self,
        space: &str,
        edge_type_name: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::alter_edge_type(&self.ctx, space, edge_type_name, additions, deletions)
    }

    fn drop_edge_type(&mut self, space: &str, edge_type: &str) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::drop_edge_type(&self.ctx, space, edge_type)
    }

    fn create_tag_index(&mut self, space: &str, index: &Index) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        let created = schema_writer::create_tag_index(&self.ctx, space, index)?;
        if created {
            self.rebuild_tag_index(space, &index.name)?;
        }
        Ok(created)
    }

    fn drop_tag_index(&mut self, space: &str, index_name: &str) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::drop_tag_index(&self.ctx, space, index_name)
    }

    fn rebuild_tag_index(&mut self, space: &str, index_name: &str) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        let rebuild_gate = self.ctx.index_data_manager().read().rebuild_gate();
        let _rebuild_guard = rebuild_gate.write();
        let snapshot_timestamp = self.ctx.get_read_timestamp();
        let start_lsn = match index_manager::current_wal_lsn(&self.ctx) {
            graphdb_core::types::CommitLsn::ZERO => graphdb_core::types::CommitLsn::new(1),
            lsn => lsn,
        };
        let snapshot_ctx = self.ctx.with_operation_context(StorageOperationContext {
            transaction_id: None,
            read_timestamp: snapshot_timestamp,
            write_timestamp: None,
            read_only: true,
            auto_commit: false,
            mutation_recorder: None,
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
            auto_commit_group_start: None,
        });
        let vertices = reader::scan_vertices(&snapshot_ctx, space)?;
        let result = index_manager::rebuild_tag_index(
            &self.ctx,
            space,
            index_name,
            &vertices,
            graphdb_core::types::SnapshotTimestamp::new(snapshot_timestamp),
            start_lsn,
        );
        if let Some(stats) = self.ctx.stats_manager() {
            if result.is_err() {
                stats.record_generation_rebuild_failure();
            }
        }
        result
    }

    fn create_edge_index(&mut self, space: &str, index: &Index) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::create_edge_index(&self.ctx, space, index)
    }

    fn drop_edge_index(&mut self, space: &str, index_name: &str) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        schema_writer::drop_edge_index(&self.ctx, space, index_name)
    }

    fn rebuild_edge_index(&mut self, space: &str, index_name: &str) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        let rebuild_gate = self.ctx.index_data_manager().read().rebuild_gate();
        let _rebuild_guard = rebuild_gate.write();
        let snapshot_timestamp = self.ctx.get_read_timestamp();
        let start_lsn = match index_manager::current_wal_lsn(&self.ctx) {
            graphdb_core::types::CommitLsn::ZERO => graphdb_core::types::CommitLsn::new(1),
            lsn => lsn,
        };
        let snapshot_ctx = self.ctx.with_operation_context(StorageOperationContext {
            transaction_id: None,
            read_timestamp: snapshot_timestamp,
            write_timestamp: None,
            read_only: true,
            auto_commit: false,
            mutation_recorder: None,
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
            auto_commit_group_start: None,
        });
        let edges = reader::scan_all_edges(&snapshot_ctx, space)?;
        let result = index_manager::rebuild_edge_index(
            &self.ctx,
            space,
            index_name,
            &edges,
            graphdb_core::types::SnapshotTimestamp::new(snapshot_timestamp),
            start_lsn,
        );
        if let Some(stats) = self.ctx.stats_manager() {
            if result.is_err() {
                stats.record_generation_rebuild_failure();
            }
        }
        result
    }
}
