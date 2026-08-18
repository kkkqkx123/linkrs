use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::{selection_propagation_enabled, DataChunk};
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::executor::ValueRowContext;
use crate::query::executor::streaming::operators::source_operator::OperatorConfig;
use crate::query::executor::streaming::runtime::ExecutionRuntime;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::executor::streaming::subquery::{EvalEnv, SubqueryExecutor};

#[derive(Debug, Default)]
pub struct UnaryOperatorState {
    pub env: EvalEnv,
}

#[derive(Debug)]
pub enum UnaryOperatorKind {
    Filter {
        predicate: Expression,
        state: UnaryOperatorState,
    },
    Project {
        output_expressions: Vec<Expression>,
        output_col_names: Vec<String>,
        state: UnaryOperatorState,
    },
    Limit {
        offset: u32,
        limit: u32,
        skipped: u32,
        consumed: u32,
    },
    Dedup {
        seen_rows: std::collections::HashSet<Vec<Value>>,
    },
    Assign {
        assignments: Vec<(String, Expression)>,
        state: UnaryOperatorState,
    },
    Remove {
        columns_to_remove: Vec<String>,
    },
    Unwind {
        unwind_column: String,
        list_expression: Option<Expression>,
        col_index: Option<usize>,
        layout: Option<Arc<SlotLayout>>,
        all_rows: Vec<Vec<Value>>,
        current_row_index: usize,
        current_unwind_index: usize,
        input_done: bool,
    },
    AppendVertices {
        entity_var: String,
        entity_expr: Expression,
        prop_names: Vec<String>,
        storage: Option<Arc<parking_lot::RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        state: UnaryOperatorState,
    },
    Sample {
        count: u64,
        consumed: u64,
    },
}

