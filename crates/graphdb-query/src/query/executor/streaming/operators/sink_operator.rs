use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::storage_ids::VertexId;
use crate::core::vertex_edge_path::{Edge, Tag, Vertex};
use crate::core::Value;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operator_base::OperatorBase;
use crate::storage::{StorageClient, StorageWriter};

#[derive(Debug)]
pub enum SinkOperator {
    InsertVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_properties: Vec<(String, Expression)>,
        tags: Vec<String>,
        rows_inserted: u64,
    },
    InsertEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        edge_properties: Vec<(String, Expression)>,
        rows_inserted: u64,
    },
    UpdateVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        updates: Vec<(String, Expression)>,
        rows_updated: u64,
    },
    UpdateEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        updates: Vec<(String, Expression)>,
        rows_updated: u64,
    },
    DeleteVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_id_col: String,
        rows_deleted: u64,
    },
    DeleteEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        rows_deleted: u64,
    },
    PipeDeleteVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_id_col: String,
        rows_deleted: u64,
    },
    PipeDeleteEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        rows_deleted: u64,
    },
    DeleteTags {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        tag_names: Vec<String>,
        vertex_ids: Option<Vec<Value>>,
        rows_deleted: u64,
    },
}

fn make_modify_result(op: &str, count: u64) -> DataChunk {
    let schema = Arc::new(Schema::new(vec![
        ColumnInfo {
            name: "operation".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "rows_affected".to_string(),
            data_type: "bigint".to_string(),
        },
    ]));
    DataChunk::new(
        vec![vec![
            Value::String(op.to_string()),
            Value::BigInt(count as i64),
        ]],
        schema,
    )
}

fn eval_expr(expr: &Expression, context: &mut ValueRowContext) -> Result<Value, QueryError> {
    ExpressionEvaluator::evaluate(expr, context).map_err(|e| QueryError::execution(e.to_string()))
}

