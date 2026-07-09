use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::vertex_edge_path::{Edge, Tag, Vertex};
use crate::core::Value;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::executor::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::storage::{StorageClient, StorageWriter};

fn make_modify_result(op: &str, count: u64) -> DataChunk {
    let schema = Arc::new(Schema::new(vec![
        ColumnInfo { name: "operation".to_string(), data_type: "string".to_string() },
        ColumnInfo { name: "rows_affected".to_string(), data_type: "bigint".to_string() },
    ]));
    DataChunk::new(
        vec![vec![
            Value::String(op.to_string()),
            Value::BigInt(count as i64),
        ]],
        schema,
    )
}

// ============ InsertVertices ============

pub fn open_insertvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertVertices { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_insertvertices".to_string())),
    }
}

pub fn next_insertvertices(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::InsertVertices {
            input,
            storage,
            space_name,
            vertex_properties,
            tags,
            rows_inserted,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("InsertVertices not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                if let Some(storage_lock) = storage {
                    let mut writer = storage_lock.write();
                    let col_names = chunk.col_names();

                    for row in &chunk.rows {
                        let mut context = ValueRowContext::new(row.clone(), col_names.clone());

                        let vid = if let Some((_name, expr)) = vertex_properties.first() {
                            match ExpressionEvaluator::evaluate(expr, &mut context) {
                                Ok(val) => VertexId::try_from(&val).unwrap_or_else(|_| VertexId::from_int64(0)),
                                Err(_) => VertexId::from_int64(0),
                            }
                        } else {
                            VertexId::from_int64(0)
                        };

                        let mut props = std::collections::HashMap::new();
                        for (prop_name, expr) in vertex_properties.iter().skip(1) {
                            if let Ok(val) = ExpressionEvaluator::evaluate(expr, &mut context) {
                                props.insert(prop_name.clone(), val);
                            }
                        }

                        let tag_list: Vec<Tag> = tags.iter()
                            .map(|tag_name| Tag::new(tag_name.clone(), props.clone()))
                            .collect();

                        let vertex = Vertex::new_with_properties(vid, tag_list, std::collections::HashMap::new());
                        let _ = StorageWriter::insert_vertex(&mut *writer, space_name, vertex);
                        *rows_inserted += 1;
                    }
                } else {
                    let count = chunk.rows.len() as u64;
                    *rows_inserted += count;
                }
                Ok(Some(chunk))
            } else {
                Ok(Some(make_modify_result("insert_vertices", *rows_inserted)))
            }
        }
        _ => Err(QueryError::execution("Type mismatch in next_insertvertices".to_string())),
    }
}

pub fn stop_insertvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertVertices { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_insertvertices".to_string())),
    }
}

pub fn close_insertvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertVertices { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_insertvertices".to_string())),
    }
}

// ============ InsertEdges ============

pub fn open_insertedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertEdges { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_insertedges".to_string())),
    }
}

pub fn next_insertedges(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::InsertEdges {
            input,
            storage,
            space_name,
            src_col,
            dst_col,
            edge_type,
            edge_properties,
            rows_inserted,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("InsertEdges not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                if let Some(storage_lock) = storage {
                    let mut writer = storage_lock.write();
                    let col_names = chunk.col_names();

                    for row in &chunk.rows {
                        let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                        let src_str = context.get_variable(src_col).unwrap_or(crate::core::Value::Null(crate::core::NullType::Null));
                        let dst_str = context.get_variable(dst_col).unwrap_or(crate::core::Value::Null(crate::core::NullType::Null));

                        if let (Ok(src), Ok(dst)) = (VertexId::try_from(&src_str), VertexId::try_from(&dst_str)) {
                            let mut props = std::collections::HashMap::new();
                            for (prop_name, expr) in edge_properties.iter() {
                                if let Ok(val) = ExpressionEvaluator::evaluate(expr, &mut context) {
                                    props.insert(prop_name.clone(), val);
                                }
                            }
                            let edge = Edge::new(src, dst, edge_type.clone(), 0, props);
                            let _ = StorageWriter::insert_edge(&mut *writer, space_name, edge);
                            *rows_inserted += 1;
                        }
                    }
                } else {
                    let count = chunk.rows.len() as u64;
                    *rows_inserted += count;
                }
                Ok(Some(chunk))
            } else {
                Ok(Some(make_modify_result("insert_edges", *rows_inserted)))
            }
        }
        _ => Err(QueryError::execution("Type mismatch in next_insertedges".to_string())),
    }
}

pub fn stop_insertedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertEdges { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_insertedges".to_string())),
    }
}

pub fn close_insertedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertEdges { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_insertedges".to_string())),
    }
}

// ============ UpdateVertices ============

