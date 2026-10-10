use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::executor::expression::evaluator::traits::ExpressionContext;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::context::ValueRowContext;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use crate::storage::{QueryStorage, StorageWriter};
use linkrs_core::error::QueryError;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::vertex_edge_path::{Edge, Tag, Vertex};
use linkrs_core::Value;

mod copy;
mod delete;
mod insert;
mod update;

#[derive(Debug)]
pub enum SinkOperatorKind {
    CopyFrom {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        target: crate::executor::streaming::operators::spec::CopyTarget,
        file_paths: Vec<String>,
        by_column: bool,
        header: bool,
        delimiter: u8,
        batch_size: usize,
        rows_inserted: u64,
        summary_returned: bool,
    },
    CopyTo {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        target: crate::executor::streaming::operators::spec::CopyTarget,
        file_path: String,
        header: bool,
        delimiter: u8,
        rows_exported: u64,
        summary_returned: bool,
    },
    InsertVertices {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        vertex_properties: Vec<(String, Expression)>,
        tag: String,
        tag_property_names: Vec<String>,
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
        replace_properties: bool,
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
        replace_properties: bool,
        rows_updated: u64,
        summary_returned: bool,
    },
    DeleteVertices {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        tag: String,
        vertex_id_col: String,
        cascade: bool,
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
        cascade: bool,
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

pub(super) fn make_modify_result(
    output_layout: Arc<SlotLayout>,
    op: &str,
    count: u64,
) -> DataChunk {
    let row = vec![Value::string(op), Value::BigInt(count as i64)];
    DataChunk::new_with_layout(vec![row], output_layout)
}

pub(super) fn eval_expr(
    expr: &Expression,
    context: &mut ValueRowContext,
) -> Result<Value, QueryError> {
    ExpressionEvaluator::evaluate(expr, context).map_err(|e| QueryError::execution(e.to_string()))
}

pub(super) fn eval_update_props(
    updates: &[(String, Expression)],
    replace_properties: bool,
    context: &mut ValueRowContext,
) -> Result<HashMap<Arc<str>, Value>, QueryError> {
    if !replace_properties {
        let mut props = HashMap::new();
        for (prop_name, expr) in updates.iter() {
            let val = eval_expr(expr, context)?;
            props.insert(Arc::from(prop_name.as_str()), val);
        }
        return Ok(props);
    }
    let mut props = HashMap::new();
    let mut saw_map = false;
    for (prop_name, expr) in updates.iter() {
        let val = eval_expr(expr, context)?;
        match val {
            Value::Map(entries) => {
                saw_map = true;
                for (key, item) in entries.iter() {
                    match key {
                        Value::String(name) => {
                            props.insert(Arc::from(name.as_str()), item.clone());
                        }
                        Value::FixedString(name) => {
                            props.insert(Arc::from(name.as_str()), item.clone());
                        }
                        _ => {
                            return Err(QueryError::execution(
                                "Map overwrite keys must be strings".to_string(),
                            ));
                        }
                    }
                }
            }
            other => {
                props.insert(Arc::from(prop_name.as_str()), other);
            }
        }
    }
    if !saw_map {
        return Err(QueryError::execution(
            "Map overwrite value must be a map".to_string(),
        ));
    }
    Ok(props)
}

/// Build a row context that resolves `$name` parameter references.
///
/// Sink operators evaluate shape-normalized DML expressions (`$__dml_N`
/// placeholders), so the context must carry the runtime parameter values —
/// otherwise parameter resolution fails with "Undefined parameter".
pub(super) fn row_context(
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
pub(super) fn condition_matches(value: &Value) -> bool {
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
///
/// Whether a write-path error message indicates a transaction conflict
/// (write-write conflict, rollback-only transaction).
///
/// Storage errors are flattened to strings at the operator boundary; the
/// storage layer's typed `StorageErrorKind::Conflict` renders as "conflict"
/// / "Write-write conflict", which this classifier recognizes.
fn is_transaction_conflict_message(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("conflict")
        || lowered.contains("rollback-only")
        || lowered.contains("rollback_only")
}

pub(super) fn resolve_edge_endpoints(
    src_val: &Value,
    dst_val: &Value,
) -> Option<(VertexId, VertexId)> {
    match (src_val, dst_val) {
        (Value::Edge(edge), _) => Some((edge.src, edge.dst)),
        (_, Value::Edge(edge)) => Some((edge.src, edge.dst)),
        (Value::EdgeHeader(header), _) => Some((header.src, header.dst)),
        (_, Value::EdgeHeader(header)) => Some((header.src, header.dst)),
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
                file_paths,
                by_column,
                header,
                delimiter,
                batch_size,
            } => SinkOperatorKind::CopyFrom {
                storage,
                space_name: space_name.clone(),
                target: target.clone(),
                file_paths: file_paths.clone(),
                by_column: *by_column,
                header: *header,
                delimiter: *delimiter,
                batch_size: *batch_size,
                rows_inserted: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::CopyTo {
                space_name,
                target,
                file_path,
                header,
                delimiter,
            } => SinkOperatorKind::CopyTo {
                storage,
                space_name: space_name.clone(),
                target: target.clone(),
                file_path: file_path.clone(),
                header: *header,
                delimiter: *delimiter,
                rows_exported: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::InsertVertices {
                space_name,
                vertex_properties,
                tag,
                tag_property_names,
                if_not_exists,
            } => SinkOperatorKind::InsertVertices {
                storage,
                space_name: space_name.clone(),
                vertex_properties: vertex_properties.clone(),
                tag: tag.clone(),
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
                replace_properties,
            } => SinkOperatorKind::UpdateVertices {
                storage,
                space_name: space_name.clone(),
                tag_name: tag_name.clone(),
                updates: updates.clone(),
                condition: condition.clone(),
                is_upsert: *is_upsert,
                replace_properties: *replace_properties,
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
                replace_properties,
            } => SinkOperatorKind::UpdateEdges {
                storage,
                space_name: space_name.clone(),
                src_col: src_col.clone(),
                dst_col: dst_col.clone(),
                edge_type: edge_type.clone(),
                updates: updates.clone(),
                condition: condition.clone(),
                is_upsert: *is_upsert,
                replace_properties: *replace_properties,
                rows_updated: 0,
                summary_returned: false,
            },
            super::spec::SinkSpec::DeleteVertices {
                space_name,
                tag,
                vertex_id_col,
                cascade,
            } => SinkOperatorKind::DeleteVertices {
                storage,
                space_name: space_name.clone(),
                tag: tag.clone(),
                vertex_id_col: vertex_id_col.clone(),
                cascade: *cascade,
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
                cascade,
            } => SinkOperatorKind::PipeDeleteVertices {
                storage,
                space_name: space_name.clone(),
                vertex_id_col: vertex_id_col.clone(),
                cascade: *cascade,
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
    /// requires a transaction scope for DML operations.  Absent scope
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
            | SinkOperatorKind::CopyTo { .. }
            | SinkOperatorKind::InsertVertices { .. }
            | SinkOperatorKind::InsertEdges { .. }
            | SinkOperatorKind::UpdateVertices { .. }
            | SinkOperatorKind::UpdateEdges { .. }
            | SinkOperatorKind::DeleteVertices { .. }
            | SinkOperatorKind::DeleteEdges { .. }
            | SinkOperatorKind::PipeDeleteVertices { .. }
            | SinkOperatorKind::PipeDeleteEdges { .. } => {
                input.open()?;
                Ok(())
            }
        }
    }

    pub fn next(&mut self, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        let result = self.next_inner(input);
        if let Err(error) = &result {
            // Write-conflict linkage: a conflict-classified failure (write-write
            // conflict, rollback-only transaction) cancels the remaining pipeline
            // stages of the same transaction instead of letting them compute on.
            if is_transaction_conflict_message(&error.to_string()) {
                if let Some(rt) = self.runtime.as_ref() {
                    rt.note_transaction_conflict();
                }
            }
        }
        result
    }

    fn next_inner(
        &mut self,
        input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        if matches!(&self.kind, SinkOperatorKind::CopyFrom { .. }) {
            return copy::handle_copy_from(self, input);
        }
        if matches!(&self.kind, SinkOperatorKind::CopyTo { .. }) {
            return copy::handle_copy_to(self, input);
        }
        if matches!(&self.kind, SinkOperatorKind::InsertVertices { .. }) {
            return insert::handle_insert_vertices(self, input);
        }
        if matches!(&self.kind, SinkOperatorKind::InsertEdges { .. }) {
            return insert::handle_insert_edges(self, input);
        }
        if matches!(&self.kind, SinkOperatorKind::UpdateVertices { .. }) {
            return update::handle_update_vertices(self, input);
        }
        if matches!(&self.kind, SinkOperatorKind::UpdateEdges { .. }) {
            return update::handle_update_edges(self, input);
        }
        if matches!(&self.kind, SinkOperatorKind::DeleteVertices { .. }) {
            return delete::handle_delete_vertices(self, input);
        }
        if matches!(&self.kind, SinkOperatorKind::DeleteEdges { .. }) {
            return delete::handle_delete_edges(self, input);
        }
        if matches!(&self.kind, SinkOperatorKind::PipeDeleteVertices { .. }) {
            return delete::handle_pipe_delete_vertices(self, input);
        }
        if matches!(&self.kind, SinkOperatorKind::PipeDeleteEdges { .. }) {
            return delete::handle_pipe_delete_edges(self, input);
        }
        unreachable!("sink_operator::next_inner called for an unknown kind")
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }
}
