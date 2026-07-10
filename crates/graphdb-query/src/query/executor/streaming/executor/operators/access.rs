use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::storage::StorageClient;

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

// ============ Start Operator ============

pub fn open_start(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

pub fn next_start(_executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    Ok(None)
}

pub fn stop_start(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

pub fn close_start(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

// ============ GetVertices Operator ============

pub fn open_getvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetVertices { opened, .. } => {
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_getvertices".to_string(),
        )),
    }
}

pub fn next_getvertices(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::GetVertices {
            storage,
            space_name,
            vertex_ids,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("GetVertices not opened".to_string()));
            }

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
        _ => Err(QueryError::execution(
            "Type mismatch in next_getvertices".to_string(),
        )),
    }
}

pub fn stop_getvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetVertices { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_getvertices".to_string(),
        )),
    }
}

pub fn close_getvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetVertices { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_getvertices".to_string(),
        )),
    }
}

// ============ GetEdges Operator ============

pub fn open_getedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetEdges { opened, .. } => {
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_getedges".to_string(),
        )),
    }
}

pub fn next_getedges(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::GetEdges {
            storage,
            space_name,
            edge_type,
            src,
            dst,
            rank,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("GetEdges not opened".to_string()));
            }

            let storage_lock = storage
                .as_ref()
                .ok_or_else(|| QueryError::execution("No storage in GetEdges".to_string()))?;
            let storage = storage_lock.read();

            let et = edge_type.as_deref();
            let edges = if let (Some(src_str), Some(dst_str), Some(et)) =
                (src.as_deref(), dst.as_deref(), et)
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
        _ => Err(QueryError::execution(
            "Type mismatch in next_getedges".to_string(),
        )),
    }
}

pub fn stop_getedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetEdges { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_getedges".to_string(),
        )),
    }
}

pub fn close_getedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetEdges { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_getedges".to_string(),
        )),
    }
}

// ============ GetNeighbors Operator ============

pub fn open_getneighbors(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetNeighbors { opened, .. } => {
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_getneighbors".to_string(),
        )),
    }
}

pub fn next_getneighbors(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::GetNeighbors {
            storage,
            space_name,
            direction,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("GetNeighbors not opened".to_string()));
            }

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
                            crate::core::EdgeDirection::Out => e.dst().clone(),
                            crate::core::EdgeDirection::In => e.src().clone(),
                            crate::core::EdgeDirection::Both => {
                                if e.src() == &v.vid {
                                    e.dst().clone()
                                } else {
                                    e.src().clone()
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
        _ => Err(QueryError::execution(
            "Type mismatch in next_getneighbors".to_string(),
        )),
    }
}

pub fn stop_getneighbors(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetNeighbors { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_getneighbors".to_string(),
        )),
    }
}

pub fn close_getneighbors(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetNeighbors { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_getneighbors".to_string(),
        )),
    }
}

// ============ IndexScan Operator ============

pub fn open_indexscan(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::IndexScan { opened, .. } => {
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_indexscan".to_string(),
        )),
    }
}

pub fn next_indexscan(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::IndexScan {
            storage,
            space_name,
            index_name,
            index_value,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("IndexScan not opened".to_string()));
            }

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
        _ => Err(QueryError::execution(
            "Type mismatch in next_indexscan".to_string(),
        )),
    }
}

pub fn stop_indexscan(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::IndexScan { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_indexscan".to_string(),
        )),
    }
}

pub fn close_indexscan(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::IndexScan { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_indexscan".to_string(),
        )),
    }
}

// ============ EdgeIndexScan Operator ============

pub fn open_edgeindexscan(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::EdgeIndexScan { opened, .. } => {
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_edgeindexscan".to_string(),
        )),
    }
}

pub fn next_edgeindexscan(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::EdgeIndexScan {
            storage,
            space_name,
            edge_type,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution(
                    "EdgeIndexScan not opened".to_string(),
                ));
            }

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
        _ => Err(QueryError::execution(
            "Type mismatch in next_edgeindexscan".to_string(),
        )),
    }
}

pub fn stop_edgeindexscan(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::EdgeIndexScan { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_edgeindexscan".to_string(),
        )),
    }
}

pub fn close_edgeindexscan(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::EdgeIndexScan { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_edgeindexscan".to_string(),
        )),
    }
}

// ============ Argument Operator ============

pub fn open_argument(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

pub fn next_argument(_executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    Ok(None)
}

pub fn stop_argument(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

pub fn close_argument(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

// ============ Sample Operator ============

pub fn open_sample(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

pub fn next_sample(_executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    Ok(None)
}

pub fn stop_sample(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

pub fn close_sample(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

// ============ GetProp Operator ============

pub fn open_getprop(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetProp { opened, .. } => {
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_getprop".to_string(),
        )),
    }
}

pub fn next_getprop(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::GetProp {
            storage,
            space_name,
            vertex_ids,
            edge_ids,
            prop_names,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("GetProp not opened".to_string()));
            }

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
        _ => Err(QueryError::execution(
            "Type mismatch in next_getprop".to_string(),
        )),
    }
}

pub fn stop_getprop(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetProp { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_getprop".to_string(),
        )),
    }
}

pub fn close_getprop(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GetProp { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_getprop".to_string(),
        )),
    }
}

// ============ LookupIndex Operator ============

pub fn open_lookupindex(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::LookupIndex { opened, .. } => {
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_lookupindex".to_string(),
        )),
    }
}

pub fn next_lookupindex(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::LookupIndex {
            storage,
            space_name,
            index_name,
            index_condition,
            limit,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("LookupIndex not opened".to_string()));
            }

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
        _ => Err(QueryError::execution(
            "Type mismatch in next_lookupindex".to_string(),
        )),
    }
}

pub fn stop_lookupindex(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::LookupIndex { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_lookupindex".to_string(),
        )),
    }
}

pub fn close_lookupindex(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::LookupIndex { opened, .. } => {
            *opened = false;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_lookupindex".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_operator() {
        let mut executor = StreamingExecutor::Start { opened: false };
        assert!(executor.open().is_ok());
        assert!(executor.next().unwrap().is_none());
        assert!(executor.close().is_ok());
    }

    #[test]
    fn test_argument_operator() {
        let mut executor = StreamingExecutor::Argument { opened: false };
        assert!(executor.open().is_ok());
        assert!(executor.next().unwrap().is_none());
        assert!(executor.close().is_ok());
    }

    #[test]
    fn test_getvertices_no_storage() {
        let mut executor = StreamingExecutor::GetVertices {
            storage: None,
            space_name: "default".to_string(),
            vertex_ids: None,
            opened: false,
        };
        assert!(executor.open().is_ok());
        let result = executor.next();
        assert!(result.is_err());
    }
}
