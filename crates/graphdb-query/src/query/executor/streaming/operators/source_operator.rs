use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operator_base::OperatorBase;
use crate::storage::cursor::{open_edge_scan, open_vertex_scan, EdgeCursor, ScanOptions, VertexCursor};
use crate::storage::StorageClient;

const CHUNK_SIZE: usize = 1024;

fn make_vertex_row(vertex: crate::core::vertex_edge_path::Vertex) -> Vec<Value> {
    vec![Value::Vertex(Box::new(vertex))]
}

fn make_edge_row(edge: crate::core::vertex_edge_path::Edge) -> Vec<Value> {
    vec![Value::Edge(Box::new(edge))]
}

fn read_vertices(
    storage: &dyn StorageClient,
    space_name: &str,
    vertex_ids: &Option<Vec<Value>>,
) -> Vec<crate::core::vertex_edge_path::Vertex> {
    match vertex_ids {
        Some(ids) if !ids.is_empty() => {
            let mut result = Vec::new();
            for id_val in ids {
                if let Ok(vid) = VertexId::try_from(id_val) {
                    if let Ok(Some(vertex)) = storage.get_vertex(space_name, &vid) {
                        result.push(vertex);
                    }
                }
            }
            result
        }
        _ => storage.scan_vertices(space_name).unwrap_or_default(),
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
        cursor: Option<Box<dyn EdgeCursor>>,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    GetVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
    },
    GetEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
        src: Option<String>,
        dst: Option<String>,
        rank: i64,
    },
    GetNeighbors {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        direction: String,
    },
    EdgeIndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
    },
    IndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: Option<String>,
        index_value: Option<Value>,
    },
    Argument,
    GetProp {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        edge_ids: Option<Vec<Value>>,
        prop_names: Vec<String>,
    },
    LookupIndex {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: String,
        index_condition: Option<(String, Value)>,
        limit: Option<usize>,
    },
    Start,
}

