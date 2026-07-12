use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operator_base::OperatorBase;
use crate::storage::cursor::{
    open_edge_scan, open_vertex_scan, EdgeCursor, ScanOptions, VertexCursor,
};
use crate::storage::StorageClient;

const CHUNK_SIZE: usize = 1024;

#[derive(Debug, Default)]
pub struct SourceRows {
    rows: VecDeque<Vec<Value>>,
}

impl SourceRows {
    pub fn empty() -> Self {
        Self::default()
    }

    fn replace(&mut self, rows: Vec<Vec<Value>>) {
        self.rows = rows.into();
    }

    fn clear(&mut self) {
        self.rows.clear();
    }

    fn next_chunk(&mut self) -> Option<DataChunk> {
        if self.rows.is_empty() {
            return None;
        }
        let size = self.rows.len().min(CHUNK_SIZE);
        Some(DataChunk::from_rows(self.rows.drain(..size).collect()))
    }
}

fn make_vertex_row(vertex: crate::core::vertex_edge_path::Vertex) -> Vec<Value> {
    vec![Value::Vertex(Box::new(vertex))]
}

fn make_edge_row(edge: crate::core::vertex_edge_path::Edge) -> Vec<Value> {
    vec![Value::Edge(Box::new(edge))]
}

fn storage_error(
    source: &str,
    operation: &str,
    space_name: &str,
    error: impl std::fmt::Display,
) -> QueryError {
    QueryError::execution(format!(
        "{} {} failed for space '{}': {}",
        source, operation, space_name, error
    ))
}

fn read_vertices(
    storage: &dyn StorageClient,
    space_name: &str,
    vertex_ids: &Option<Vec<Value>>,
) -> Result<Vec<crate::core::vertex_edge_path::Vertex>, QueryError> {
    match vertex_ids {
        Some(ids) if !ids.is_empty() => {
            let mut result = Vec::new();
            for id_val in ids {
                if let Ok(vid) = VertexId::try_from(id_val) {
                    if let Some(vertex) = storage.get_vertex(space_name, &vid).map_err(|error| {
                        storage_error("GetVertices", "get vertex", space_name, error)
                    })? {
                        result.push(vertex);
                    }
                }
            }
            Ok(result)
        }
        _ => storage
            .scan_vertices(space_name)
            .map_err(|error| storage_error("GetVertices", "scan vertices", space_name, error)),
    }
}

#[derive(Debug)]
pub enum SourceOperator {
    ScanVertices {
        partition_id: usize,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    StorageScanVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        limit: Option<usize>,
        partition_id: usize,
        partition_range: Option<std::ops::Range<i64>>,
        cursor: Option<Box<dyn VertexCursor>>,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    ScanEdges {
        partition_id: usize,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    StorageScanEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        limit: Option<usize>,
        edge_type: Option<String>,
        partition_id: usize,
        partition_range: Option<std::ops::Range<i64>>,
        cursor: Option<Box<dyn EdgeCursor>>,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    GetVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        rows: SourceRows,
    },
    GetEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
        src: Option<String>,
        dst: Option<String>,
        rank: i64,
        rows: SourceRows,
    },
    GetNeighbors {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        direction: String,
        rows: SourceRows,
    },
    EdgeIndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
        rows: SourceRows,
    },
    IndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: Option<String>,
        index_value: Option<Value>,
        rows: SourceRows,
    },
    Argument,
    GetProp {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        edge_ids: Option<Vec<Value>>,
        prop_names: Vec<String>,
        rows: SourceRows,
    },
    LookupIndex {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: String,
        index_condition: Option<(String, Value)>,
        limit: Option<usize>,
        rows: SourceRows,
    },
    Start,
}