/// Unary operator.
///
/// Wraps [`UnaryOperatorKind`] with the runtime context injected at `open()`.
/// Lifecycle state is owned exclusively by the executor; operators never
/// write it.
#[derive(Debug)]
pub struct UnaryOperator {
    pub kind: UnaryOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl UnaryOperator {
    /// Create a UnaryOperator with fresh mutable state from an immutable spec.
    pub fn from_spec(spec: &super::spec::UnarySpec, output_layout: Arc<SlotLayout>) -> Self {
        let state = UnaryOperatorState::default();
        let kind = match spec {
            super::spec::UnarySpec::Filter {
                predicate,
                subquery_runners: _,
            } => UnaryOperatorKind::Filter {
                predicate: predicate.clone(),
                state,
            },
            super::spec::UnarySpec::Project {
                output_expressions,
                output_col_names,
                subquery_runners: _,
            } => UnaryOperatorKind::Project {
                output_expressions: output_expressions.clone(),
                output_col_names: output_col_names.clone(),
                state,
            },
            super::spec::UnarySpec::Limit { offset, limit } => UnaryOperatorKind::Limit {
                offset: *offset,
                limit: *limit,
                skipped: 0,
                consumed: 0,
            },
            super::spec::UnarySpec::Assign {
                assignments,
                subquery_runners: _,
            } => UnaryOperatorKind::Assign {
                assignments: assignments.clone(),
                state,
            },
            super::spec::UnarySpec::Remove { columns_to_remove } => UnaryOperatorKind::Remove {
                columns_to_remove: columns_to_remove.clone(),
            },
            super::spec::UnarySpec::Unwind {
                unwind_column,
                list_expression,
            } => UnaryOperatorKind::Unwind {
                unwind_column: unwind_column.clone(),
                list_expression: list_expression.clone(),
                col_index: None,
                layout: None,
                all_rows: Vec::new(),
                current_row_index: 0,
                current_unwind_index: 0,
                input_done: false,
            },
            super::spec::UnarySpec::AppendVertices {
                space_name,
                entity_var,
                entity_expr,
                prop_names,
            } => UnaryOperatorKind::AppendVertices {
                entity_var: entity_var.clone(),
                entity_expr: entity_expr.clone(),
                prop_names: prop_names.clone(),
                storage: None,
                space_name: space_name.clone(),
                state,
            },
            super::spec::UnarySpec::Sample { count } => UnaryOperatorKind::Sample {
                count: *count,
                consumed: 0,
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn new(kind: UnaryOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
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

    /// Inject the hosting operator's expression-level subquery executor into
    /// the Filter/Project/Assign state. Called by the materializer
    /// after `from_spec`; no-op for the other unary kinds.
    pub fn set_subquery_executor(&mut self, executor: Arc<SubqueryExecutor>) {
        match &mut self.kind {
            UnaryOperatorKind::Filter { state, .. }
            | UnaryOperatorKind::Project { state, .. }
            | UnaryOperatorKind::Assign { state, .. } => {
                state.env.subquery_executor = Some(executor);
            }
            _ => {}
        }
    }

    pub fn open(&mut self, input: &mut StreamingExecutor) -> Result<(), QueryError> {
        let params = self.runtime.as_ref().and_then(|rt| rt.parameter_values());
        let session_variables = self
            .runtime
            .as_ref()
            .and_then(|rt| rt.session_variable_values());
        let storage = self.runtime.as_ref().and_then(|rt| rt.storage.clone());
        match &mut self.kind {
            UnaryOperatorKind::Filter { state, .. }
            | UnaryOperatorKind::Project { state, .. }
            | UnaryOperatorKind::Assign { state, .. }
            | UnaryOperatorKind::AppendVertices { state, .. } => {
                state.env.params = params;
                state.env.session_variables = session_variables;
            }
            _ => {}
        }
        if let UnaryOperatorKind::AppendVertices {
            storage: target, ..
        } = &mut self.kind
        {
            *target = storage;
        }
        input.open()?;
        Ok(())
    }

    pub fn next(&mut self, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        let Self {
            kind,
            runtime,
            output_layout,
            ..
        } = self;
        match kind {
            UnaryOperatorKind::Filter { predicate, state } => loop {
                match input.advance()? {
                    Some(mut chunk) => {
                        let results = chunk
                            .evaluate_expression(predicate, Some(&state.env))
                            .map_err(|e| {
                                QueryError::execution(format!(
                                    "Filter predicate evaluation failed: {}",
                                    e
                                ))
                            })?;
                        // Build a selection vector restricted to the
                        // currently-visible rows (a nested filter keeps the
                        // absolute row indices).
                        let mut selected = Vec::new();
                        match chunk.selection() {
                            None => {
                                for (i, val) in results.iter().enumerate() {
                                    if matches_value(val) {
                                        selected.push(i);
                                    }
                                }
                            }
                            Some(sel) => {
                                for &i in sel {
                                    if matches_value(&results[i]) {
                                        selected.push(i);
                                    }
                                }
                            }
                        }
                        if selected.is_empty() {
                            continue;
                        }
                        // All visible rows selected — hand the chunk through
                        // as-is, keeping any existing selection.
                        if selected.len() == chunk.visible_count() {
                            if selection_propagation_enabled() {
                                return Ok(Some(chunk));
                            }
                            // Rollback mode: never hand a selection downstream.
                            chunk.materialize_selection_by("Filter"); // rollback path
                            return Ok(Some(chunk));
                        }
                        if !selection_propagation_enabled() {
                            let selected_chunk = chunk.take_indices(&selected);
                            return Ok(Some(selected_chunk));
                        }
                        // Attach the selection vector instead of moving rows;
                        // the columnar/typed caches stay valid for the downstream
                        // selection-aware consumers.
                        if let Some(stats) = &chunk.columnar_stats {
                            stats.record_selection_attached();
                        }
                        let selected_chunk = chunk.with_selection(selected);
                        return Ok(Some(selected_chunk));
                    }
                    None => return Ok(None),
                }
            },
            UnaryOperatorKind::Project {
                output_expressions,
                output_col_names: _,
                state,
            } => loop {
                if let Some(mut chunk) = input.advance()? {
                    // When the child carries a selection vector, evaluate
                    // each output expression only for the visible rows — the
                    // output chunk is fully materialized (small).
                    if chunk.selection().is_some() {
                        let mut columns = Vec::with_capacity(output_expressions.len());
                        for expr in output_expressions.iter() {
                            let col = chunk
                                .evaluate_expression_visible(expr, Some(&state.env))
                                .map_err(|e| {
                                    QueryError::execution(format!(
                                        "Project expression evaluation failed: {}",
                                        e
                                    ))
                                })?;
                            columns.push(col);
                        }
                        if !columns.is_empty() && !columns[0].is_empty() {
                            return Ok(Some(DataChunk::from_columns(
                                columns,
                                Arc::clone(output_layout),
                            )));
                        }
                        continue;
                    }
                    let columns = chunk
                        .evaluate_expressions(output_expressions, Some(&state.env))
                        .map_err(|e| {
                            QueryError::execution(format!(
                                "Project expression evaluation failed: {}",
                                e
                            ))
                        })?;
                    if !columns.is_empty() && !columns[0].is_empty() {
                        return Ok(Some(DataChunk::from_columns(
                            columns,
                            Arc::clone(output_layout),
                        )));
                    }
                } else {
                    return Ok(None);
                }
            },
            UnaryOperatorKind::Limit {
                offset,
                limit,
                skipped,
                consumed,
            } => {
                if *consumed >= *limit {
                    return Ok(None);
                }

                loop {
                    let Some(mut chunk) = input.advance()? else {
                        return Ok(None);
                    };
                    // P2: consume offset/limit directly on the visible rows.
                    // The selection vector (if any) is trimmed in place and
                    // handed downstream, so the chunk stays compact across
                    // the boundary instead of materializing.
                    let mut vis = chunk.visible_indices();
                    let remain_offset = (*offset).saturating_sub(*skipped) as usize;
                    *skipped += vis.len().min(remain_offset) as u32;
                    if remain_offset < vis.len() {
                        vis.drain(..remain_offset);
                    } else {
                        vis.clear();
                    }
                    if vis.is_empty() {
                        continue;
                    }
                    let remaining_limit = (*limit - *consumed) as usize;
                    if vis.len() > remaining_limit {
                        vis.truncate(remaining_limit);
                    }
                    *consumed += vis.len() as u32;
                    if vis.len() == chunk.rows.len() {
                        // Every row is visible again — drop the redundant
                        // selection so the invariant stays tight.
                        let _ = chunk.take_selection();
                        return Ok(Some(chunk));
                    }
                    return Ok(Some(chunk.with_selection(vis)));
                }
            }
            UnaryOperatorKind::Dedup { seen_rows } => {
                while let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Dedup");
                    let mut result_rows = vec![];
                    for row in chunk.rows {
                        if seen_rows.insert(row.clone()) {
                            result_rows.push(row);
                        }
                    }
                    if !result_rows.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(
                            result_rows,
                            Arc::clone(output_layout),
                        )));
                    }
                }
                Ok(None)
            }
            UnaryOperatorKind::Assign { assignments, state } => loop {
                if let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Assign");
                    // Batch-evaluate all assignment expressions first
                    let mut new_cols: Vec<Vec<Value>> = Vec::with_capacity(assignments.len());
                    for (_col_name, expr) in assignments.iter() {
                        let col = match chunk.evaluate_expression(expr, Some(&state.env)) {
                            Ok(col) => col,
                            Err(_) => {
                                vec![Value::Null(crate::core::value::NullType::Null); chunk.len()]
                            }
                        };
                        new_cols.push(col);
                    }
                    // Extend each row with the computed values
                    for (i, row) in chunk.rows.iter_mut().enumerate() {
                        for col in &new_cols {
                            row.push(col[i].clone());
                        }
                    }
                    if !chunk.rows.is_empty() {
                        // Invalidate columnar caches since rows changed
                        chunk.columns = None;
                        chunk.typed_columns = None;
                        return Ok(Some(DataChunk::new_with_layout(
                            chunk.rows,
                            Arc::clone(output_layout),
                        )));
                    }
                } else {
                    return Ok(None);
                }
            },
            UnaryOperatorKind::Remove { columns_to_remove } => loop {
                if let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Remove");
                    let col_names = chunk.col_names();
                    let mut indices_to_keep = vec![];
                    for (idx, col_name) in col_names.iter().enumerate() {
                        if !columns_to_remove.contains(col_name) {
                            indices_to_keep.push(idx);
                        }
                    }
                    let mut result_rows = vec![];
                    for row in chunk.rows {
                        let mut new_row = vec![];
                        for idx in &indices_to_keep {
                            if *idx < row.len() {
                                new_row.push(row[*idx].clone());
                            }
                        }
                        result_rows.push(new_row);
                    }
                    if !result_rows.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(
                            result_rows,
                            Arc::clone(output_layout),
                        )));
                    }
                } else {
                    return Ok(None);
                }
            },
            UnaryOperatorKind::Unwind {
                unwind_column,
                list_expression,
                col_index,
                layout,
                all_rows,
                current_row_index,
                current_unwind_index,
                input_done,
            } => loop {
                if let Some(rt) = runtime.as_ref() {
                    rt.ensure_not_cancelled()?;
                }
                if *current_row_index >= all_rows.len() && !*input_done {
                    match input.advance()? {
                        Some(mut chunk) => {
                            // P2: boundary materialization.
                            chunk.materialize_selection_by("Unwind");
                            let col_names = chunk.col_names();
                            *col_index = col_names.iter().position(|c| c == unwind_column.as_str());
                            *layout = Some(chunk.get_layout());
                            *all_rows = chunk.rows;
                            *current_row_index = 0;
                            *current_unwind_index = 0;
                        }
                        None => {
                            *input_done = true;
                            if all_rows.is_empty() && list_expression.is_some() {
                                *all_rows = vec![Vec::new()];
                                *current_row_index = 0;
                                *current_unwind_index = 0;
                            } else {
                                return Ok(None);
                            }
                        }
                    }
                    continue;
                }
                if *current_row_index >= all_rows.len() {
                    return Ok(None);
                }
                let row = &all_rows[*current_row_index];
                let list_val: Option<Value> = if let Some(expr) = list_expression {
                    let row_layout = match layout {
                        Some(l) => l.clone(),
                        None => Arc::new(SlotLayout::new(vec![])),
                    };
                    let mut ctx = ValueRowContext::new(row.clone(), row_layout);
                    ExpressionEvaluator::evaluate(expr, &mut ctx).ok()
                } else {
                    col_index.and_then(|idx| row.get(idx).cloned())
                };
                let result_row = match list_val {
                    Some(Value::List(items)) if *current_unwind_index < items.len() => {
                        let mut result_row = row.clone();
                        result_row.push(items[*current_unwind_index].clone());
                        *current_unwind_index += 1;
                        Some(result_row)
                    }
                    Some(Value::List(items)) if items.is_empty() => None,
                    _ => None,
                };
                if let Some(result_row) = result_row {
                    return Ok(Some(DataChunk::new_with_layout(
                        vec![result_row],
                        Arc::clone(output_layout),
                    )));
                }
                *current_row_index += 1;
                *current_unwind_index = 0;
            },
            UnaryOperatorKind::AppendVertices {
                entity_var: _,
                entity_expr,
                prop_names,
                storage,
                space_name,
                state,
            } => loop {
                if let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("AppendVertices");
                    let layout = chunk.get_layout();
                    let storage_ref = storage.as_ref().ok_or_else(|| {
                        QueryError::execution("AppendVertices requires storage".to_string())
                    })?;
                    let guard = storage_ref.read();
                    let flat = !prop_names.is_empty();
                    let mut result_rows = Vec::new();
                    for row in chunk.rows {
                        let mut new_row = row.clone();
                        let mut ctx = if let Some(ref params) = state.env.params {
                            ValueRowContext::with_parameters(
                                row.clone(),
                                layout.clone(),
                                params.clone(),
                            )
                        } else {
                            ValueRowContext::new(row.clone(), layout.clone())
                        };
                        let entity = match ExpressionEvaluator::evaluate(entity_expr, &mut ctx) {
                            Ok(val) => val,
                            Err(_) => Value::Null(crate::core::NullType::Null),
                        };
                        let vid = match crate::core::types::storage_ids::VertexId::try_from(&entity)
                        {
                            Ok(vid) => vid,
                            Err(_) => {
                                new_row.push(Value::Null(crate::core::NullType::Null));
                                result_rows.push(new_row);
                                continue;
                            }
                        };
                        match guard.get_vertex_projected(space_name, &vid, prop_names) {
                            Ok(Some(vertex)) => {
                                if flat {
                                    for prop in prop_names.iter() {
                                        new_row.push(vertex.property_value(prop).unwrap_or_else(
                                            || Value::Null(crate::core::NullType::Null),
                                        ));
                                    }
                                } else {
                                    new_row.push(Value::Vertex(Box::new(vertex)));
                                }
                            }
                            Ok(None) => {
                                if flat {
                                    for _ in prop_names.iter() {
                                        new_row.push(Value::Null(crate::core::NullType::Null));
                                    }
                                } else {
                                    new_row.push(Value::Null(crate::core::NullType::Null));
                                }
                            }
                            Err(_) => {
                                new_row.push(Value::Null(crate::core::NullType::Null));
                            }
                        }
                        result_rows.push(new_row);
                    }
                    if !result_rows.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(
                            result_rows,
                            Arc::clone(output_layout),
                        )));
                    }
                } else {
                    return Ok(None);
                }
            },
            UnaryOperatorKind::Sample { count, consumed } => {
                if *consumed >= *count {
                    return Ok(None);
                }
                loop {
                    match input.advance()? {
                        Some(mut chunk) => {
                            // P2: boundary materialization.
                            chunk.materialize_selection_by("Sample");
                            let remaining = (*count - *consumed) as usize;
                            let take_count = chunk.rows.len().min(remaining);
                            let rows: Vec<Vec<Value>> =
                                chunk.rows.into_iter().take(take_count).collect();
                            *consumed += take_count as u64;
                            if !rows.is_empty() {
                                return Ok(Some(DataChunk::new_with_layout(
                                    rows,
                                    Arc::clone(output_layout),
                                )));
                            } else {
                                continue;
                            }
                        }
                        None => return Ok(None),
                    }
                }
            }
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    /// Reset this operator's per-run counters/buffers and rewind the input.
    ///
    /// Stateless operators (Filter/Project/Assign/Remove/AppendVertices)
    /// are no-ops beyond rewinding the input; Limit/Sample reset their
    /// counters; Dedup clears its seen-set; Unwind clears its input buffer.
    pub fn reset(&mut self, input: &mut StreamingExecutor) -> Result<bool, QueryError> {
        match &mut self.kind {
            UnaryOperatorKind::Limit {
                skipped, consumed, ..
            } => {
                *skipped = 0;
                *consumed = 0;
            }
            UnaryOperatorKind::Dedup { seen_rows } => seen_rows.clear(),
            UnaryOperatorKind::Unwind {
                col_index,
                layout,
                all_rows,
                current_row_index,
                current_unwind_index,
                input_done,
                ..
            } => {
                *col_index = None;
                *layout = None;
                all_rows.clear();
                *current_row_index = 0;
                *current_unwind_index = 0;
                *input_done = false;
            }
            UnaryOperatorKind::Sample { consumed, .. } => *consumed = 0,
            UnaryOperatorKind::Filter { .. }
            | UnaryOperatorKind::Project { .. }
            | UnaryOperatorKind::Assign { .. }
            | UnaryOperatorKind::Remove { .. }
            | UnaryOperatorKind::AppendVertices { .. } => {}
        }
        input.reset()?;
        Ok(false)
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }
}

