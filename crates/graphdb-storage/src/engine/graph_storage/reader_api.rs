use std::sync::Arc;

use graphdb_core::types::{EdgeTypeInfo, Index, SpaceInfo, TagInfo, VertexId};
use graphdb_core::{Edge, EdgeDirection, StorageError, Value, Vertex};

use crate::cursor::{EdgeCursor, IndexCursor, IndexRow, IndexScanPlan, ScanOptions, VertexCursor};
use crate::StorageReader;

use super::GraphStorage;
use super::{cursor_impl, index_manager, reader};

impl StorageReader for GraphStorage {
    fn get_vertex(&self, space: &str, id: &VertexId) -> Result<Option<Vertex>, StorageError> {
        reader::get_vertex(&self.ctx, space, id)
    }

    fn layout_version(&self) -> u64 {
        self.ctx.layout_version()
    }

    fn vertex_id_domain(&self, space: &str) -> Option<std::ops::Range<i64>> {
        self.ctx.vertex_id_domain(space)
    }

    fn get_vertex_projected(
        &self,
        space: &str,
        id: &VertexId,
        projection: &[String],
    ) -> Result<Option<Vertex>, StorageError> {
        reader::get_vertex_projected(&self.ctx, space, id, projection)
    }

    fn scan_vertices(&self, space: &str) -> Result<Vec<Vertex>, StorageError> {
        reader::scan_vertices(&self.ctx, space)
    }

    fn scan_vertices_by_tag(&self, space: &str, tag: &str) -> Result<Vec<Vertex>, StorageError> {
        reader::scan_vertices_by_tag(&self.ctx, space, tag)
    }