impl SourceOperator {
    pub fn open(&mut self, base: &mut OperatorBase) -> Result<(), QueryError> {
        if self.is_buffered_source() {
            let rows = self.materialize_rows(base)?;
            self.buffered_rows_mut()
                .ok_or_else(|| {
                    QueryError::execution("Buffered source state is missing".to_string())
                })?
                .replace(rows);
        }

        match self {
            Self::ScanVertices { current_index, .. } | Self::ScanEdges { current_index, .. } => {
                *current_index = 0;
            }
            Self::StorageScanVertices {
                storage,
                space_name,
                limit,
                partition_range,
                cursor,
                current_index,
                ..
            } => {
                let storage = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("StorageScanVertices requires storage".to_string())
                })?;
                let options = ScanOptions {
                    limit: *limit,
                    vertex_id_range: partition_range.clone(),
                    ..ScanOptions::default()
                };
                *cursor = Some(open_vertex_scan(storage, space_name, &options).map_err(
                    |error| storage_error("StorageScanVertices", "open cursor", space_name, error),
                )?);
                *current_index = 0;
            }
            Self::StorageScanEdges {
                storage,
                space_name,
                limit,
                edge_type,
                partition_range,
                cursor,
                current_index,
                ..
            } => {
                let storage = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("StorageScanEdges requires storage".to_string())
                })?;
                let options = ScanOptions {
                    limit: *limit,
                    edge_type: edge_type.clone(),
                    edge_src_id_range: partition_range.clone(),
                    ..ScanOptions::default()
                };
                *cursor = Some(
                    open_edge_scan(storage, space_name, &options).map_err(|error| {
                        storage_error("StorageScanEdges", "open cursor", space_name, error)
                    })?,
                );
                *current_index = 0;
            }
            _ => {}
        }

        base.opened = true;
        Ok(())
    }

    pub fn next(&mut self, base: &mut OperatorBase) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::ScanVertices {
                current_index,
                buffer,
                col_names,
                ..
            }
            | Self::ScanEdges {
                current_index,
                buffer,
                col_names,
                ..
            } => Ok(next_buffer_chunk(buffer, current_index, col_names)),
            Self::StorageScanVertices {
                space_name,
                cursor: cursor_state,
                col_names,
                ..
            } => loop {
                base.ensure_not_cancelled()?;
                let Some(cursor) = cursor_state.as_mut() else {
                    return Ok(None);
                };
                let batch = cursor.next_batch(CHUNK_SIZE).map_err(|error| {
                    storage_error("StorageScanVertices", "read cursor", space_name, error)
                })?;
                if batch.is_empty() {
                    *cursor_state = None;
                    return Ok(None);
                }
                let rows = batch.into_iter().map(make_vertex_row).collect::<Vec<_>>();
                if !rows.is_empty() {
                    return Ok(Some(DataChunk::from_rows_with_col_names(
                        rows,
                        optional_col_names(col_names),
                    )));
                }
            },
            Self::StorageScanEdges {
                space_name,
                cursor: cursor_state,
                col_names,
                ..
            } => loop {
                base.ensure_not_cancelled()?;
                let Some(cursor) = cursor_state.as_mut() else {
                    return Ok(None);
                };
                let batch = cursor.next_batch(CHUNK_SIZE).map_err(|error| {
                    storage_error("StorageScanEdges", "read cursor", space_name, error)
                })?;
                if batch.is_empty() {
                    *cursor_state = None;
                    return Ok(None);
                }
                let rows = batch.into_iter().map(make_edge_row).collect::<Vec<_>>();
                if !rows.is_empty() {
                    return Ok(Some(DataChunk::from_rows_with_col_names(
                        rows,
                        optional_col_names(col_names),
                    )));
                }
            },
            Self::GetVertices { rows, .. }
            | Self::GetEdges { rows, .. }
            | Self::GetNeighbors { rows, .. }
            | Self::EdgeIndexScan { rows, .. }
            | Self::IndexScan { rows, .. }
            | Self::GetProp { rows, .. }
            | Self::LookupIndex { rows, .. } => Ok(rows.next_chunk()),
            Self::Argument | Self::Start => Ok(None),
        }
    }

    pub fn stop(&mut self, _base: &mut OperatorBase) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self, base: &mut OperatorBase) -> Result<(), QueryError> {
        match self {
            Self::StorageScanVertices {
                cursor,
                current_index,
                ..
            } => {
                *cursor = None;
                *current_index = 0;
            }
            Self::StorageScanEdges {
                cursor,
                current_index,
                ..
            } => {
                *cursor = None;
                *current_index = 0;
            }
            Self::ScanVertices { current_index, .. } | Self::ScanEdges { current_index, .. } => {
                *current_index = 0;
            }
            Self::GetVertices { rows, .. }
            | Self::GetEdges { rows, .. }
            | Self::GetNeighbors { rows, .. }
            | Self::EdgeIndexScan { rows, .. }
            | Self::IndexScan { rows, .. }
            | Self::GetProp { rows, .. }
            | Self::LookupIndex { rows, .. } => rows.clear(),
            Self::Argument | Self::Start => {}
        }
        base.opened = false;
        Ok(())
    }

    fn is_buffered_source(&self) -> bool {
        matches!(
            self,
            Self::GetVertices { .. }
                | Self::GetEdges { .. }
                | Self::GetNeighbors { .. }
                | Self::EdgeIndexScan { .. }
                | Self::IndexScan { .. }
                | Self::GetProp { .. }
                | Self::LookupIndex { .. }
        )
    }

    fn buffered_rows_mut(&mut self) -> Option<&mut SourceRows> {
        match self {
            Self::GetVertices { rows, .. }
            | Self::GetEdges { rows, .. }
            | Self::GetNeighbors { rows, .. }
            | Self::EdgeIndexScan { rows, .. }
            | Self::IndexScan { rows, .. }
            | Self::GetProp { rows, .. }
            | Self::LookupIndex { rows, .. } => Some(rows),
            _ => None,
        }
    }

    fn materialize_rows(&self, base: &OperatorBase) -> Result<Vec<Vec<Value>>, QueryError> {
        match self {
            Self::GetVertices {
                storage,
                space_name,
                vertex_ids,
                ..
            } => {
                let storage = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("GetVertices requires storage".to_string())
                })?;
                Ok(read_vertices(&*storage.read(), space_name, vertex_ids)?
                    .into_iter()
                    .map(make_vertex_row)
                    .collect())
            }
            Self::GetEdges {
                storage,
                space_name,
                edge_type,
                src,
                dst,
                rank,
                ..
            } => {
                let storage = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("GetEdges requires storage".to_string())
                })?;
                let storage = storage.read();
                let edges = if let (Some(src), Some(dst), Some(edge_type)) =
                    (src.as_deref(), dst.as_deref(), edge_type.as_deref())
                {
                    let src = parse_vertex_id(src);
                    let dst = parse_vertex_id(dst);
                    storage
                        .get_edge(space_name, &src, &dst, edge_type, *rank)
                        .map_err(|error| storage_error("GetEdges", "get edge", space_name, error))?
                        .into_iter()
                        .collect()
                } else if let Some(edge_type) = edge_type {
                    storage
                        .scan_edges_by_type(space_name, edge_type)
                        .map_err(|error| {
                            storage_error("GetEdges", "scan edges by type", space_name, error)
                        })?
                } else {
                    storage.scan_all_edges(space_name).map_err(|error| {
                        storage_error("GetEdges", "scan all edges", space_name, error)
                    })?
                };
                Ok(edges.into_iter().map(make_edge_row).collect())
            }
            Self::GetNeighbors {
                storage,
                space_name,
                direction,
                ..
            } => {
                let storage = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("GetNeighbors requires storage".to_string())
                })?;
                let storage = storage.read();
                let direction = match direction.to_lowercase().as_str() {
                    "out" | "outgoing" => crate::core::EdgeDirection::Out,
                    "in" | "incoming" => crate::core::EdgeDirection::In,
                    _ => crate::core::EdgeDirection::Both,
                };
                let vertices = storage.scan_vertices(space_name).map_err(|error| {
                    storage_error("GetNeighbors", "scan vertices", space_name, error)
                })?;
                let mut seen = HashSet::new();
                let mut neighbor_ids = Vec::new();
                for vertex in &vertices {
                    base.ensure_not_cancelled()?;
                    let edges = storage
                        .get_node_edges(space_name, &vertex.vid, direction)
                        .map_err(|error| {
                            storage_error("GetNeighbors", "get node edges", space_name, error)
                        })?;
                    for edge in edges {
                        let neighbor_id = match direction {
                            crate::core::EdgeDirection::Out => *edge.dst(),
                            crate::core::EdgeDirection::In => *edge.src(),
                            crate::core::EdgeDirection::Both => {
                                if edge.src() == &vertex.vid {
                                    *edge.dst()
                                } else {
                                    *edge.src()
                                }
                            }
                        };
                        if seen.insert(neighbor_id) {
                            neighbor_ids.push(neighbor_id);
                        }
                    }
                }
                let mut rows = Vec::new();
                for neighbor_id in neighbor_ids {
                    base.ensure_not_cancelled()?;
                    if let Some(vertex) =
                        storage
                            .get_vertex(space_name, &neighbor_id)
                            .map_err(|error| {
                                storage_error(
                                    "GetNeighbors",
                                    "get neighbor vertex",
                                    space_name,
                                    error,
                                )
                            })?
                    {
                        rows.push(make_vertex_row(vertex));
                    }
                }
                Ok(rows)
            }
            Self::EdgeIndexScan {
                storage,
                space_name,
                edge_type,
                ..
            } => {
                let storage = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("EdgeIndexScan requires storage".to_string())
                })?;
                let storage = storage.read();
                let edges = if let Some(edge_type) = edge_type {
                    storage
                        .scan_edges_by_type(space_name, edge_type)
                        .map_err(|error| {
                            storage_error("EdgeIndexScan", "scan edges by type", space_name, error)
                        })?
                } else {
                    storage.scan_all_edges(space_name).map_err(|error| {
                        storage_error("EdgeIndexScan", "scan all edges", space_name, error)
                    })?
                };
                Ok(edges.into_iter().map(make_edge_row).collect())
            }
            Self::IndexScan {
                storage,
                space_name,
                index_name,
                index_value,
                ..
            } => {
                let storage = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("IndexScan requires storage".to_string())
                })?;
                let storage = storage.read();
                let ids = match (index_name, index_value) {
                    (Some(index_name), Some(index_value)) => storage
                        .lookup_index(space_name, index_name, index_value)
                        .map_err(|error| {
                            storage_error("IndexScan", "lookup index", space_name, error)
                        })?,
                    _ => Vec::new(),
                };
                let mut rows = Vec::new();
                for id in ids {
                    base.ensure_not_cancelled()?;
                    if let Ok(vertex_id) = VertexId::try_from(&id) {
                        if let Some(vertex) =
                            storage
                                .get_vertex(space_name, &vertex_id)
                                .map_err(|error| {
                                    storage_error("IndexScan", "get vertex", space_name, error)
                                })?
                        {
                            rows.push(make_vertex_row(vertex));
                        }
                    }
                }
                Ok(rows)
            }
            Self::GetProp {
                storage,
                space_name,
                vertex_ids,
                edge_ids,
                prop_names,
                ..
            } => {
                let storage = storage
                    .as_ref()
                    .ok_or_else(|| QueryError::execution("GetProp requires storage".to_string()))?;
                let storage = storage.read();
                let mut rows = Vec::new();
                if let Some(vertex_ids) = vertex_ids {
                    for vertex_id in vertex_ids {
                        base.ensure_not_cancelled()?;
                        if let Ok(vertex_id) = VertexId::try_from(vertex_id) {
                            if let Some(vertex) = storage
                                .get_vertex(space_name, &vertex_id)
                                .map_err(|error| {
                                    storage_error("GetProp", "get vertex", space_name, error)
                                })?
                            {
                                for property_name in prop_names {
                                    rows.push(vec![vertex
                                        .get_property_any(property_name)
                                        .cloned()
                                        .unwrap_or(Value::Null(crate::core::NullType::Null))]);
                                }
                            }
                        }
                    }
                }
                if let Some(edge_ids) = edge_ids {
                    for edge_id in edge_ids {
                        base.ensure_not_cancelled()?;
                        if let Value::Edge(edge) = edge_id {
                            for property_name in prop_names {
                                rows.push(vec![edge
                                    .get_property(property_name)
                                    .cloned()
                                    .unwrap_or(Value::Null(crate::core::NullType::Null))]);
                            }
                        }
                    }
                }
                Ok(rows)
            }
            Self::LookupIndex {
                storage,
                space_name,
                index_name,
                index_condition,
                limit,
                ..
            } => {
                let storage = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("LookupIndex requires storage".to_string())
                })?;
                let storage = storage.read();
                let mut results = match index_condition {
                    Some((_, value)) => storage
                        .lookup_index(space_name, index_name, value)
                        .map_err(|error| {
                            storage_error("LookupIndex", "lookup index", space_name, error)
                        })?,
                    None => Vec::new(),
                };
                if let Some(limit) = limit {
                    results.truncate(*limit);
                }
                Ok(results.into_iter().map(|value| vec![value]).collect())
            }
            _ => Ok(Vec::new()),
        }
    }
}