impl SourceOperator {
    pub fn open(&mut self, _base: &mut OperatorBase) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn next(&mut self, _base: &mut OperatorBase) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::ScanVertices {
                current_index,
                buffer,
                col_names,
                ..
            } => {
                if *current_index >= buffer.len() {
                    return Ok(None);
                }
                let end = (*current_index + CHUNK_SIZE).min(buffer.len());
                let chunk_rows: Vec<Vec<Value>> = buffer[*current_index..end].to_vec();
                *current_index = end;
                if chunk_rows.is_empty() {
                    Ok(None)
                } else {
                    let col = if col_names.is_empty() {
                        None
                    } else {
                        Some(col_names.clone())
                    };
                    Ok(Some(DataChunk::from_rows_with_col_names(chunk_rows, col)))
                }
            }
            Self::StorageScanVertices {
                storage,
                space_name,
                limit,
                cursor,
                col_names,
                ..
            } => {
                if cursor.is_none() {
                    *cursor = if let Some(storage) = storage.as_ref() {
                        let opts = ScanOptions {
                            limit: *limit,
                            ..ScanOptions::default()
                        };
                        Some(
                            open_vertex_scan(storage, space_name, &opts)
                                .map_err(|e| QueryError::execution(e.to_string()))?,
                        )
                    } else {
                        return Ok(None);
                    };
                }
                let c = cursor.as_mut().unwrap();
                let batch = c
                    .next_batch(CHUNK_SIZE)
                    .map_err(|e| QueryError::execution(e.to_string()))?;
                if batch.is_empty() {
                    *cursor = None;
                    return Ok(None);
                }
                let chunk_rows: Vec<Vec<Value>> = batch
                    .into_iter()
                    .map(|vertex| vec![Value::Vertex(Box::new(vertex))])
                    .collect();
                let col = if col_names.is_empty() {
                    None
                } else {
                    Some(col_names.clone())
                };
                Ok(Some(DataChunk::from_rows_with_col_names(chunk_rows, col)))
            }
            Self::ScanEdges {
                current_index,
                buffer,
                col_names,
                ..
            } => {
                if *current_index >= buffer.len() {
                    return Ok(None);
                }
                let end = (*current_index + CHUNK_SIZE).min(buffer.len());
                let chunk_rows: Vec<Vec<Value>> = buffer[*current_index..end].to_vec();
                *current_index = end;
                if chunk_rows.is_empty() {
                    Ok(None)
                } else {
                    let col = if col_names.is_empty() {
                        None
                    } else {
                        Some(col_names.clone())
                    };
                    Ok(Some(DataChunk::from_rows_with_col_names(chunk_rows, col)))
                }
            }
            Self::StorageScanEdges {
                storage,
                space_name,
                limit,
                edge_type,
                cursor,
                col_names,
                ..
            } => {
                if cursor.is_none() {
                    *cursor = if let Some(storage) = storage.as_ref() {
                        let opts = ScanOptions {
                            limit: *limit,
                            edge_type: edge_type.clone(),
                            ..ScanOptions::default()
                        };
                        Some(
                            open_edge_scan(storage, space_name, &opts)
                                .map_err(|e| QueryError::execution(e.to_string()))?,
                        )
                    } else {
                        return Ok(None);
                    };
                }
                let c = cursor.as_mut().unwrap();
                let batch = c
                    .next_batch(CHUNK_SIZE)
                    .map_err(|e| QueryError::execution(e.to_string()))?;
                if batch.is_empty() {
                    *cursor = None;
                    return Ok(None);
                }
                let chunk_rows: Vec<Vec<Value>> = batch
                    .into_iter()
                    .map(|edge| vec![Value::Edge(Box::new(edge))])
                    .collect();
                let col = if col_names.is_empty() {
                    None
                } else {
                    Some(col_names.clone())
                };
                Ok(Some(DataChunk::from_rows_with_col_names(chunk_rows, col)))
            }
            Self::GetVertices {
                storage,
                space_name,
                vertex_ids,
                ..
            } => {
                let storage_lock = storage
                    .as_ref()
                    .ok_or_else(|| QueryError::execution("No storage in GetVertices".to_string()))?;
                let storage = storage_lock.read();
                let vertices = read_vertices(&*storage, space_name, vertex_ids);
                let rows: Vec<Vec<Value>> = vertices.into_iter().map(make_vertex_row).collect();
                if rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(rows)))
                }
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
                let storage_lock = storage
                    .as_ref()
                    .ok_or_else(|| QueryError::execution("No storage in GetEdges".to_string()))?;
                let storage = storage_lock.read();
                let edges = if let (Some(src_str), Some(dst_str), Some(et)) =
                    (src.as_deref(), dst.as_deref(), edge_type.as_deref())
                {
                    let src_vid = if let Ok(id) = src_str.parse::<i64>() {
                        VertexId::from_int64(id)
                    } else {
                        VertexId::from_string(src_str.to_string())
                    };
                    let dst_vid = if let Ok(id) = dst_str.parse::<i64>() {
                        VertexId::from_int64(id)
                    } else {
                        VertexId::from_string(dst_str.to_string())
                    };
                    match storage.get_edge(space_name, &src_vid, &dst_vid, et, *rank) {
                        Ok(Some(edge)) => vec![edge],
                        _ => Vec::new(),
                    }
                } else if let Some(et) = &edge_type {
                    storage
                        .scan_edges_by_type(space_name, et)
                        .unwrap_or_default()
                } else {
                    storage.scan_all_edges(space_name).unwrap_or_default()
                };
                let rows: Vec<Vec<Value>> = edges.into_iter().map(make_edge_row).collect();
                if rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(rows)))
                }
            }
            Self::GetNeighbors {
                storage,
                space_name,
                direction,
                ..
            } => {
                let storage_lock = storage
                    .as_ref()
                    .ok_or_else(|| QueryError::execution("No storage in GetNeighbors".to_string()))?;
                let storage = storage_lock.read();
                let dir = match direction.to_lowercase().as_str() {
                    "out" | "outgoing" => crate::core::EdgeDirection::Out,
                    "in" | "incoming" => crate::core::EdgeDirection::In,
                    _ => crate::core::EdgeDirection::Both,
                };
                let vertices = storage.scan_vertices(space_name).unwrap_or_default();
                let mut neighbor_ids = std::collections::HashSet::new();
                for v in &vertices {
                    if let Ok(edges) = storage.get_node_edges(space_name, &v.vid, dir) {
                        for e in &edges {
                            let nid = match dir {
                                crate::core::EdgeDirection::Out => *e.dst(),
                                crate::core::EdgeDirection::In => *e.src(),
                                crate::core::EdgeDirection::Both => {
                                    if e.src() == &v.vid {
                                        *e.dst()
                                    } else {
                                        *e.src()
                                    }
                                }
                            };
                            neighbor_ids.insert(nid);
                        }
                    }
                }
                let mut rows = Vec::new();
                for nid in &neighbor_ids {
                    if let Ok(Some(vertex)) = storage.get_vertex(space_name, nid) {
                        rows.push(make_vertex_row(vertex));
                    }
                }
                if rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(rows)))
                }
            }
            Self::EdgeIndexScan {
                storage,
                space_name,
                edge_type,
                ..
            } => {
                let storage_lock = storage
                    .as_ref()
                    .ok_or_else(|| QueryError::execution("No storage in EdgeIndexScan".to_string()))?;
                let storage = storage_lock.read();
                let edges = if let Some(et) = edge_type {
                    storage
                        .scan_edges_by_type(space_name, et)
                        .unwrap_or_default()
                } else {
                    storage.scan_all_edges(space_name).unwrap_or_default()
                };
                let rows: Vec<Vec<Value>> = edges.into_iter().map(make_edge_row).collect();
                if rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(rows)))
                }
            }
            Self::IndexScan {
                storage,
                space_name,
                index_name,
                index_value,
                ..
            } => {
                let storage_lock = storage
                    .as_ref()
                    .ok_or_else(|| QueryError::execution("No storage in IndexScan".to_string()))?;
                let storage = storage_lock.read();
                let ids = if let (Some(idx_name), Some(val)) = (index_name, index_value) {
                    storage
                        .lookup_index(space_name, idx_name, val)
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let mut rows = Vec::new();
                for id_val in &ids {
                    if let Ok(vid) = VertexId::try_from(id_val) {
                        if let Ok(Some(vertex)) = storage.get_vertex(space_name, &vid) {
                            rows.push(make_vertex_row(vertex));
                        }
                    }
                }
                if rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(rows)))
                }
            }
            Self::Argument => Ok(None),
            Self::GetProp {
                storage,
                space_name,
                vertex_ids,
                edge_ids,
                prop_names,
                ..
            } => {
                let storage_lock = storage
                    .as_ref()
                    .ok_or_else(|| QueryError::execution("No storage in GetProp".to_string()))?;
                let storage = storage_lock.read();
                let mut props = Vec::new();
                if let Some(ref vids) = vertex_ids {
                    for vid_val in vids {
                        if let Ok(vid) = VertexId::try_from(vid_val) {
                            if let Ok(Some(vertex)) = storage.get_vertex(space_name, &vid) {
                                for prop_name in prop_names.iter() {
                                    if let Some(val) = vertex.get_property_any(prop_name) {
                                        props.push(val.clone());
                                    } else {
                                        props.push(Value::Null(crate::core::NullType::Null));
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(ref eids) = edge_ids {
                    for edge_val in eids {
                        if let Value::Edge(edge) = edge_val {
                            for prop_name in prop_names.iter() {
                                if let Some(val) = edge.get_property(prop_name) {
                                    props.push(val.clone());
                                } else {
                                    props.push(Value::Null(crate::core::NullType::Null));
                                }
                            }
                        }
                    }
                }
                let rows: Vec<Vec<Value>> = props.into_iter().map(|v| vec![v]).collect();
                if rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(rows)))
                }
            }
            Self::LookupIndex {
                storage,
                space_name,
                index_name,
                index_condition,
                limit,
                ..
            } => {
                let storage_lock = storage
                    .as_ref()
                    .ok_or_else(|| QueryError::execution("No storage in LookupIndex".to_string()))?;
                let storage = storage_lock.read();
                let mut results: Vec<Value> = if let Some((_field, ref val)) = index_condition {
                    storage
                        .lookup_index(space_name, index_name, val)
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                if let Some(lim) = limit {
                    results.truncate(*lim);
                }
                let rows: Vec<Vec<Value>> = results.into_iter().map(|v| vec![v]).collect();
                if rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(rows)))
                }
            }
            Self::Start => Ok(None),
        }
    }

    pub fn stop(&mut self, _base: &mut OperatorBase) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self, _base: &mut OperatorBase) -> Result<(), QueryError> {
        match self {
            Self::StorageScanVertices { cursor, current_index, .. } => {
                *cursor = None;
                *current_index = 0;
            }
            Self::ScanVertices { current_index, .. } => {
                *current_index = 0;
            }
            Self::StorageScanEdges { cursor, current_index, .. } => {
                *cursor = None;
                *current_index = 0;
            }
            Self::ScanEdges { current_index, .. } => {
                *current_index = 0;
            }
            _ => {}
        }
        Ok(())
    }
}