/// Convert a Value to a boolean for filter predicate evaluation.
fn matches_value(val: &Value) -> bool {
    match val {
        Value::Bool(b) => *b,
        Value::Null(_) => false,
        Value::Int(i) => *i != 0,
        Value::BigInt(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::Double(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::storage_ids::VertexId;
    use crate::core::{Tag, Vertex};
    use crate::query::executor::base::MemoryBudget;
    use crate::query::executor::streaming::operators::base::OperatorBase;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;
    use crate::query::executor::streaming::operators::source_operator::SourceOperatorKind;
    use crate::query::executor::streaming::runtime::{ExecutionRuntime, QueryIdentity};
    use crate::storage::StorageWriter;
    use parking_lot::RwLock;

    fn runtime_with_storage(
        storage: Arc<RwLock<dyn crate::storage::QueryStorage>>,
    ) -> Arc<ExecutionRuntime> {
        Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            Some(storage),
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ))
    }

    fn scan_source(rows: Vec<Vec<Value>>, col_names: Vec<String>) -> Box<StreamingExecutor> {
        Box::new(StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::new(
                SourceOperatorKind::ScanVertices {
                    buffer: rows,
                    current_index: 0,
                    col_names,
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        ))
    }

    #[test]
    fn append_vertices_fetches_vertex_and_appends_flat_columns() {
        let mut mock = crate::storage::MockStorage::new().expect("MockStorage should be created");
        mock.insert_vertex(
            "test",
            Vertex::new(
                VertexId::from_int64(1),
                vec![Tag::new(
                    "person".to_string(),
                    vec![
                        ("name".to_string(), Value::string("Alice")),
                        ("age".to_string(), Value::Int(30)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            ),
        )
        .expect("insert vertex");
        let storage: Arc<RwLock<dyn crate::storage::QueryStorage>> = Arc::new(RwLock::new(mock));

        // Input row: [vid]
        let input = scan_source(vec![vec![Value::string("1")]], vec!["vid".to_string()]);
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::new(
                UnaryOperatorKind::AppendVertices {
                    entity_var: "v".to_string(),
                    entity_expr: Expression::Variable("vid".to_string()),
                    prop_names: vec!["name".to_string(), "age".to_string()],
                    storage: None,
                    space_name: "test".to_string(),
                    state: UnaryOperatorState::default(),
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        );
        append.base_mut().runtime = Some(runtime_with_storage(storage.clone()));
        append.open().expect("open should succeed");
        let chunk = append.advance().expect("advance should succeed");
        let chunk = chunk.expect("one output chunk");
        assert_eq!(chunk.len(), 1);
        let row = &chunk.rows[0];
        assert_eq!(row.len(), 3, "vid + name + age");
        assert_eq!(row[0], Value::string("1"));
        assert_eq!(row[1], Value::string("Alice"));
        assert_eq!(row[2], Value::Int(30));
        assert!(append.advance().expect("advance").is_none());
    }

    #[test]
    fn append_vertices_full_value_appends_vertex_object() {
        let mut mock = crate::storage::MockStorage::new().expect("MockStorage should be created");
        mock.insert_vertex(
            "test",
            Vertex::new(
                VertexId::from_int64(7),
                vec![Tag::new(
                    "person".to_string(),
                    vec![("name".to_string(), Value::string("Bob"))]
                        .into_iter()
                        .collect(),
                )],
            ),
        )
        .expect("insert vertex");
        let storage: Arc<RwLock<dyn crate::storage::QueryStorage>> = Arc::new(RwLock::new(mock));

        let input = scan_source(vec![vec![Value::Int(7)]], vec!["vid".to_string()]);
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::new(
                UnaryOperatorKind::AppendVertices {
                    entity_var: "v".to_string(),
                    entity_expr: Expression::Variable("vid".to_string()),
                    prop_names: vec![],
                    storage: None,
                    space_name: "test".to_string(),
                    state: UnaryOperatorState::default(),
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        );
        append.base_mut().runtime = Some(runtime_with_storage(storage.clone()));
        append.open().expect("open should succeed");
        let chunk = append.advance().expect("advance").expect("one chunk");
        let row = &chunk.rows[0];
        assert_eq!(row.len(), 2, "vid + full vertex");
        match &row[1] {
            Value::Vertex(vertex) => {
                assert_eq!(vertex.vid, VertexId::from_int64(7));
                assert_eq!(vertex.property_value("name"), Some(Value::string("Bob")));
            }
            other => panic!("expected full vertex, got {:?}", other),
        }
    }

    #[test]
    fn append_vertices_missing_vertex_yields_null_columns() {
        let storage: Arc<RwLock<dyn crate::storage::QueryStorage>> = Arc::new(RwLock::new(
            crate::storage::MockStorage::new().expect("MockStorage should be created"),
        ));
        let input = scan_source(
            vec![vec![Value::string("missing")]],
            vec!["vid".to_string()],
        );
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::new(
                UnaryOperatorKind::AppendVertices {
                    entity_var: "v".to_string(),
                    entity_expr: Expression::Variable("vid".to_string()),
                    prop_names: vec!["name".to_string()],
                    storage: None,
                    space_name: "test".to_string(),
                    state: UnaryOperatorState::default(),
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        );
        append.base_mut().runtime = Some(runtime_with_storage(storage.clone()));
        append.open().expect("open should succeed");
        let chunk = append.advance().expect("advance").expect("one chunk");
        let row = &chunk.rows[0];
        assert_eq!(row.len(), 2);
        assert!(matches!(row[1], Value::Null(_)));
    }

    #[test]
    fn append_vertices_without_storage_fails() {
        let input = scan_source(vec![vec![Value::Int(1)]], vec!["vid".to_string()]);
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::new(
                UnaryOperatorKind::AppendVertices {
                    entity_var: "v".to_string(),
                    entity_expr: Expression::Variable("vid".to_string()),
                    prop_names: vec!["name".to_string()],
                    storage: None,
                    space_name: "test".to_string(),
                    state: UnaryOperatorState::default(),
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        );
        append.open().expect("open should succeed");
        let error = append.advance().expect_err("storage required");
        assert!(error.to_string().contains("requires storage"));
    }
}