fn parse_vertex_id(value: &str) -> VertexId {
    value
        .parse::<i64>()
        .map(VertexId::from_int64)
        .unwrap_or_else(|_| VertexId::from_string(value.to_string()))
}

fn optional_col_names(col_names: &[String]) -> Option<Vec<String>> {
    (!col_names.is_empty()).then(|| col_names.to_vec())
}

fn next_buffer_chunk(
    buffer: &[Vec<Value>],
    current_index: &mut usize,
    col_names: &[String],
) -> Option<DataChunk> {
    if *current_index >= buffer.len() {
        return None;
    }
    let end = (*current_index + CHUNK_SIZE).min(buffer.len());
    let rows = buffer[*current_index..end].to_vec();
    *current_index = end;
    Some(DataChunk::from_rows_with_col_names(
        rows,
        optional_col_names(col_names),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_rows_are_emitted_once_in_bounded_chunks() {
        let rows = (0..(CHUNK_SIZE + 1))
            .map(|index| vec![Value::BigInt(index as i64)])
            .collect();
        let mut source_rows = SourceRows::empty();
        source_rows.replace(rows);

        assert_eq!(
            source_rows.next_chunk().map(|chunk| chunk.len()),
            Some(CHUNK_SIZE)
        );
        assert_eq!(source_rows.next_chunk().map(|chunk| chunk.len()), Some(1));
        assert!(source_rows.next_chunk().is_none());
    }

    #[test]
    fn scan_source_terminates_after_consuming_its_buffer() {
        let mut source = SourceOperator::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::BigInt(1)]],
            current_index: 0,
            col_names: Vec::new(),
        };
        let mut base = OperatorBase::new(0);

        source.open(&mut base).expect("source should open");
        assert_eq!(
            source
                .next(&mut base)
                .expect("first pull should succeed")
                .map(|chunk| chunk.len()),
            Some(1)
        );
        assert!(source
            .next(&mut base)
            .expect("second pull should succeed")
            .is_none());
    }

    #[test]
    fn scan_source_splits_across_multiple_chunks() {
        let row_count = CHUNK_SIZE * 2 + 7;
        let buffer: Vec<Vec<Value>> = (0..row_count)
            .map(|i| vec![Value::BigInt(i as i64)])
            .collect();
        let mut source = SourceOperator::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: Vec::new(),
        };
        let mut base = OperatorBase::new(0);
        source.open(&mut base).expect("source should open");

        let chunk1 = source
            .next(&mut base)
            .expect("first pull should succeed")
            .expect("first chunk should be Some");
        assert_eq!(chunk1.len(), CHUNK_SIZE);

        let chunk2 = source
            .next(&mut base)
            .expect("second pull should succeed")
            .expect("second chunk should be Some");
        assert_eq!(chunk2.len(), CHUNK_SIZE);

        let chunk3 = source
            .next(&mut base)
            .expect("third pull should succeed")
            .expect("third chunk should be Some");
        assert_eq!(chunk3.len(), 7);

        assert!(source
            .next(&mut base)
            .expect("fourth pull should succeed")
            .is_none());

        // Verify no data loss: concatenate all chunks
        let total: i64 = chunk1
            .rows
            .iter()
            .chain(chunk2.rows.iter())
            .chain(chunk3.rows.iter())
            .map(|row| match &row[0] {
                Value::BigInt(n) => *n,
                _ => 0,
            })
            .sum();
        let expected: i64 = (0..row_count as i64).sum();
        assert_eq!(total, expected);
    }

    #[test]
    fn buffered_source_open_returns_configuration_errors() {
        let mut source = SourceOperator::GetVertices {
            storage: None,
            space_name: "test".to_string(),
            vertex_ids: None,
            rows: SourceRows::empty(),
        };
        let mut base = OperatorBase::new(0);

        let error = source
            .open(&mut base)
            .expect_err("source without storage must fail to open");
        assert!(error.to_string().contains("GetVertices requires storage"));
        assert!(!base.opened);
    }
}