impl SinkOperator {
    pub fn from_spec(spec: &super::super::operator_spec::SinkSpec) -> Self {
        match spec {
            super::super::operator_spec::SinkSpec::InsertVertices {
                vertex_properties,
                tags,
            } => Self::InsertVertices {
                storage: None,
                space_name: String::new(),
                vertex_properties: vertex_properties.clone(),
                tags: tags.clone(),
                rows_inserted: 0,
            },
            super::super::operator_spec::SinkSpec::InsertEdges {
                src_col,
                dst_col,
                edge_type,
                edge_properties,
            } => Self::InsertEdges {
                storage: None,
                space_name: String::new(),
                src_col: src_col.clone(),
                dst_col: dst_col.clone(),
                edge_type: edge_type.clone(),
                edge_properties: edge_properties.clone(),
                rows_inserted: 0,
            },
            super::super::operator_spec::SinkSpec::UpdateVertices { updates } => {
                Self::UpdateVertices {
                    storage: None,
                    space_name: String::new(),
                    updates: updates.clone(),
                    rows_updated: 0,
                }
            }
            super::super::operator_spec::SinkSpec::UpdateEdges {
                src_col,
                dst_col,
                edge_type,
                updates,
            } => Self::UpdateEdges {
                storage: None,
                space_name: String::new(),
                src_col: src_col.clone(),
                dst_col: dst_col.clone(),
                edge_type: edge_type.clone(),
                updates: updates.clone(),
                rows_updated: 0,
            },
            super::super::operator_spec::SinkSpec::DeleteVertices { vertex_id_col } => {
                Self::DeleteVertices {
                    storage: None,
                    space_name: String::new(),
                    vertex_id_col: vertex_id_col.clone(),
                    rows_deleted: 0,
                }
            }
            super::super::operator_spec::SinkSpec::DeleteEdges { src_col, dst_col } => {
                Self::DeleteEdges {
                    storage: None,
                    space_name: String::new(),
                    src_col: src_col.clone(),
                    dst_col: dst_col.clone(),
                    rows_deleted: 0,
                }
            }
            super::super::operator_spec::SinkSpec::PipeDeleteVertices { vertex_id_col } => {
                Self::PipeDeleteVertices {
                    storage: None,
                    space_name: String::new(),
                    vertex_id_col: vertex_id_col.clone(),
                    rows_deleted: 0,
                }
            }
            super::super::operator_spec::SinkSpec::PipeDeleteEdges { src_col, dst_col } => {
                Self::PipeDeleteEdges {
                    storage: None,
                    space_name: String::new(),
                    src_col: src_col.clone(),
                    dst_col: dst_col.clone(),
                    rows_deleted: 0,
                }
            }
            super::super::operator_spec::SinkSpec::DeleteTags {
                tag_names,
                vertex_ids,
            } => Self::DeleteTags {
                storage: None,
                space_name: String::new(),
                tag_names: tag_names.clone(),
                vertex_ids: vertex_ids.clone(),
                rows_deleted: 0,
            },
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            SinkOperator::InsertVertices { .. }
            | SinkOperator::InsertEdges { .. }
            | SinkOperator::UpdateVertices { .. }
            | SinkOperator::UpdateEdges { .. }
            | SinkOperator::DeleteVertices { .. }
            | SinkOperator::DeleteEdges { .. }
            | SinkOperator::PipeDeleteVertices { .. }
            | SinkOperator::PipeDeleteEdges { .. }
            | SinkOperator::DeleteTags { .. } => {
                input.open()?;
                base.lifecycle.mark_opened();
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            SinkOperator::InsertVertices {
                storage,
                space_name,
                vertex_properties,
                tags,
                rows_inserted,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution(
                        "InsertVertices not opened".to_string(),
                    ));
                }

                if let Some(chunk) = input.advance()? {
                    base.ensure_not_cancelled()?;
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let col_names = chunk.col_names();

                        for row in &chunk.rows {
                            let mut context = ValueRowContext::new(row.clone(), col_names.clone());

                            let vid = if let Some((_name, expr)) = vertex_properties.first() {
                                let val = eval_expr(expr, &mut context)?;
                                VertexId::try_from(&val).map_err(|e| {
                                    QueryError::execution(format!("Invalid vertex id: {}", e))
                                })?
                            } else {
                                return Err(QueryError::execution(
                                    "InsertVertices requires a vertex id expression".to_string(),
                                ));
                            };

                            let mut props = HashMap::new();
                            for (prop_name, expr) in vertex_properties.iter().skip(1) {
                                let val = eval_expr(expr, &mut context)?;
                                props.insert(prop_name.clone(), val);
                            }

                            let tag_list: Vec<Tag> = tags
                                .iter()
                                .map(|tag_name| Tag::new(tag_name.clone(), props.clone()))
                                .collect();

                            let vertex = Vertex::new_with_properties(vid, tag_list, props);
                            StorageWriter::insert_vertex(&mut *writer, space_name, vertex)
                                .map_err(|e| QueryError::execution(e.to_string()))?;
                            *rows_inserted += 1;
                        }
                    } else {
                        *rows_inserted += chunk.rows.len() as u64;
                    }
                    Ok(Some(chunk))
                } else {
                    Ok(Some(make_modify_result("insert_vertices", *rows_inserted)))
                }
            }