pub fn open_updatevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateVertices { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_updatevertices".to_string())),
    }
}

pub fn next_updatevertices(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::UpdateVertices {
            input,
            storage,
            space_name,
            updates,
            rows_updated,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("UpdateVertices not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                if let Some(storage_lock) = storage {
                    let mut writer = storage_lock.write();
                    let col_names = chunk.col_names();

                    for row in &chunk.rows {
                        let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                        let vid_val = context.get_variable("vid")
                            .or_else(|| row.first().cloned())
                            .unwrap_or(crate::core::Value::Null(crate::core::NullType::Null));
                        if let Ok(vid) = VertexId::try_from(&vid_val) {
                            let vertex = Vertex::with_vid(vid);
                            let _ = StorageWriter::update_vertex(&mut *writer, space_name, vertex);
                            *rows_updated += 1;
                        }
                    }
                } else {
                    let count = chunk.rows.len() as u64;
                    *rows_updated += count;
                }
                Ok(Some(chunk))
            } else {
                Ok(Some(make_modify_result("update_vertices", *rows_updated)))
            }
        }
        _ => Err(QueryError::execution("Type mismatch in next_updatevertices".to_string())),
    }
}

pub fn stop_updatevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateVertices { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_updatevertices".to_string())),
    }
}

pub fn close_updatevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateVertices { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_updatevertices".to_string())),
    }
}

// ============ UpdateEdges ============

pub fn open_updateedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateEdges { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_updateedges".to_string())),
    }
}

pub fn next_updateedges(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::UpdateEdges {
            input,
            storage,
            space_name,
            rows_updated,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("UpdateEdges not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                if let Some(_storage_lock) = storage {
                    let _writer = _storage_lock.write();
                    // TODO: implement per-row edge update
                }
                let count = chunk.rows.len() as u64;
                *rows_updated += count;
                Ok(Some(chunk))
            } else {
                Ok(Some(make_modify_result("update_edges", *rows_updated)))
            }
        }
        _ => Err(QueryError::execution("Type mismatch in next_updateedges".to_string())),
    }
}

pub fn stop_updateedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateEdges { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_updateedges".to_string())),
    }
}

pub fn close_updateedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateEdges { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_updateedges".to_string())),
    }
}

// ============ DeleteVertices ============

pub fn open_deletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteVertices { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_deletevertices".to_string())),
    }
}

pub fn next_deletevertices(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::DeleteVertices {
            input,
            storage,
            space_name,
            vertex_id_col,
            rows_deleted,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("DeleteVertices not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                if let Some(storage_lock) = storage {
                    let mut writer = storage_lock.write();
                    let col_names = chunk.col_names();

                    for row in &chunk.rows {
                        let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                        if let Some(vid_val) = context.get_variable(vertex_id_col) {
                            if let Ok(vid) = VertexId::try_from(&vid_val) {
                                let _ = StorageWriter::delete_vertex(&mut *writer, space_name, &vid);
                                *rows_deleted += 1;
                            }
                        }
                    }
                } else {
                    let count = chunk.rows.len() as u64;
                    *rows_deleted += count;
                }
                Ok(Some(chunk))
            } else {
                Ok(Some(make_modify_result("delete_vertices", *rows_deleted)))
            }
        }
        _ => Err(QueryError::execution("Type mismatch in next_deletevertices".to_string())),
    }
}

pub fn stop_deletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteVertices { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_deletevertices".to_string())),
    }
}

pub fn close_deletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteVertices { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_deletevertices".to_string())),
    }
}

// ============ DeleteEdges ============

pub fn open_deleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteEdges { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_deleteedges".to_string())),
    }
}

pub fn next_deleteedges(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::DeleteEdges {
            input,
            storage,
            space_name,
            src_col,
            dst_col,
            rows_deleted,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("DeleteEdges not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                if let Some(storage_lock) = storage {
                    let mut writer = storage_lock.write();
                    let col_names = chunk.col_names();

                    for row in &chunk.rows {
                        let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                        let src_val = context.get_variable(src_col).unwrap_or(crate::core::Value::Null(crate::core::NullType::Null));
                        let dst_val = context.get_variable(dst_col).unwrap_or(crate::core::Value::Null(crate::core::NullType::Null));
                        if let (Ok(src), Ok(dst)) = (VertexId::try_from(&src_val), VertexId::try_from(&dst_val)) {
                            use crate::storage::StorageWriter;
                            let _ = StorageWriter::delete_edge(&mut *writer, space_name, &src, &dst, "", 0);
                            *rows_deleted += 1;
                        }
                    }
                } else {
                    let count = chunk.rows.len() as u64;
                    *rows_deleted += count;
                }
                Ok(Some(chunk))
            } else {
                Ok(Some(make_modify_result("delete_edges", *rows_deleted)))
            }
        }
        _ => Err(QueryError::execution("Type mismatch in next_deleteedges".to_string())),
    }
}

