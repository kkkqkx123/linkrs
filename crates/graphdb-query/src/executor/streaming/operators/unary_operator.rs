use std::sync::Arc;

use crate::executor::expression::evaluator::compiled::{compiled_eval_enabled, CompiledExpr};
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::{selection_propagation_enabled, DataChunk};
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::executor::ValueRowContext;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use crate::executor::streaming::subquery::{EvalEnv, SubqueryExecutor};
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::Value;

#[derive(Debug, Default)]
pub struct UnaryOperatorState {
    pub env: EvalEnv,
    /// Lazily-compiled Filter predicate (compiled on the first chunk whose
    /// layout binds the expression's slots; reused for all later chunks).
    pub compiled_predicate: Option<CompiledExpr>,
    /// Lazily-compiled Project output expressions, one per output column.
    pub compiled_project: Option<Vec<CompiledExpr>>,
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
    Flatten {
        group_pos: u32,
        current_idx: usize,
        size_to_flatten: usize,
        saved_sel_vector: Option<Vec<usize>>,
        buffered_chunk: Option<DataChunk>,
        /// Rows emitted per output chunk. Defaults to the vectorized morsel
        /// size so the batched flatten path produces one chunk per input
        /// morsel; tests may set `1` for the single-row path.
        batch_size: usize,
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

impl UnaryOperatorKind {
    /// Plan level factorization group this flatten replays, if any.
    pub fn flatten_group_pos(&self) -> Option<u32> {
        match self {
            UnaryOperatorKind::Flatten { group_pos, .. } => Some(*group_pos),
            _ => None,
        }
    }
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
            super::spec::UnarySpec::Flatten { group_pos } => UnaryOperatorKind::Flatten {
                group_pos: *group_pos,
                current_idx: 0,
                size_to_flatten: 0,
                saved_sel_vector: None,
                buffered_chunk: None,
                batch_size:
                    crate::executor::streaming::operators::flatten::DEFAULT_FLATTEN_BATCH_SIZE,
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

    /// Evaluate the filter predicate, preferring the compiled closure tree.
    ///
    /// The predicate is compiled once against the first chunk's layout and
    /// reused for all later chunks. When the compiled path is disabled
    /// (rollback switch) or compilation/evaluation fails, the scalar chunk
    /// path is used so semantics stay identical.
    fn evaluate_filter_predicate(
        chunk: &mut DataChunk,
        predicate: &Expression,
        state: &mut UnaryOperatorState,
    ) -> Result<Vec<Value>, QueryError> {
        if compiled_eval_enabled() {
            if state.compiled_predicate.is_none() {
                let layout = chunk.get_layout();
                state.compiled_predicate = Some(CompiledExpr::compile(predicate, &layout));
            }
            if let Some(compiled) = &state.compiled_predicate {
                let layout = chunk.get_layout();
                let len = chunk.rows.len();
                match compiled.evaluate_batch(&chunk.rows, layout, Some(&state.env)) {
                    Ok(col) => return Ok(col.into_values(len)),
                    Err(_) => {
                        // Compiled evaluation failed; fall back to the scalar
                        // path so the runtime error text stays identical.
                    }
                }
            }
        }
        chunk
            .evaluate_expression(predicate, Some(&state.env))
            .map_err(|e| {
                QueryError::execution(format!("Filter predicate evaluation failed: {}", e))
            })
    }

    /// Evaluate the project output expressions, preferring the compiled
    /// closure tree over the scalar chunk path.
    fn evaluate_project_expressions(
        chunk: &mut DataChunk,
        output_expressions: &[Expression],
        state: &mut UnaryOperatorState,
    ) -> Result<Vec<Vec<Value>>, QueryError> {
        if compiled_eval_enabled() {
            if state.compiled_project.is_none() {
                let layout = chunk.get_layout();
                state.compiled_project = Some(
                    output_expressions
                        .iter()
                        .map(|e| CompiledExpr::compile(e, &layout))
                        .collect(),
                );
            }
            if let Some(compiled) = &state.compiled_project {
                let layout = chunk.get_layout();
                let len = chunk.rows.len();
                let mut columns = Vec::with_capacity(compiled.len());
                let mut ok = true;
                for expr in compiled {
                    match expr.evaluate_batch(&chunk.rows, layout.clone(), Some(&state.env)) {
                        Ok(col) => columns.push(col.into_values(len)),
                        Err(_) => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    return Ok(columns);
                }
            }
        }
        chunk
            .evaluate_expressions(output_expressions, Some(&state.env))
            .map_err(|e| {
                QueryError::execution(format!("Project expression evaluation failed: {}", e))
            })
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
                        let results =
                            Self::evaluate_filter_predicate(&mut chunk, predicate, state)?;
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
                    let columns =
                        Self::evaluate_project_expressions(&mut chunk, output_expressions, state)?;
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
                    // consume offset/limit directly on the visible rows.
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
                                vec![Value::Null(graphdb_core::value::NullType::Null); chunk.len()]
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
                            // boundary materialization.
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
                            Err(_) => Value::Null(graphdb_core::NullType::Null),
                        };
                        let vid =
                            match graphdb_core::types::storage_ids::VertexId::try_from(&entity) {
                                Ok(vid) => vid,
                                Err(_) => {
                                    new_row.push(Value::Null(graphdb_core::NullType::Null));
                                    result_rows.push(new_row);
                                    continue;
                                }
                            };
                        match guard.get_vertex_projected(space_name, &vid, prop_names) {
                            Ok(Some(vertex)) => {
                                if flat {
                                    for prop in prop_names.iter() {
                                        new_row.push(vertex.property_value(prop).unwrap_or_else(
                                            || Value::Null(graphdb_core::NullType::Null),
                                        ));
                                    }
                                } else {
                                    new_row.push(Value::Vertex(Box::new(vertex)));
                                }
                            }
                            Ok(None) => {
                                if flat {
                                    for _ in prop_names.iter() {
                                        new_row.push(Value::Null(graphdb_core::NullType::Null));
                                    }
                                } else {
                                    new_row.push(Value::Null(graphdb_core::NullType::Null));
                                }
                            }
                            Err(_) => {
                                new_row.push(Value::Null(graphdb_core::NullType::Null));
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
                            // boundary materialization.
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
            UnaryOperatorKind::Flatten {
                // Plan level group identifier, carried end to end
                // (plan -> spec -> operator) and visible in EXPLAIN /
                // cbo_notes. The streaming engine stores flat row batches
                // only, so flatten currently replays the child selection
                // without group aware column projection; the position is
                // logged for traceability until the compressed
                // representation lands.
                group_pos,
                current_idx,
                size_to_flatten,
                saved_sel_vector,
                buffered_chunk,
                batch_size,
            } => {
                log::debug!("flatten: replaying selection for group {group_pos}");
                if *batch_size <= 1 {
                    crate::executor::streaming::operators::flatten::flatten_next_inner(
                        current_idx,
                        size_to_flatten,
                        saved_sel_vector,
                        buffered_chunk,
                        input,
                    )
                } else {
                    crate::executor::streaming::operators::flatten::flatten_next_batch(
                        current_idx,
                        size_to_flatten,
                        saved_sel_vector,
                        buffered_chunk,
                        input,
                        *batch_size,
                    )
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
            UnaryOperatorKind::Flatten {
                current_idx,
                size_to_flatten,
                saved_sel_vector,
                buffered_chunk,
                batch_size,
                ..
            } => {
                *current_idx = 0;
                *size_to_flatten = 0;
                *saved_sel_vector = None;
                *buffered_chunk = None;
                *batch_size =
                    crate::executor::streaming::operators::flatten::DEFAULT_FLATTEN_BATCH_SIZE;
            }
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
        if let UnaryOperatorKind::Flatten { buffered_chunk, .. } = &mut self.kind {
            if let Some(chunk) = buffered_chunk.take() {
                if let Some(stats) = chunk.columnar_stats {
                    stats.record_selection_materialized();
                }
            }
        }
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
    use crate::executor::base::MemoryBudget;
    use crate::executor::streaming::operators::base::OperatorBase;
    use crate::executor::streaming::operators::source_operator::SourceOperator;
    use crate::executor::streaming::operators::source_operator::SourceOperatorKind;
    use crate::executor::streaming::runtime::{ExecutionRuntime, QueryIdentity};
    use crate::storage::StorageWriter;
    use graphdb_core::types::storage_ids::VertexId;
    use graphdb_core::{Tag, Vertex};
    use parking_lot::RwLock;

    fn runtime_with_storage(
        storage: Arc<RwLock<dyn crate::storage::QueryStorage>>,
    ) -> Arc<ExecutionRuntime> {
        Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            Some(storage),
            crate::executor::base::SearchContext::default(),
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
    fn flatten_group_pos_is_observable() {
        let kind = UnaryOperatorKind::Flatten {
            group_pos: 7,
            current_idx: 0,
            size_to_flatten: 0,
            saved_sel_vector: None,
            buffered_chunk: None,
            batch_size: crate::executor::streaming::operators::flatten::DEFAULT_FLATTEN_BATCH_SIZE,
        };
        assert_eq!(kind.flatten_group_pos(), Some(7));
        let filter = UnaryOperatorKind::Limit {
            offset: 0,
            limit: 1,
            skipped: 0,
            consumed: 0,
        };
        assert_eq!(filter.flatten_group_pos(), None);
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
