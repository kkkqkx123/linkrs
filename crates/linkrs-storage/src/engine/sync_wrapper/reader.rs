use super::SyncWrapper;
use crate::cursor::{EdgeCursor, IndexCursor, IndexRow, IndexScanPlan, ScanOptions, VertexCursor};
use crate::macros::forward_methods;
use crate::{StorageClient, StorageReader};
use linkrs_core::types::{EdgeTypeInfo, TagInfo, VertexId};
use linkrs_core::{Edge, StorageError, Value, Vertex};
use std::sync::Arc;

impl<S: StorageClient + 'static> StorageReader for SyncWrapper<S> {
    forward_methods!(inner;
        fn get_vertex(&self, space: &str, tag: &str, id: &VertexId) -> Result<Option<Vertex>, StorageError>;
        fn layout_version(&self) -> u64;
        fn vertex_id_domain(&self, space: &str) -> Option<std::ops::Range<i64>>;
        fn get_vertex_projected(&self, space: &str, tag: &str, id: &VertexId, projection: &[std::sync::Arc<str>]) -> Result<Option<Vertex>, StorageError>;
        fn scan_vertices(&self, space: &str) -> Result<Vec<Vertex>, StorageError>;
        fn scan_vertices_by_tag(&self, space: &str, tag: &str) -> Result<Vec<Vertex>, StorageError>;
        fn scan_vertices_by_tag_paginated(&self, space: &str, tag: &str, offset: usize, limit: usize) -> Result<Vec<Vertex>, StorageError>;
        fn scan_vertices_by_prop(
            &self,
            space: &str,
            tag: &str,
            prop: &str,
            value: &Value,
        ) -> Result<Vec<Vertex>, StorageError>;
        fn get_edge(
            &self,
            space: &str,
            src: &VertexId,
            dst: &VertexId,
            edge_type: &str,
            rank: i64,
        ) -> Result<Option<Edge>, StorageError>;
        fn get_edge_projected(
            &self,
            space: &str,
            src: &VertexId,
            dst: &VertexId,
            edge_type: &str,
            rank: i64,
            projection: &[Arc<str>],
        ) -> Result<Option<Edge>, StorageError>;
        fn get_node_edges(
            &self,
            space: &str,
            node_id: &VertexId,
            direction: linkrs_core::EdgeDirection,
            edge_types: &[String],
        ) -> Result<Vec<Edge>, StorageError>;
        fn get_node_edges_projected(
            &self,
            space: &str,
            node_id: &VertexId,
            direction: linkrs_core::EdgeDirection,
            edge_types: &[String],
            projection: Option<&[Arc<str>]>,
            limit: Option<usize>,
        ) -> Result<Vec<Edge>, StorageError>;
        fn get_vertices_batch(
            &self,
            space: &str,
            tag: &str,
            ids: &[VertexId],
        ) -> Result<Vec<Option<Vertex>>, StorageError>;
        fn get_vertices_projected_batch(
            &self,
            space: &str,
            tag: &str,
            ids: &[VertexId],
            projection: &[std::sync::Arc<str>],
        ) -> Result<Vec<Option<Vertex>>, StorageError>;
        fn neighbor_dst_ids_batch(
            &self,
            space: &str,
            src_ids: &[VertexId],
            direction: linkrs_core::EdgeDirection,
            edge_types: &[String],
        ) -> Result<Vec<Vec<VertexId>>, StorageError>;
        fn out_degree_batch(
            &self,
            space: &str,
            src_ids: &[VertexId],
            direction: linkrs_core::EdgeDirection,
            edge_types: &[String],
        ) -> Result<Vec<usize>, StorageError>;
        fn scan_edges_by_type(&self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError>;
        fn scan_all_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;
        fn scan_edges_by_type_paginated(&self, space: &str, edge_type: &str, offset: usize, limit: usize) -> Result<Vec<Edge>, StorageError>;
        fn count_vertices_by_tag(&self, space: &str, tag: &str) -> Result<u64, StorageError>;
        fn count_edges_by_type(&self, space: &str, edge_type: &str) -> Result<u64, StorageError>;
        fn enable_edge_property_index(&self, space: &str, edge_type: &str, pool_capacity: u64) -> Result<bool, StorageError>;
        fn has_edge_property_index(&self, space: &str, edge_type: &str) -> Result<bool, StorageError>;
        fn disable_edge_property_index(&self, space: &str, edge_type: &str) -> Result<(), StorageError>;
        fn lookup_edges_by_property_range(&self, space: &str, edge_type: &str, prop_name: &str, lower: Option<&Value>, upper: Option<&Value>, include_lower: bool, include_upper: bool) -> Result<Vec<Edge>, StorageError>;
        fn lookup_index(
            &self,
            space: &str,
            index: &str,
            value: &Value,
        ) -> Result<Vec<Value>, StorageError>;
        fn get_vertex_with_schema(
            &self,
            space: &str,
            tag: &str,
            id: &Value,
        ) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError>;
        fn get_edge_with_schema(
            &self,
            space: &str,
            edge_type: &str,
            src: &Value,
            dst: &Value,
        ) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError>;
        fn scan_vertices_with_schema(
            &self,
            space: &str,
            tag: &str,
        ) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError>;
        fn scan_edges_with_schema(
            &self,
            space: &str,
            edge_type: &str,
        ) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError>;
        fn get_space(
            &self,
            space: &str,
        ) -> Result<Option<linkrs_core::types::SpaceInfo>, StorageError>;
        fn get_space_by_id(
            &self,
            space_id: u64,
        ) -> Result<Option<linkrs_core::types::SpaceInfo>, StorageError>;
        fn list_spaces(&self) -> Result<Vec<linkrs_core::types::SpaceInfo>, StorageError>;
        fn get_space_id(&self, space: &str) -> Result<u64, StorageError>;
        fn space_exists(&self, space: &str) -> bool;
        fn get_tag(
            &self,
            space: &str,
            tag: &str,
        ) -> Result<Option<linkrs_core::types::TagInfo>, StorageError>;
        fn list_tags(&self, space: &str) -> Result<Vec<linkrs_core::types::TagInfo>, StorageError>;
        fn get_edge_type(
            &self,
            space: &str,
            edge: &str,
        ) -> Result<Option<linkrs_core::types::EdgeTypeInfo>, StorageError>;
        fn list_edge_types(
            &self,
            space: &str,
        ) -> Result<Vec<linkrs_core::types::EdgeTypeInfo>, StorageError>;
        fn resolve_edge_type_name(&self, space: &str, hash: u32) -> Result<Option<String>, StorageError>;
        fn get_tag_index(
            &self,
            space: &str,
            index: &str,
        ) -> Result<Option<linkrs_core::types::Index>, StorageError>;
        fn list_tag_indexes(
            &self,
            space: &str,
        ) -> Result<Vec<linkrs_core::types::Index>, StorageError>;
        fn get_edge_index(
            &self,
            space: &str,
            index: &str,
        ) -> Result<Option<linkrs_core::types::Index>, StorageError>;
        fn list_edge_indexes(
            &self,
            space: &str,
        ) -> Result<Vec<linkrs_core::types::Index>, StorageError>;
        fn get_vertex_version_history(
            &self,
            space: &str,
            tag: &str,
        ) -> Result<Option<crate::LabelVersionHistory>, StorageError>;
        fn get_edge_version_history(
            &self,
            space: &str,
            edge_type: &str,
        ) -> Result<Option<crate::LabelVersionHistory>, StorageError>;
        fn get_vertex_schema_changes(
            &self,
            space: &str,
            tag: &str,
            from_version: u64,
            to_version: u64,
        ) -> Result<Vec<crate::PropertyChange>, StorageError>;
        fn get_edge_schema_changes(
            &self,
            space: &str,
            edge_type: &str,
            from_version: u64,
            to_version: u64,
        ) -> Result<Vec<crate::PropertyChange>, StorageError>;
        fn detect_vertex_breaking_changes(
            &self,
            space: &str,
            tag: &str,
            from_version: u64,
            to_version: u64,
        ) -> Result<Vec<crate::PropertyChange>, StorageError>;
        fn detect_edge_breaking_changes(
            &self,
            space: &str,
            edge_type: &str,
            from_version: u64,
            to_version: u64,
        ) -> Result<Vec<crate::PropertyChange>, StorageError>;
        fn create_vertex_cursor(
            &self,
            space: &str,
            options: &ScanOptions,
        ) -> Result<Box<dyn VertexCursor>, StorageError>;
        fn create_edge_cursor(
            &self,
            space: &str,
            options: &ScanOptions,
        ) -> Result<Box<dyn EdgeCursor>, StorageError>;
        fn create_index_cursor(
            &self,
            plan: &IndexScanPlan,
        ) -> Result<Box<dyn IndexCursor<Row = IndexRow>>, StorageError>;
        fn list_migration_history(&self, space: &str, label: &str, is_edge: bool) -> Result<Vec<crate::MigrationHistoryRecord>, StorageError>;
        fn get_applied_versions(&self, space: &str, label: &str, is_edge: bool) -> Result<Vec<u64>, StorageError>;
        fn record_migration_history(&self, record: crate::MigrationHistoryRecord) -> Result<(), StorageError>;
        fn list_all_migration_history(&self) -> Result<Vec<crate::MigrationHistoryRecord>, StorageError>;
    );
}