pub fn stop_deleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteEdges { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_deleteedges".to_string())),
    }
}

pub fn close_deleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteEdges { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_deleteedges".to_string())),
    }
}

// ============ PipeDeleteVertices ============

pub fn open_pipedeletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteVertices { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_pipedeletevertices".to_string())),
    }
}

pub fn next_pipedeletevertices(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteVertices {
            input,
            storage,
            space_name,
            vertex_id_col,
            rows_deleted,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("PipeDeleteVertices not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                if let Some(storage_lock) = storage {
                    let mut writer = storage_lock.write();
                    let col_names = chunk.col_names();

                    for row in &chunk.rows {
                        let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                        if let Some(vid_val) = context.get_variable(vertex_id_col) {
                            if let Ok(vid) = VertexId::try_from(&vid_val) {
                                let _ = StorageWriter::delete_vertex(&mut *writer, space_name, &vid);
                                *rows_deleted += 1;
                            }
                        }
                    }
                } else {
                    let count = chunk.rows.len() as u64;
                    *rows_deleted += count;
                }
                Ok(Some(chunk))
            } else {
                Ok(Some(make_modify_result("pipe_delete_vertices", *rows_deleted)))
            }
        }
        _ => Err(QueryError::execution("Type mismatch in next_pipedeletevertices".to_string())),
    }
}

pub fn stop_pipedeletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteVertices { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_pipedeletevertices".to_string())),
    }
}

pub fn close_pipedeletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteVertices { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_pipedeletevertices".to_string())),
    }
}

// ============ PipeDeleteEdges ============

pub fn open_pipedeleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteEdges { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_pipedeleteedges".to_string())),
    }
}

pub fn next_pipedeleteedges(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteEdges {
            input,
            storage,
            space_name,
            src_col,
            dst_col,
            rows_deleted,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("PipeDeleteEdges not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                if let Some(storage_lock) = storage {
                    let mut writer = storage_lock.write();
                    let col_names = chunk.col_names();

                    for row in &chunk.rows {
                        let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                        let src_val = context.get_variable(src_col).unwrap_or(crate::core::Value::Null(crate::core::NullType::Null));
                        let dst_val = context.get_variable(dst_col).unwrap_or(crate::core::Value::Null(crate::core::NullType::Null));
                        if let (Ok(src), Ok(dst)) = (VertexId::try_from(&src_val), VertexId::try_from(&dst_val)) {
                            let _ = StorageWriter::delete_edge(&mut *writer, space_name, &src, &dst, "", 0);
                            *rows_deleted += 1;
                        }
                    }
                } else {
                    let count = chunk.rows.len() as u64;
                    *rows_deleted += count;
                }
                Ok(Some(chunk))
            } else {
                Ok(Some(make_modify_result("pipe_delete_edges", *rows_deleted)))
            }
        }
        _ => Err(QueryError::execution("Type mismatch in next_pipedeleteedges".to_string())),
    }
}

pub fn stop_pipedeleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteEdges { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_pipedeleteedges".to_string())),
    }
}

pub fn close_pipedeleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteEdges { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_pipedeleteedges".to_string())),
    }
}

// ============ DeleteTags ============

pub fn open_deletetags(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteTags { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_deletetags".to_string())),
    }
}

pub fn next_deletetags(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::DeleteTags {
            storage,
            space_name,
            tag_names,
            vertex_ids,
            rows_deleted,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("DeleteTags not opened".to_string()));
            }

            // Only emit one result chunk
            if *rows_deleted > 0 {
                return Ok(None);
            }

            if let Some(storage_lock) = storage {
                if let Some(ref ids) = vertex_ids {
                    let mut writer = storage_lock.write();
                    for vertex_id_val in ids {
                        if let Ok(vertex_id) = VertexId::try_from(vertex_id_val) {
                            match StorageWriter::delete_tags(&mut *writer, space_name, &vertex_id, tag_names) {
                                Ok(count) => *rows_deleted += count as u64,
                                Err(_) => {}
                            }
                        }
                    }
                }
            } else {
                let count = vertex_ids.as_ref().map_or(0, |ids| ids.len() * tag_names.len()) as u64;
                *rows_deleted += count;
            }

            Ok(Some(make_modify_result("delete_tags", *rows_deleted)))
        }
        _ => Err(QueryError::execution("Type mismatch in next_deletetags".to_string())),
    }
}

pub fn stop_deletetags(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteTags { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_deletetags".to_string())),
    }
}

pub fn close_deletetags(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteTags { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_deletetags".to_string())),
    }
}
