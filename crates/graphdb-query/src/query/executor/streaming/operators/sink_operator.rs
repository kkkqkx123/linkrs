use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::storage_ids::VertexId;
use crate::core::vertex_edge_path::{Edge, Tag, Vertex};
use crate::core::Value;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::source_operator::OperatorConfig;
use crate::query::executor::streaming::runtime::ExecutionRuntime;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::storage::{QueryStorage, StorageWriter};

#[derive(Debug)]
pub enum SinkOperatorKind {
    CopyFrom {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        target: crate::query::executor::streaming::operators::spec::CopyTarget,
        file_path: String,
        header: bool,
        delimiter: u8,
        batch_size: usize,
        rows_inserted: u64,
        summary_returned: bool,
    },
    InsertVertices {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        vertex_properties: Vec<(String, Expression)>,
        tags: Vec<String>,
        tag_property_names: Vec<Vec<String>>,
        if_not_exists: bool,
        rows_inserted: u64,
        summary_returned: bool,
    },
    InsertEdges {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        edge_properties: Vec<(String, Expression)>,
        if_not_exists: bool,
        rows_inserted: u64,
        summary_returned: bool,
    },
    UpdateVertices {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        tag_name: String,
        updates: Vec<(String, Expression)>,
        condition: Option<Expression>,
        is_upsert: bool,
        rows_updated: u64,
        summary_returned: bool,
    },
    UpdateEdges {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        updates: Vec<(String, Expression)>,
        condition: Option<Expression>,
        is_upsert: bool,
        rows_updated: u64,
        summary_returned: bool,
    },
    DeleteVertices {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        vertex_id_col: String,
        rows_deleted: u64,
        summary_returned: bool,
    },
    DeleteEdges {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        rows_deleted: u64,
        summary_returned: bool,
    },
    PipeDeleteVertices {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        vertex_id_col: String,
        rows_deleted: u64,
        summary_returned: bool,
    },
    PipeDeleteEdges {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        rows_deleted: u64,
        summary_returned: bool,
    },
    DeleteTags {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        tag_names: Vec<String>,
        vertex_ids: Option<Vec<Value>>,
        rows_deleted: u64,
        summary_returned: bool,
    },
}

