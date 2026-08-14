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
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::source_operator::OperatorConfig;
use crate::query::executor::streaming::runtime::ExecutionRuntime;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::storage::{QueryStorage, StorageWriter};

#[derive(Debug)]
pub enum SinkOperatorKind {
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
            SinkOperatorKind::InsertVertices { .. }
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
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }
}