            SinkOperator::InsertEdges {
                storage,
                space_name,
                src_col,
                dst_col,
                edge_type,
                edge_properties,
                rows_inserted,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("InsertEdges not opened".to_string()));
                }

                if let Some(chunk) = input.advance()? {
                    base.ensure_not_cancelled()?;
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let col_names = chunk.col_names();

                        for row in &chunk.rows {
                            let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                            let src_val = context
                                .get_variable(src_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            let dst_val = context
                                .get_variable(dst_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));

                            if let (Ok(src), Ok(dst)) =
                                (VertexId::try_from(&src_val), VertexId::try_from(&dst_val))
                            {
                                let mut props = HashMap::new();
                                for (prop_name, expr) in edge_properties.iter() {
                                    let val = eval_expr(expr, &mut context)?;
                                    props.insert(prop_name.clone(), val);
                                }
                                let edge = Edge::new(src, dst, edge_type.clone(), 0, props);
                                StorageWriter::insert_edge(&mut *writer, space_name, edge)
                                    .map_err(|e| QueryError::execution(e.to_string()))?;
                                *rows_inserted += 1;
                            }
                        }
                    } else {
                        *rows_inserted += chunk.rows.len() as u64;
                    }
                    Ok(Some(chunk))
                } else {
                    Ok(Some(make_modify_result("insert_edges", *rows_inserted)))
                }
            }

            SinkOperator::UpdateVertices {
                storage,
                space_name,
                updates,
                rows_updated,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution(
                        "UpdateVertices not opened".to_string(),
                    ));
                }

                if let Some(chunk) = input.advance()? {
                    base.ensure_not_cancelled()?;
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let col_names = chunk.col_names();

                        for row in &chunk.rows {
                            let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                            let vid_val = context
                                .get_variable("vid")
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            if let Ok(vid) = VertexId::try_from(&vid_val) {
                                let mut props = HashMap::new();
                                for (prop_name, expr) in updates.iter() {
                                    let val = eval_expr(expr, &mut context)?;
                                    props.insert(prop_name.clone(), val);
                                }
                                let vertex = Vertex::new_with_properties(vid, Vec::new(), props);
                                StorageWriter::update_vertex(&mut *writer, space_name, vertex)
                                    .map_err(|e| QueryError::execution(e.to_string()))?;
                                *rows_updated += 1;
                            }
                        }
                    } else {
                        *rows_updated += chunk.rows.len() as u64;
                    }
                    Ok(Some(chunk))
                } else {
                    Ok(Some(make_modify_result("update_vertices", *rows_updated)))
                }
            }

            SinkOperator::UpdateEdges {
                storage,
                space_name,
                src_col,
                dst_col,
                edge_type,
                updates,
                rows_updated,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("UpdateEdges not opened".to_string()));
                }

                if let Some(chunk) = input.advance()? {
                    base.ensure_not_cancelled()?;
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let col_names = chunk.col_names();

                        for row in &chunk.rows {
                            let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                            let src_val = context
                                .get_variable(src_col)
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            let dst_val = context
                                .get_variable(dst_col)
                                .or_else(|| row.get(1).cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));

                            if let (Ok(src), Ok(dst)) =
                                (VertexId::try_from(&src_val), VertexId::try_from(&dst_val))
                            {
                                let mut props = HashMap::new();
                                for (prop_name, expr) in updates.iter() {
                                    if let Ok(val) =
                                        ExpressionEvaluator::evaluate(expr, &mut context)
                                    {
                                        props.insert(prop_name.clone(), val);
                                    }
                                }
                                let mut edge = Edge::new_empty(src, dst, edge_type.clone(), 0);
                                edge.props = props;
                                StorageWriter::update_edge(&mut *writer, space_name, edge)
                                    .map_err(|e| QueryError::execution(e.to_string()))?;
                                *rows_updated += 1;
                            }
                        }
                    } else {
                        *rows_updated += chunk.rows.len() as u64;
                    }
                    Ok(Some(chunk))
                } else {
                    Ok(Some(make_modify_result("update_edges", *rows_updated)))
                }
            }

            SinkOperator::DeleteVertices {
                storage,
                space_name,
                vertex_id_col,
                rows_deleted,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution(
                        "DeleteVertices not opened".to_string(),
                    ));
                }

                if let Some(chunk) = input.advance()? {
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let col_names = chunk.col_names();

                        for row in &chunk.rows {
                            let context = ValueRowContext::new(row.clone(), col_names.clone());
                            if let Some(vid_val) = context.get_variable(vertex_id_col) {
                                if let Ok(vid) = VertexId::try_from(&vid_val) {
                                    StorageWriter::delete_vertex(&mut *writer, space_name, &vid)
                                        .map_err(|e| QueryError::execution(e.to_string()))?;
                                    *rows_deleted += 1;
                                }
                            }
                        }
                    } else {
                        *rows_deleted += chunk.rows.len() as u64;
                    }
                    Ok(Some(chunk))
                } else {
                    Ok(Some(make_modify_result("delete_vertices", *rows_deleted)))
                }
            }

            SinkOperator::DeleteEdges {
                storage,
                space_name,
                src_col,
                dst_col,
                rows_deleted,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("DeleteEdges not opened".to_string()));
                }

                if let Some(chunk) = input.advance()? {
                    base.ensure_not_cancelled()?;
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let col_names = chunk.col_names();

                        for row in &chunk.rows {
                            let context = ValueRowContext::new(row.clone(), col_names.clone());
                            let src_val = context
                                .get_variable(src_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            let dst_val = context
                                .get_variable(dst_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            if let (Ok(src), Ok(dst)) =
                                (VertexId::try_from(&src_val), VertexId::try_from(&dst_val))
                            {
                                StorageWriter::delete_edge(
                                    &mut *writer,
                                    space_name,
                                    &src,
                                    &dst,
                                    "",
                                    0,
                                )
                                .map_err(|e| QueryError::execution(e.to_string()))?;
                                *rows_deleted += 1;
                            }
                        }
                    } else {
                        *rows_deleted += chunk.rows.len() as u64;
                    }
                    Ok(Some(chunk))
                } else {
                    Ok(Some(make_modify_result("delete_edges", *rows_deleted)))
                }
            }

            SinkOperator::PipeDeleteEdges {
                storage,
                space_name,
                src_col,
                dst_col,
                rows_deleted,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution(
                        "PipeDeleteEdges not opened".to_string(),
                    ));
                }

                if let Some(chunk) = input.advance()? {
                    base.ensure_not_cancelled()?;
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let col_names = chunk.col_names();

                        for row in &chunk.rows {
                            let context = ValueRowContext::new(row.clone(), col_names.clone());
                            let src_val = context
                                .get_variable(src_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            let dst_val = context
                                .get_variable(dst_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            if let (Ok(src), Ok(dst)) =
                                (VertexId::try_from(&src_val), VertexId::try_from(&dst_val))
                            {
                                StorageWriter::delete_edge(
                                    &mut *writer,
                                    space_name,
                                    &src,
                                    &dst,
                                    "",
                                    0,
                                )
                                .map_err(|e| QueryError::execution(e.to_string()))?;
                                *rows_deleted += 1;
                            }
                        }
                    } else {
                        *rows_deleted += chunk.rows.len() as u64;
                    }
                    Ok(Some(chunk))
                } else {
                    Ok(Some(make_modify_result("delete_edges", *rows_deleted)))
                }
            }

            SinkOperator::PipeDeleteVertices {
                storage,
                space_name,
                vertex_id_col,
                rows_deleted,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution(
                        "PipeDeleteVertices not opened".to_string(),
                    ));
                }

                if let Some(chunk) = input.advance()? {
                    base.ensure_not_cancelled()?;
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let col_names = chunk.col_names();

                        for row in &chunk.rows {
                            let context = ValueRowContext::new(row.clone(), col_names.clone());
                            if let Some(vid_val) = context.get_variable(vertex_id_col) {
                                if let Ok(vid) = VertexId::try_from(&vid_val) {
                                    StorageWriter::delete_vertex(&mut *writer, space_name, &vid)
                                        .map_err(|e| QueryError::execution(e.to_string()))?;
                                    *rows_deleted += 1;
                                }
                            }
                        }
                    } else {
                        *rows_deleted += chunk.rows.len() as u64;
                    }
                    Ok(Some(chunk))
                } else {
                    Ok(Some(make_modify_result(
                        "pipe_delete_vertices",
                        *rows_deleted,
                    )))
                }
            }

            SinkOperator::DeleteTags {
                storage,
                space_name,
                tag_names,
                vertex_ids,
                rows_deleted,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("DeleteTags not opened".to_string()));
                }

                if *rows_deleted > 0 {
                    return Ok(None);
                }

                base.ensure_not_cancelled()?;
                if let Some(storage_lock) = storage {
                    if let Some(ref ids) = vertex_ids {
                        let mut writer = storage_lock.write();
                        for vertex_id_val in ids {
                            if let Ok(vertex_id) = VertexId::try_from(vertex_id_val) {
                                let count = StorageWriter::delete_tags(
                                    &mut *writer,
                                    space_name,
                                    &vertex_id,
                                    tag_names,
                                )
                                .map_err(|e| QueryError::execution(e.to_string()))?;
                                *rows_deleted += count as u64;
                            }
                        }
                    }
                } else {
                    let count = vertex_ids
                        .as_ref()
                        .map_or(0, |ids| ids.len() * tag_names.len())
                        as u64;
                    *rows_deleted += count;
                }

                Ok(Some(make_modify_result("delete_tags", *rows_deleted)))
            }
        }
    }

    pub fn stop(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            SinkOperator::InsertVertices { .. }
            | SinkOperator::InsertEdges { .. }
            | SinkOperator::UpdateVertices { .. }
            | SinkOperator::UpdateEdges { .. }
            | SinkOperator::DeleteVertices { .. }
            | SinkOperator::DeleteEdges { .. } => {
                if base.lifecycle.can_close() {
                    input.stop()?;
                    base.lifecycle.mark_stopped();
                }
                Ok(())
            }
            SinkOperator::PipeDeleteVertices { .. }
            | SinkOperator::PipeDeleteEdges { .. }
            | SinkOperator::DeleteTags { .. } => {
                input.stop()?;
                Ok(())
            }
        }
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        if base.lifecycle.can_close() {
            input.close()?;
            base.lifecycle.mark_closed();
        }
        Ok(())
    }
}