/// Sink operator.
///
/// Wraps [`SinkOperatorKind`] with the runtime context injected at `open()`.
/// Lifecycle state is owned exclusively by the executor; operators never
/// write it.
#[derive(Debug)]
pub struct SinkOperator {
    pub kind: SinkOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

fn make_modify_result(output_layout: Arc<SlotLayout>, op: &str, count: u64) -> DataChunk {
    let row = vec![Value::string(op), Value::BigInt(count as i64)];
    DataChunk::new_with_layout(vec![row], output_layout)
}

fn eval_expr(expr: &Expression, context: &mut ValueRowContext) -> Result<Value, QueryError> {
    ExpressionEvaluator::evaluate(expr, context).map_err(|e| QueryError::execution(e.to_string()))
}

/// Build a row context that resolves `$name` parameter references.
///
/// Sink operators evaluate shape-normalized DML expressions (`$__dml_N`
/// placeholders), so the context must carry the runtime parameter values —
/// otherwise parameter resolution fails with "Undefined parameter".
fn row_context(
    row: Vec<Value>,
    layout: Arc<SlotLayout>,
    params: Option<Arc<HashMap<String, Value>>>,
) -> ValueRowContext {
    match params {
        Some(parameters) => ValueRowContext::with_parameters(row, layout, parameters),
        None => ValueRowContext::new(row, layout),
    }
}

/// Row predicate semantics for update conditions (`WHEN`/`WHERE`), matching
/// the filter operator: false/null/zero/empty reject the row.
fn condition_matches(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Null(_) => false,
        Value::Int(i) => *i != 0,
        Value::SmallInt(i) => *i != 0,
        Value::BigInt(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::Double(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// Resolve edge endpoints from row values.
///
/// When the row carries an `Edge` value (e.g. `MATCH ... DELETE EDGE e`), the
/// endpoints are taken from the edge itself; otherwise both values are
/// converted to vertex ids directly.
fn resolve_edge_endpoints(src_val: &Value, dst_val: &Value) -> Option<(VertexId, VertexId)> {
    match (src_val, dst_val) {
        (Value::Edge(edge), _) => Some((edge.src, edge.dst)),
        (_, Value::Edge(edge)) => Some((edge.src, edge.dst)),
        _ => {
            let src = VertexId::try_from(src_val).ok()?;
            let dst = VertexId::try_from(dst_val).ok()?;
            Some((src, dst))
        }
    }
}

impl SinkOperator {
    pub fn from_spec(
        spec: &super::spec::SinkSpec,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let kind = match spec {
            super::spec::SinkSpec::CopyFrom {
                space_name,
                target,
                file_path,
                header,
                delimiter,
                batch_size,
            } => SinkOperatorKind::CopyFrom {
                storage,
                space_name: space_name.clone(),
                target: target.clone(),
                file_path: file_path.clone(),
                header: *header,
                delimiter: *delimiter,
                batch_size: *batch_size,
                rows_inserted: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::InsertVertices {
                space_name,
                vertex_properties,
                tags,
                tag_property_names,
                if_not_exists,
            } => SinkOperatorKind::InsertVertices {
                storage,
                space_name: space_name.clone(),
                vertex_properties: vertex_properties.clone(),
                tags: tags.clone(),
                tag_property_names: tag_property_names.clone(),
                if_not_exists: *if_not_exists,
                rows_inserted: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::InsertEdges {
                space_name,
                src_col,
                dst_col,
                edge_type,
                edge_properties,
                if_not_exists,
            } => SinkOperatorKind::InsertEdges {
                storage,
                space_name: space_name.clone(),
                src_col: src_col.clone(),
                dst_col: dst_col.clone(),
                edge_type: edge_type.clone(),
                edge_properties: edge_properties.clone(),
                if_not_exists: *if_not_exists,
                rows_inserted: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::UpdateVertices {
                space_name,
                tag_name,
                updates,
                condition,
                is_upsert,
            } => SinkOperatorKind::UpdateVertices {
                storage,
                space_name: space_name.clone(),
                tag_name: tag_name.clone(),
                updates: updates.clone(),
                condition: condition.clone(),
                is_upsert: *is_upsert,
                rows_updated: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::UpdateEdges {
                space_name,
                src_col,
                dst_col,
                edge_type,
                updates,
                condition,
                is_upsert,
            } => SinkOperatorKind::UpdateEdges {
                storage,
                space_name: space_name.clone(),
                src_col: src_col.clone(),
                dst_col: dst_col.clone(),
                edge_type: edge_type.clone(),
                updates: updates.clone(),
                condition: condition.clone(),
                is_upsert: *is_upsert,
                rows_updated: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::DeleteVertices {
                space_name,
                vertex_id_col,
            } => SinkOperatorKind::DeleteVertices {
                storage,
                space_name: space_name.clone(),
                vertex_id_col: vertex_id_col.clone(),
                rows_deleted: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::DeleteEdges {
                space_name,
                src_col,
                dst_col,
                edge_type,
            } => SinkOperatorKind::DeleteEdges {
                storage,
                space_name: space_name.clone(),
                src_col: src_col.clone(),
                dst_col: dst_col.clone(),
                edge_type: edge_type.clone(),
                rows_deleted: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::PipeDeleteVertices {
                space_name,
                vertex_id_col,
            } => SinkOperatorKind::PipeDeleteVertices {
                storage,
                space_name: space_name.clone(),
                vertex_id_col: vertex_id_col.clone(),
                rows_deleted: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::PipeDeleteEdges {
                space_name,
                src_col,
                dst_col,
                edge_type,
            } => SinkOperatorKind::PipeDeleteEdges {
                storage,
                space_name: space_name.clone(),
                src_col: src_col.clone(),
                dst_col: dst_col.clone(),
                edge_type: edge_type.clone(),
                rows_deleted: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::DeleteTags {
                space_name,
                tag_names,
                vertex_ids,
            } => SinkOperatorKind::DeleteTags {
                storage,
                space_name: space_name.clone(),
                tag_names: tag_names.clone(),
                vertex_ids: vertex_ids.clone(),
                rows_deleted: 0,
                summary_returned: false,
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn new(kind: SinkOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            kind,
            runtime: None,
            output_layout,
            config: OperatorConfig::default(),
        }
    }

    /// Inject the runtime and execution config (called once by the executor
    /// before this operator produces any data).
    pub fn inject_context(
        &mut self,
        runtime: Option<&Arc<ExecutionRuntime>>,
        config: OperatorConfig,
    ) {
        if let Some(rt) = runtime {
            self.runtime = Some(rt.clone());
        }
        self.config = config;
    }

    /// Check that the transaction scope allows writes.
    ///
    /// M0.4: requires a transaction scope for DML operations.  Absent scope
    /// is rejected to prevent unbounded writes outside any transaction.
    fn check_write_permission(&self) -> Result<(), QueryError> {
        let rt = self.runtime.as_ref().ok_or_else(|| {
            QueryError::execution(
                "DML requires an execution runtime with transaction scope".to_string(),
            )
        })?;
        let scope = rt.transaction_scope().ok_or_else(|| {
            QueryError::execution(
                "DML requires a transaction scope — no transaction is active".to_string(),
            )
        })?;
        if !scope.allows_write() {
            return Err(QueryError::execution(
                "Write operation not allowed in current transaction scope".to_string(),
            ));
        }
        Ok(())
    }

    pub fn open(&mut self, input: &mut StreamingExecutor) -> Result<(), QueryError> {
        self.check_write_permission()?;
        match &mut self.kind {
            SinkOperatorKind::CopyFrom { .. }
            | SinkOperatorKind::InsertVertices { .. }
            | SinkOperatorKind::InsertEdges { .. }
            | SinkOperatorKind::UpdateVertices { .. }
            | SinkOperatorKind::UpdateEdges { .. }
            | SinkOperatorKind::DeleteVertices { .. }
            | SinkOperatorKind::DeleteEdges { .. }
            | SinkOperatorKind::PipeDeleteVertices { .. }
            | SinkOperatorKind::PipeDeleteEdges { .. }
            | SinkOperatorKind::DeleteTags { .. } => {
                input.open()?;
                Ok(())
            }
        }
    }

    pub fn next(&mut self, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        match &mut self.kind {
            SinkOperatorKind::InsertVertices {
                storage,
                space_name,
                vertex_properties,
                tags,
                tag_property_names,
                if_not_exists,
                rows_inserted,
                summary_returned,
                ..
            } => {
                if *summary_returned {
                    return Ok(None);
                }

                while let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Sink");
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let layout = chunk.get_layout();
                        let params = self.runtime.as_ref().and_then(|rt| rt.parameter_values());

                        for row in &chunk.rows {
                            let mut context =
                                row_context(row.clone(), layout.clone(), params.clone());

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

                            if *if_not_exists
                                && writer
                                    .get_vertex(space_name, &vid)
                                    .map_err(|e| QueryError::execution(e.to_string()))?
                                    .is_some()
                            {
                                continue;
                            }

                            let tag_list: Vec<Tag> = tags
                                .iter()
                                .zip(tag_property_names.iter())
                                .map(|(tag_name, prop_names)| {
                                    let mut props = HashMap::new();
                                    for name in prop_names {
                                        if let Some((_n, expr)) =
                                            vertex_properties.iter().find(|(n, _)| n == name)
                                        {
                                            if let Ok(val) = eval_expr(expr, &mut context) {
                                                props.insert(name.clone(), val);
                                            }
                                        }
                                    }
                                    Tag::new(tag_name.clone(), props)
                                })
                                .collect();

                            let vertex = Vertex::new_with_properties(vid, tag_list, HashMap::new());
                            StorageWriter::insert_vertex(&mut *writer, space_name, vertex)
                                .map_err(|e| QueryError::execution(e.to_string()))?;
                            *rows_inserted += 1;
                        }
                    } else {
                        *rows_inserted += chunk.rows.len() as u64;
                    }
                }

                *summary_returned = true;
                Ok(Some(make_modify_result(
                    Arc::clone(&self.output_layout),
                    "insert_vertices",
                    *rows_inserted,
                )))
            }

            SinkOperatorKind::InsertEdges {
                storage,
                space_name,
                src_col,
                dst_col,
                edge_type,
                edge_properties,
                if_not_exists,
                rows_inserted,
                summary_returned,
                ..
            } => {
                if *summary_returned {
                    return Ok(None);
                }

                while let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Sink");
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let layout = chunk.get_layout();
                        let params = self.runtime.as_ref().and_then(|rt| rt.parameter_values());

                        for row in &chunk.rows {
                            let mut context =
                                row_context(row.clone(), layout.clone(), params.clone());
                            let src_val = context
                                .get_variable(src_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            let dst_val = context
                                .get_variable(dst_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));

                            if let (Ok(src), Ok(dst)) =
                                (VertexId::try_from(&src_val), VertexId::try_from(&dst_val))
                            {
                                // Multi-edge semantics: the storage layer
                                // assigns an increasing rank when a
                                // (src, dst, edge_type) pair already exists,
                                // so plain INSERT always succeeds. The
                                // if-not-exists guard only skips duplicates.
                                if *if_not_exists
                                    && writer
                                        .get_edge(space_name, &src, &dst, edge_type, 0)
                                        .map_err(|e| QueryError::execution(e.to_string()))?
                                        .is_some()
                                {
                                    continue;
                                }
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
                }

                *summary_returned = true;
                Ok(Some(make_modify_result(
                    Arc::clone(&self.output_layout),
                    "insert_edges",
                    *rows_inserted,
                )))
            }

            SinkOperatorKind::UpdateVertices {
                storage,
                space_name,
                tag_name,
                updates,
                condition,
                is_upsert,
                rows_updated,
                summary_returned,
                ..
            } => {
                if *summary_returned {
                    return Ok(None);
                }

                while let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Sink");
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let layout = chunk.get_layout();
                        let params = self.runtime.as_ref().and_then(|rt| rt.parameter_values());

                        for row in &chunk.rows {
                            let mut context =
                                row_context(row.clone(), layout.clone(), params.clone());
                            let vid_val = context
                                .get_variable("vid")
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            if let Ok(vid) = VertexId::try_from(&vid_val) {
                                let existing = writer
                                    .get_vertex(space_name, &vid)
                                    .map_err(|e| QueryError::execution(e.to_string()))?;
                                let existing = match existing {
                                    Some(ev) => ev,
                                    None => {
                                        if *is_upsert {
                                            let mut props = HashMap::new();
                                            for (prop_name, expr) in updates.iter() {
                                                let val = eval_expr(expr, &mut context)?;
                                                props.insert(prop_name.clone(), val);
                                            }
                                            let tags =
                                                vec![Tag::new(tag_name.clone(), props.clone())];
                                            let vertex =
                                                Vertex::new_with_properties(vid, tags, props);
                                            StorageWriter::insert_vertex(
                                                &mut *writer,
                                                space_name,
                                                vertex,
                                            )
                                            .map_err(|e| QueryError::execution(e.to_string()))?;
                                            *rows_updated += 1;
                                        } else {
                                            return Err(QueryError::execution(format!(
                                                "Vertex not found: {}",
                                                vid
                                            )));
                                        }
                                        continue;
                                    }
                                };
                                // Load existing properties into context so expressions
                                // like `SET stock = stock - 1` and conditions like
                                // `WHEN age > 100` can resolve existing columns.
                                for tag in &existing.tags {
                                    for (k, v) in &tag.properties {
                                        context.set_variable(k.clone(), v.clone());
                                    }
                                }
                                if let Some(cond) = condition {
                                    let keep = eval_expr(cond, &mut context)?;
                                    if !condition_matches(&keep) {
                                        continue;
                                    }
                                }
                                let mut props = HashMap::new();
                                for (prop_name, expr) in updates.iter() {
                                    let val = eval_expr(expr, &mut context)?;
                                    props.insert(prop_name.clone(), val);
                                }
                                let tags: Vec<Tag> = if tag_name.is_empty() {
                                    existing
                                        .tags
                                        .iter()
                                        .map(|t| {
                                            let mut merged = t.properties.clone();
                                            for (k, v) in &props {
                                                merged.insert(k.clone(), v.clone());
                                            }
                                            Tag::new(t.name.clone(), merged)
                                        })
                                        .collect()
                                } else {
                                    vec![Tag::new(tag_name.clone(), props)]
                                };
                                let vertex = Vertex::new_with_properties(vid, tags, HashMap::new());
                                StorageWriter::update_vertex(&mut *writer, space_name, vertex)
                                    .map_err(|e| QueryError::execution(e.to_string()))?;
                                *rows_updated += 1;
                            }
                        }
                    } else {
                        *rows_updated += chunk.rows.len() as u64;
                    }
                }

                *summary_returned = true;
                Ok(Some(make_modify_result(
                    Arc::clone(&self.output_layout),
                    "update_vertices",
                    *rows_updated,
                )))
            }

            SinkOperatorKind::UpdateEdges {
                storage,
                space_name,
                src_col,
                dst_col,
                edge_type,
                updates,
                condition,
                is_upsert,
                rows_updated,
                summary_returned,
                ..
            } => {
                if *summary_returned {
                    return Ok(None);
                }

                while let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Sink");
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let layout = chunk.get_layout();
                        let params = self.runtime.as_ref().and_then(|rt| rt.parameter_values());

                        for row in &chunk.rows {
                            let mut context =
                                row_context(row.clone(), layout.clone(), params.clone());
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
                                let existing = writer
                                    .get_edge(space_name, &src, &dst, edge_type, 0)
                                    .map_err(|e| QueryError::execution(e.to_string()))?;
                                let existing = match existing {
                                    Some(edge) => edge,
                                    None => {
                                        if *is_upsert {
                                            let mut props = HashMap::new();
                                            for (prop_name, expr) in updates.iter() {
                                                let val = eval_expr(expr, &mut context)?;
                                                props.insert(prop_name.clone(), val);
                                            }
                                            let edge =
                                                Edge::new(src, dst, edge_type.clone(), 0, props);
                                            StorageWriter::insert_edge(
                                                &mut *writer,
                                                space_name,
                                                edge,
                                            )
                                            .map_err(|e| QueryError::execution(e.to_string()))?;
                                            *rows_updated += 1;
                                        } else {
                                            return Err(QueryError::execution(format!(
                                                "Edge not found: {} -> {} of {}",
                                                src, dst, edge_type
                                            )));
                                        }
                                        continue;
                                    }
                                };
                                for (k, v) in &existing.props {
                                    context.set_variable(k.clone(), v.clone());
                                }
                                if let Some(cond) = condition {
                                    let keep = eval_expr(cond, &mut context)?;
                                    if !condition_matches(&keep) {
                                        continue;
                                    }
                                }
                                let mut props = HashMap::new();
                                for (prop_name, expr) in updates.iter() {
                                    let val = eval_expr(expr, &mut context)?;
                                    props.insert(prop_name.clone(), val);
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
                }

                *summary_returned = true;
                Ok(Some(make_modify_result(
                    Arc::clone(&self.output_layout),
                    "update_edges",
                    *rows_updated,
                )))
            }

            SinkOperatorKind::DeleteVertices {
                storage,
                space_name,
                vertex_id_col,
                rows_deleted,
                summary_returned,
                ..
            } => {
                if *summary_returned {
                    return Ok(None);
                }

                while let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Sink");
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let layout = chunk.get_layout();

                        for row in &chunk.rows {
                            let context = ValueRowContext::new(row.clone(), layout.clone());
                            if let Some(vid_val) = context.get_variable(vertex_id_col) {
                                if let Ok(vid) = VertexId::try_from(&vid_val) {
                                    StorageWriter::delete_vertex_with_edges(
                                        &mut *writer,
                                        space_name,
                                        &vid,
                                    )
                                    .map_err(|e| QueryError::execution(e.to_string()))?;
                                    *rows_deleted += 1;
                                }
                            }
                        }
                    } else {
                        *rows_deleted += chunk.rows.len() as u64;
                    }
                }

                *summary_returned = true;
                Ok(Some(make_modify_result(
                    Arc::clone(&self.output_layout),
                    "delete_vertices",
                    *rows_deleted,
                )))
            }

            SinkOperatorKind::DeleteEdges {
                storage,
                space_name,
                src_col,
                dst_col,
                edge_type,
                rows_deleted,
                summary_returned,
                ..
            } => {
                if *summary_returned {
                    return Ok(None);
                }

                while let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Sink");
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let layout = chunk.get_layout();

                        for row in &chunk.rows {
                            let context = ValueRowContext::new(row.clone(), layout.clone());
                            let src_val = context
                                .get_variable(src_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            let dst_val = context
                                .get_variable(dst_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            if let Some((src, dst)) = resolve_edge_endpoints(&src_val, &dst_val) {
                                StorageWriter::delete_edge(
                                    &mut *writer,
                                    space_name,
                                    &src,
                                    &dst,
                                    edge_type,
                                    0,
                                )
                                .map_err(|e| QueryError::execution(e.to_string()))?;
                                *rows_deleted += 1;
                            }
                        }
                    } else {
                        *rows_deleted += chunk.rows.len() as u64;
                    }
                }

                *summary_returned = true;
                Ok(Some(make_modify_result(
                    Arc::clone(&self.output_layout),
                    "delete_edges",
                    *rows_deleted,
                )))
            }

            SinkOperatorKind::PipeDeleteEdges {
                storage,
                space_name,
                src_col,
                dst_col,
                edge_type,
                rows_deleted,
                summary_returned,
                ..
            } => {
                if *summary_returned {
                    return Ok(None);
                }

                while let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Sink");
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let layout = chunk.get_layout();

                        for row in &chunk.rows {
                            let context = ValueRowContext::new(row.clone(), layout.clone());
                            let src_val = context
                                .get_variable(src_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            let dst_val = context
                                .get_variable(dst_col)
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            if let Some((src, dst)) = resolve_edge_endpoints(&src_val, &dst_val) {
                                StorageWriter::delete_edge(
                                    &mut *writer,
                                    space_name,
                                    &src,
                                    &dst,
                                    edge_type,
                                    0,
                                )
                                .map_err(|e| QueryError::execution(e.to_string()))?;
                                *rows_deleted += 1;
                            }
                        }
                    } else {
                        *rows_deleted += chunk.rows.len() as u64;
                    }
                }

                *summary_returned = true;
                Ok(Some(make_modify_result(
                    Arc::clone(&self.output_layout),
                    "delete_edges",
                    *rows_deleted,
                )))
            }

            SinkOperatorKind::PipeDeleteVertices {
                storage,
                space_name,
                vertex_id_col,
                rows_deleted,
                summary_returned,
                ..
            } => {
                if *summary_returned {
                    return Ok(None);
                }

                while let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Sink");
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if let Some(storage_lock) = storage {
                        let mut writer = storage_lock.write();
                        let layout = chunk.get_layout();

                        for row in &chunk.rows {
                            let context = ValueRowContext::new(row.clone(), layout.clone());
                            if let Some(vid_val) = context.get_variable(vertex_id_col) {
                                if let Ok(vid) = VertexId::try_from(&vid_val) {
                                    StorageWriter::delete_vertex_with_edges(
                                        &mut *writer,
                                        space_name,
                                        &vid,
                                    )
                                    .map_err(|e| QueryError::execution(e.to_string()))?;
                                    *rows_deleted += 1;
                                }
                            }
                        }
                    } else {
                        *rows_deleted += chunk.rows.len() as u64;
                    }
                }

                *summary_returned = true;
                Ok(Some(make_modify_result(
                    Arc::clone(&self.output_layout),
                    "pipe_delete_vertices",
                    *rows_deleted,
                )))
            }

            SinkOperatorKind::DeleteTags {
                storage,
                space_name,
                tag_names,
                vertex_ids,
                rows_deleted,
                summary_returned,
                ..
            } => {
                if *summary_returned {
                    return Ok(None);
                }

                if let Some(rt) = self.runtime.as_ref() {
                    rt.ensure_not_cancelled()?;
                }
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

                *summary_returned = true;
                Ok(Some(make_modify_result(
                    Arc::clone(&self.output_layout),
                    "delete_tags",
                    *rows_deleted,
                )))
            }
            SinkOperatorKind::CopyFrom {
                storage,
                space_name,
                target,
                file_path,
                header,
                delimiter,
                batch_size,
                rows_inserted,
                summary_returned,
                ..
            } => {
                if *summary_returned {
                    return Ok(None);
                }
                // Drain input (dummy single row)
                while let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("CopyFrom");
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                }
                if let Some(rt) = self.runtime.as_ref() {
                    rt.ensure_not_cancelled()?;
                }
                if let Some(storage_lock) = storage {
                    let count = execute_copy_from(
                        storage_lock,
                        space_name,
                        target,
                        file_path,
                        *header,
                        *delimiter,
                        *batch_size,
                        self.runtime.as_ref().map(|r| r.clone()),
                    )?;
                    *rows_inserted = count;
                } else {
                    // Mock storage: estimate from file line count if possible
                    *rows_inserted = 0;
                }
                *summary_returned = true;
                Ok(Some(make_modify_result(
                    Arc::clone(&self.output_layout),
                    "copy_from",
                    *rows_inserted,
                )))
            }
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }
}

// ── COPY FROM helpers (parallel CSV) ────────────────────────────────

fn parse_copy_value(s: &str) -> Value {
    if s.is_empty() {
        return Value::Null(crate::core::value::NullType::Null);
    }
    if s.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if s.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if s.eq_ignore_ascii_case("null") {
        return Value::Null(crate::core::value::NullType::Null);
    }
    if let Ok(i) = s.parse::<i64>() {
        return Value::BigInt(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        // Distinguish int-like floats already handled
        if s.contains('.') || s.contains('e') || s.contains('E') {
            return Value::Double(f);
        }
        return Value::Double(f);
    }
    Value::string(s)
}

fn vid_from_str(s: &str) -> Result<VertexId, QueryError> {
    let t = s.trim();
    if t.is_empty() {
        return Err(QueryError::execution("Empty vertex id in COPY".to_string()));
    }
    if let Ok(i) = t.parse::<i64>() {
        Ok(VertexId::from_int64(i))
    } else {
        Ok(VertexId::from_string(t))
    }
}

fn execute_copy_from(
    storage_lock: &Arc<RwLock<dyn QueryStorage>>,
    space_name: &str,
    target: &crate::query::executor::streaming::operators::spec::CopyTarget,
    file_path: &str,
    header: bool,
    delimiter: u8,
    batch_size: usize,
    runtime: Option<Arc<ExecutionRuntime>>,
) -> Result<u64, QueryError> {
    use csv::ReaderBuilder;
    use rayon::prelude::*;

    let file = File::open(file_path).map_err(|e| {
        QueryError::execution(format!("COPY FROM failed to open '{}': {}", file_path, e))
    })?;
    let reader = BufReader::new(file);
    let mut csv_reader = ReaderBuilder::new()
        .has_headers(header)
        .delimiter(delimiter)
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(reader);

    // Determine column mapping
    let (prop_names, vid_idx, src_idx, dst_idx) = match target {
        crate::query::executor::streaming::operators::spec::CopyTarget::Vertex(tag) => {
            if header {
                let headers = csv_reader
                    .headers()
                    .map_err(|e| QueryError::execution(format!("COPY CSV header error: {}", e)))?
                    .clone();
                let headers_vec: Vec<String> = headers.iter().map(|s| s.to_string()).collect();
                let vid_idx = headers_vec
                    .iter()
                    .position(|h| {
                        h.eq_ignore_ascii_case("vid")
                            || h.eq_ignore_ascii_case("id")
                            || h.eq_ignore_ascii_case("_id")
                            || h.eq_ignore_ascii_case("vertex_id")
                    })
                    .unwrap_or(0);
                let props: Vec<String> = headers_vec
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != vid_idx)
                    .map(|(_, s)| s.clone())
                    .collect();
                (props, vid_idx, 0usize, 0usize)
            } else {
                // No header: fetch tag schema for property names
                let props = {
                    let read = storage_lock.read();
                    match read.get_tag(space_name, tag) {
                        Ok(Some(info)) => info.properties.iter().map(|p| p.name.clone()).collect(),
                        Ok(None) => {
                            return Err(QueryError::execution(format!("Tag '{}' not found", tag)))
                        }
                        Err(e) => return Err(QueryError::execution(e.to_string())),
                    }
                };
                (props, 0, 0, 0)
            }
        }
        crate::query::executor::streaming::operators::spec::CopyTarget::Edge(edge_type) => {
            if header {
                let headers = csv_reader
                    .headers()
                    .map_err(|e| QueryError::execution(format!("COPY CSV header error: {}", e)))?
                    .clone();
                let headers_vec: Vec<String> = headers.iter().map(|s| s.to_string()).collect();
                // For edges, first two cols are src/dst if not named
                // Try to locate src/dst by name
                let src_idx = headers_vec
                    .iter()
                    .position(|h| {
                        h.eq_ignore_ascii_case("src")
                            || h.eq_ignore_ascii_case("_src")
                            || h.eq_ignore_ascii_case("source")
                    })
                    .unwrap_or(0);
                let dst_idx = headers_vec
                    .iter()
                    .position(|h| {
                        h.eq_ignore_ascii_case("dst")
                            || h.eq_ignore_ascii_case("_dst")
                            || h.eq_ignore_ascii_case("destination")
                            || h.eq_ignore_ascii_case("dest")
                    })
                    .unwrap_or(if src_idx == 0 { 1 } else { 0 });
                let props: Vec<String> = headers_vec
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != src_idx && *i != dst_idx)
                    .map(|(_, s)| s.clone())
                    .collect();
                (props, 0, src_idx, dst_idx)
            } else {
                let props = {
                    let read = storage_lock.read();
                    match read.get_edge_type(space_name, edge_type) {
                        Ok(Some(info)) => info.properties.iter().map(|p| p.name.clone()).collect(),
                        Ok(None) => {
                            return Err(QueryError::execution(format!(
                                "Edge type '{}' not found",
                                edge_type
                            )))
                        }
                        Err(e) => return Err(QueryError::execution(e.to_string())),
                    }
                };
                (props, 0, 0, 1)
            }
        }
    };

    let batch_sz = if batch_size == 0 { 1000 } else { batch_size };
    let mut total: u64 = 0;

    match target {
        crate::query::executor::streaming::operators::spec::CopyTarget::Vertex(tag) => {
            let mut batch_records: Vec<csv::StringRecord> = Vec::with_capacity(batch_sz);
            // For header disparity, keep prop_names for batch; if header true we already have mapping per header order.
            // But batch_records will be parsed in parallel using mapping.
            let flush_batch = |records: &mut Vec<csv::StringRecord>,
                               props: &[String],
                               vid_idx: usize,
                               tag: &str|
             -> Result<u64, QueryError> {
                if records.is_empty() {
                    return Ok(0);
                }
                // Parallel parse records into vertices
                let vertices: Vec<Vertex> = records
                    .par_iter()
                    .map(|rec| {
                        let vid_str = rec.get(vid_idx).unwrap_or("");
                        let vid = vid_from_str(vid_str)
                            .unwrap_or_else(|_| VertexId::from_string(vid_str));
                        let mut prop_map: HashMap<String, Value> = HashMap::new();
                        // Map remaining columns to prop names by position
                        // For header disparity, props order matches filtered header order.
                        // So we iterate over prop_names and pick record index accordingly.
                        // Need to map prop index to record index skipping vid_idx.
                        for (prop_i, prop_name) in props.iter().enumerate() {
                            // Compute record index: prop_i but skip vid_idx
                            let rec_idx = if prop_i >= vid_idx {
                                prop_i + 1
                            } else {
                                prop_i
                            };
                            // When header false with schema, mapping is simpler: col = vid_idx+1+prop_i
                            let value_str = rec.get(rec_idx).unwrap_or("");
                            let val = parse_copy_value(value_str);
                            prop_map.insert(prop_name.clone(), val);
                        }
                        if prop_map.is_empty() {
                            // Fallback: if no header mapping succeeded, try positional after vid
                            // (already covered)
                        }
                        let t = Tag::new(tag.to_string(), prop_map);
                        Vertex::new(vid, vec![t])
                    })
                    .collect();
                let count = vertices.len() as u64;
                // Write batch
                {
                    let mut writer = storage_lock.write();
                    StorageWriter::batch_insert_vertices(&mut *writer, space_name, vertices)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                }
                if let Some(rt) = &runtime {
                    rt.ensure_not_cancelled()
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                }
                Ok(count)
            };

            for result in csv_reader.records() {
                let record = result
                    .map_err(|e| QueryError::execution(format!("COPY CSV read error: {}", e)))?;
                batch_records.push(record);
                if batch_records.len() >= batch_sz {
                    let cnt = flush_batch(&mut batch_records, &prop_names, vid_idx, tag)?;
                    total += cnt;
                    batch_records.clear();
                }
            }
            if !batch_records.is_empty() {
                let cnt = flush_batch(&mut batch_records, &prop_names, vid_idx, tag)?;
                total += cnt;
            }
        }
        crate::query::executor::streaming::operators::spec::CopyTarget::Edge(edge_type) => {
            let mut batch_records: Vec<csv::StringRecord> = Vec::with_capacity(batch_sz);
            let flush_batch = |records: &mut Vec<csv::StringRecord>,
                               props: &[String],
                               src_idx: usize,
                               dst_idx: usize,
                               edge_type: &str|
             -> Result<u64, QueryError> {
                if records.is_empty() {
                    return Ok(0);
                }
                let edges: Vec<Edge> = records
                    .par_iter()
                    .map(|rec| {
                        let src_str = rec.get(src_idx).unwrap_or("");
                        let dst_str = rec.get(dst_idx).unwrap_or("");
                        let src = vid_from_str(src_str)
                            .unwrap_or_else(|_| VertexId::from_string(src_str));
                        let dst = vid_from_str(dst_str)
                            .unwrap_or_else(|_| VertexId::from_string(dst_str));
                        let mut prop_map: HashMap<String, Value> = HashMap::new();
                        for (prop_i, prop_name) in props.iter().enumerate() {
                            // Determine record index skipping src/dst
                            // For header disparity, filtered header order: we need to map prop_i to actual rec idx
                            // Simplify: iterate headers mapping already: prop_names filtered from headers, so prop_i corresponds to nth non-src/dst column in order.
                            // To find rec idx, count how many of src_idx/dst_idx are <= rec position.
                            // Easier: brute force find next index not src/dst
                            let mut rec_idx = 0;
                            // Walk through prop_i to compute rec idx
                            // This is O(n^2) but n small (props < 100)
                            let mut cur_prop = 0;
                            for idx in 0..rec.len() {
                                if idx == src_idx || idx == dst_idx {
                                    continue;
                                }
                                if cur_prop == prop_i {
                                    rec_idx = idx;
                                    break;
                                }
                                cur_prop += 1;
                            }
                            // Fallback if not found
                            let value_str = rec.get(rec_idx).unwrap_or("");
                            let val = parse_copy_value(value_str);
                            prop_map.insert(prop_name.clone(), val);
                        }
                        Edge {
                            src,
                            dst,
                            edge_type: edge_type.to_string(),
                            ranking: 0,
                            props: prop_map,
                        }
                    })
                    .collect();
                let count = edges.len() as u64;
                {
                    let mut writer = storage_lock.write();
                    StorageWriter::batch_insert_edges(&mut *writer, space_name, edges)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                }
                if let Some(rt) = &runtime {
                    rt.ensure_not_cancelled()
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                }
                Ok(count)
            };
            for result in csv_reader.records() {
                let record = result
                    .map_err(|e| QueryError::execution(format!("COPY CSV read error: {}", e)))?;
                batch_records.push(record);
                if batch_records.len() >= batch_sz {
                    let cnt =
                        flush_batch(&mut batch_records, &prop_names, src_idx, dst_idx, edge_type)?;
                    total += cnt;
                    batch_records.clear();
                }
            }
            if !batch_records.is_empty() {
                let cnt =
                    flush_batch(&mut batch_records, &prop_names, src_idx, dst_idx, edge_type)?;
                total += cnt;
            }
        }
    }
    Ok(total)
}