    fn scan_vertices_by_tag_paginated(
        &self,
        space: &str,
        tag: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Vertex>, StorageError> {
        let options = crate::cursor::ScanOptions::new()
            .with_tag(tag.to_string())
            .with_offset(offset)
            .with_limit(limit);
        match self.create_vertex_cursor(space, &options) {
            Ok(mut cursor) => cursor.next_batch(limit),
            Err(e) if e.kind() == graphdb_core::error::storage::StorageErrorKind::NotSupported => {
                let all = reader::scan_vertices_by_tag(&self.ctx, space, tag)?;
                Ok(all.into_iter().skip(offset).take(limit).collect())
            }
            Err(e) => Err(e),
        }
    }

    fn scan_vertices_by_prop(
        &self,
        space: &str,
        tag: &str,
        prop: &str,
        value: &Value,
    ) -> Result<Vec<Vertex>, StorageError> {
        reader::scan_vertices_by_prop(&self.ctx, space, tag, prop, value)
    }

    fn get_edge(
        &self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<Option<Edge>, StorageError> {
        reader::get_edge(&self.ctx, space, src, dst, edge_type, rank)
    }

    fn get_edge_projected(
        &self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
        projection: &[String],
    ) -> Result<Option<Edge>, StorageError> {
        reader::get_edge_projected(&self.ctx, space, src, dst, edge_type, rank, projection)
    }

    fn get_node_edges(
        &self,
        space: &str,
        node_id: &VertexId,
        direction: EdgeDirection,
    ) -> Result<Vec<Edge>, StorageError> {
        reader::get_node_edges(&self.ctx, space, node_id, direction)
    }

    fn neighbor_dst_ids_batch(
        &self,
        space: &str,
        src_ids: &[VertexId],
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Result<Vec<Vec<VertexId>>, StorageError> {
        reader::neighbor_dst_ids_batch(&self.ctx, space, src_ids, direction, edge_types)
    }

    fn out_degree_batch(
        &self,
        space: &str,
        src_ids: &[VertexId],
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Result<Vec<usize>, StorageError> {
        reader::out_degree_batch(&self.ctx, space, src_ids, direction, edge_types)
    }

    fn scan_edges_by_type(&self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError> {
        reader::scan_edges_by_type(&self.ctx, space, edge_type)
    }

    fn scan_all_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError> {
        reader::scan_all_edges(&self.ctx, space)
    }

    fn scan_edges_by_type_paginated(
        &self,
        space: &str,
        edge_type: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Edge>, StorageError> {
        reader::scan_edges_by_type_paginated(&self.ctx, space, edge_type, offset, limit)
    }

    fn count_vertices_by_tag(&self, space: &str, tag: &str) -> Result<u64, StorageError> {
        reader::count_vertices_by_tag(&self.ctx, space, tag)
    }

    fn count_edges_by_type(&self, space: &str, edge_type: &str) -> Result<u64, StorageError> {
        reader::count_edges_by_type(&self.ctx, space, edge_type)
    }

    fn lookup_index(
        &self,
        space: &str,
        index_name: &str,
        value: &Value,
    ) -> Result<Vec<Value>, StorageError> {
        index_manager::lookup_index(&self.ctx, space, index_name, value)
    }

    fn enable_edge_property_index(
        &self,
        space: &str,
        edge_type: &str,
        pool_capacity: u64,
    ) -> Result<bool, StorageError> {
        reader::enable_edge_property_index(&self.ctx, space, edge_type, pool_capacity)
    }

    fn has_edge_property_index(&self, space: &str, edge_type: &str) -> Result<bool, StorageError> {
        reader::has_edge_property_index(&self.ctx, space, edge_type)
    }

    fn disable_edge_property_index(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<(), StorageError> {
        reader::disable_edge_property_index(&self.ctx, space, edge_type)
    }

    fn lookup_edges_by_property_range(
        &self,
        space: &str,
        edge_type: &str,
        prop_name: &str,
        lower: Option<&Value>,
        upper: Option<&Value>,
        include_lower: bool,
        include_upper: bool,
    ) -> Result<Vec<Edge>, StorageError> {
        reader::lookup_edges_by_property_range(
            &self.ctx,
            space,
            edge_type,
            prop_name,
            lower,
            upper,
            include_lower,
            include_upper,
        )
    }

    fn get_vertex_with_schema(
        &self,
        space: &str,
        tag: &str,
        id: &Value,
    ) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError> {
        reader::get_vertex_with_schema(&self.ctx, space, tag, id)
    }

    fn get_edge_with_schema(
        &self,
        space: &str,
        edge_type: &str,
        src: &Value,
        dst: &Value,
    ) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError> {
        reader::get_edge_with_schema(&self.ctx, space, edge_type, src, dst)
    }

    fn scan_vertices_with_schema(
        &self,
        space: &str,
        tag: &str,
    ) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError> {
        reader::scan_vertices_with_schema(&self.ctx, space, tag)
    }

    fn scan_edges_with_schema(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError> {
        reader::scan_edges_with_schema(&self.ctx, space, edge_type)
    }

    fn get_space(&self, space: &str) -> Result<Option<SpaceInfo>, StorageError> {
        self.ctx.schema_manager().get_space(space)
    }

    fn get_space_by_id(&self, space_id: u64) -> Result<Option<SpaceInfo>, StorageError> {
        self.ctx.schema_manager().get_space_by_id(space_id)
    }

    fn list_spaces(&self) -> Result<Vec<SpaceInfo>, StorageError> {
        self.ctx.schema_manager().list_spaces()
    }

    fn get_space_id(&self, space: &str) -> Result<u64, StorageError> {
        self.ctx.schema_manager().get_space_id(space)
    }

    fn space_exists(&self, space: &str) -> bool {
        self.ctx
            .schema_manager()
            .get_space(space)
            .ok()
            .flatten()
            .is_some()
    }

    fn get_tag(&self, space: &str, tag: &str) -> Result<Option<TagInfo>, StorageError> {
        self.ctx.schema_manager().get_tag(space, tag)
    }

    fn list_tags(&self, space: &str) -> Result<Vec<TagInfo>, StorageError> {
        self.ctx.schema_manager().list_tags(space)
    }

    fn get_edge_type(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<Option<EdgeTypeInfo>, StorageError> {
        self.ctx.schema_manager().get_edge_type(space, edge_type)
    }

    fn list_edge_types(&self, space: &str) -> Result<Vec<EdgeTypeInfo>, StorageError> {
        self.ctx.schema_manager().list_edge_types(space)
    }

    fn get_tag_index(&self, space: &str, index_name: &str) -> Result<Option<Index>, StorageError> {
        index_manager::get_tag_index(&self.ctx, space, index_name)
    }

    fn list_tag_indexes(&self, space: &str) -> Result<Vec<Index>, StorageError> {
        index_manager::list_tag_indexes(&self.ctx, space)
    }

    fn get_edge_index(&self, space: &str, index_name: &str) -> Result<Option<Index>, StorageError> {
        index_manager::get_edge_index(&self.ctx, space, index_name)
    }

    fn list_edge_indexes(&self, space: &str) -> Result<Vec<Index>, StorageError> {
        index_manager::list_edge_indexes(&self.ctx, space)
    }

    fn get_vertex_version_history(
        &self,
        space: &str,
        tag: &str,
    ) -> Result<Option<crate::LabelVersionHistory>, StorageError> {
        let tag_info = self.ctx.schema_manager().get_tag(space, tag)?;
        let tag_info = tag_info.ok_or_else(|| StorageError::label_not_found(tag.to_string()))?;

        let history = self.ctx.data_store().with_vertex_tables(|vertex_tables| {
            let table = vertex_tables
                .get(&tag_info.tag_id)
                .ok_or_else(|| StorageError::label_not_found(tag.to_string()))?;
            let version_history = table.version_history_ref();
            let guard = version_history
                .lock()
                .map_err(|_| StorageError::db_error("Failed to lock version_history"))?;
            Ok::<_, StorageError>(guard.clone())
        })?;
        Ok(Some(history))
    }

    fn get_edge_version_history(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<Option<crate::LabelVersionHistory>, StorageError> {
        let edge_info = self.ctx.schema_manager().get_edge_type(space, edge_type)?;
        let edge_info =
            edge_info.ok_or_else(|| StorageError::label_not_found(edge_type.to_string()))?;

        let keys = {
            self.ctx
                .data_store()
                .catalog_read_snapshot()
                .with_edge_label_index(|edge_label_index| {
                    edge_label_index
                        .get(&edge_info.edge_type_id)
                        .cloned()
                        .ok_or_else(|| StorageError::label_not_found(edge_type.to_string()))
                })?
        };

        if keys.is_empty() {
            return Err(StorageError::label_not_found(edge_type.to_string()));
        }

        let key = &keys[0];

        let history = self.ctx.data_store().with_single_edge_table(key, |table| {
            let version_history = table.version_history_ref();
            let guard = version_history
                .lock()
                .map_err(|_| StorageError::db_error("Failed to lock version_history"))?;
            Ok::<_, StorageError>(guard.clone())
        })?;
        Ok(Some(history))
    }

    fn get_vertex_schema_changes(
        &self,
        space: &str,
        tag: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<crate::PropertyChange>, StorageError> {
        let history = self.get_vertex_version_history(space, tag)?;
        let history = history.ok_or_else(|| StorageError::label_not_found(tag.to_string()))?;

        let mut changes = Vec::new();
        for version in history.get_versions() {
            if version > from_version && version <= to_version {
                if let Some(version_changes) = history.change_log.get_version_changes(version) {
                    changes.extend(version_changes.iter().cloned());
                }
            }
        }

        Ok(changes)
    }

    fn get_edge_schema_changes(
        &self,
        space: &str,
        edge_type: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<crate::PropertyChange>, StorageError> {
        let history = self.get_edge_version_history(space, edge_type)?;
        let history =
            history.ok_or_else(|| StorageError::label_not_found(edge_type.to_string()))?;

        let mut changes = Vec::new();
        for version in history.get_versions() {
            if version > from_version && version <= to_version {
                if let Some(version_changes) = history.change_log.get_version_changes(version) {
                    changes.extend(version_changes.iter().cloned());
                }
            }
        }

        Ok(changes)
    }

    fn detect_vertex_breaking_changes(
        &self,
        space: &str,
        tag: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<crate::PropertyChange>, StorageError> {
        let changes = self.get_vertex_schema_changes(space, tag, from_version, to_version)?;
        Ok(changes.into_iter().filter(|c| c.is_breaking()).collect())
    }

    fn detect_edge_breaking_changes(
        &self,
        space: &str,
        edge_type: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<crate::PropertyChange>, StorageError> {
        let changes = self.get_edge_schema_changes(space, edge_type, from_version, to_version)?;
        Ok(changes.into_iter().filter(|c| c.is_breaking()).collect())
    }

    fn create_vertex_cursor(
        &self,
        space: &str,
        options: &ScanOptions,
    ) -> Result<Box<dyn VertexCursor>, StorageError> {
        let cursor =
            cursor_impl::GraphVertexCursor::new(self.ctx.clone(), space.to_string(), options)?;
        Ok(Box::new(cursor))
    }

    fn create_edge_cursor(
        &self,
        space: &str,
        options: &ScanOptions,
    ) -> Result<Box<dyn EdgeCursor>, StorageError> {
        cursor_impl::create_edge_cursor(self.ctx.clone(), space, options)
    }

    fn create_index_cursor(
        &self,
        plan: &IndexScanPlan,
    ) -> Result<Box<dyn IndexCursor<Row = IndexRow>>, StorageError> {
        let tag_indexes = self.list_tag_indexes(&plan.space)?;
        let edge_indexes = self.list_edge_indexes(&plan.space)?;
        let index_id = plan.index_id;
        if let Some(index) = tag_indexes.into_iter().find(|index| index.id == index_id) {
            let space_id = self.ctx.schema_manager().get_space_id(&plan.space)?;
            let space_name = plan.space.clone();
            let ctx = self.ctx.clone();
            let stale_checker: Option<crate::index::types::StaleChecker> = Some(Arc::new(
                move |entity_ref, _entity_version| match entity_ref {
                    graphdb_core::wal::EntityRef::Vertex(vid) => {
                        reader::get_vertex(&ctx, &space_name, vid)
                            .ok()
                            .flatten()
                            .is_some()
                    }
                    graphdb_core::wal::EntityRef::Edge { .. } => true,
                },
            ));
            let cursor = self
                .ctx
                .index_data_manager()
                .read()
                .open_tag_index_cursor_full(space_id, &index, plan, stale_checker, None)?;
            return Ok(Box::new(cursor));
        }

        if let Some(index) = edge_indexes.into_iter().find(|index| index.id == index_id) {
            let space_id = self.ctx.schema_manager().get_space_id(&plan.space)?;
            let space_name = plan.space.clone();
            let ctx = self.ctx.clone();
            let stale_checker: Option<crate::index::types::StaleChecker> = Some(Arc::new(
                move |entity_ref, _entity_version| match entity_ref {
                    graphdb_core::wal::EntityRef::Vertex(vid) => {
                        reader::get_vertex(&ctx, &space_name, vid)
                            .ok()
                            .flatten()
                            .is_some()
                    }
                    graphdb_core::wal::EntityRef::Edge { .. } => true,
                },
            ));
            let cursor = self
                .ctx
                .index_data_manager()
                .read()
                .open_edge_index_cursor_full(space_id, &index, plan, stale_checker, None)?;
            return Ok(Box::new(cursor));
        }

        Err(StorageError::not_found(format!(
            "Index {} not found in space {}",
            plan.index_id, plan.space
        )))
    }

    fn list_migration_history(
        &self,
        space: &str,
        label: &str,
        is_edge: bool,
    ) -> Result<Vec<crate::MigrationHistoryRecord>, StorageError> {
        Ok(self.ctx.list_migration_history(space, label, is_edge))
    }

    fn get_applied_versions(
        &self,
        space: &str,
        label: &str,
        is_edge: bool,
    ) -> Result<Vec<u64>, StorageError> {
        Ok(self.ctx.get_applied_versions(space, label, is_edge))
    }

    fn record_migration_history(
        &self,
        record: crate::MigrationHistoryRecord,
    ) -> Result<(), StorageError> {
        self.ctx.record_migration_history(record)
    }

    fn list_all_migration_history(
        &self,
    ) -> Result<Vec<crate::MigrationHistoryRecord>, StorageError> {
        Ok(self.ctx.list_all_migration_history())
    }
}
