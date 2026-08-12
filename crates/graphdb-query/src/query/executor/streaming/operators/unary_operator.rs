use std::collections::HashMap;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::{selection_propagation_enabled, DataChunk};
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::executor::ValueRowContext;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::slot::SlotLayout;

#[derive(Debug, Default)]
pub struct UnaryOperatorState {
    pub parameters: Option<Arc<HashMap<String, Value>>>,
}

#[derive(Debug)]
pub enum UnaryOperator {
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

impl UnaryOperator {
    /// Create a UnaryOperator with fresh mutable state from an immutable spec.
    pub fn from_spec(spec: &super::spec::UnarySpec) -> Self {
        let state = UnaryOperatorState { parameters: None };
        match spec {
            super::spec::UnarySpec::Filter { predicate } => Self::Filter {
                predicate: predicate.clone(),
                state,
            },
            super::spec::UnarySpec::Project {
                output_expressions,
                output_col_names,
            } => Self::Project {
                output_expressions: output_expressions.clone(),
                output_col_names: output_col_names.clone(),
                state,
            },
            super::spec::UnarySpec::Limit { offset, limit } => Self::Limit {
                offset: *offset,
                limit: *limit,
                skipped: 0,
                consumed: 0,
            },
            super::spec::UnarySpec::Assign { assignments } => Self::Assign {
                assignments: assignments.clone(),
                state,
            },
            super::spec::UnarySpec::Remove { columns_to_remove } => Self::Remove {
                columns_to_remove: columns_to_remove.clone(),
            },
            super::spec::UnarySpec::Unwind {
                unwind_column,
                list_expression,
            } => Self::Unwind {
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
            } => Self::AppendVertices {
                entity_var: entity_var.clone(),
                entity_expr: entity_expr.clone(),
                prop_names: prop_names.clone(),
                storage: None,
                space_name: space_name.clone(),
                state,
            },
            super::spec::UnarySpec::Sample { count } => Self::Sample {
                count: *count,
                consumed: 0,
            },
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        let params = base.runtime.as_ref().and_then(|rt| rt.parameter_values());
        let storage = base.runtime.as_ref().and_then(|rt| rt.storage.clone());
        match self {
            Self::Filter { state, .. }
            | Self::Project { state, .. }
            | Self::Assign { state, .. }
            | Self::AppendVertices { state, .. } => {
                state.parameters = params;
            }
            _ => {}
        }
        if let Self::AppendVertices {
            storage: target, ..
        } = self
        {
            *target = storage;
        }
        match self {
            Self::Filter { .. }
            | Self::Project { .. }
            | Self::Limit { .. }
            | Self::Dedup { .. }
            | Self::Assign { .. }
            | Self::Remove { .. }
            | Self::Unwind { .. }
            | Self::AppendVertices { .. }
            | Self::Sample { .. } => {
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
            Self::Filter { predicate, state } => loop {
                match input.advance()? {
                    Some(mut chunk) => {
                        let results = chunk
                            .evaluate_expression(predicate, state.parameters.as_ref())
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
            Self::Project {
                output_expressions,
                output_col_names: _,
                state,
            } => loop {
                if let Some(mut chunk) = input.advance()? {
                    let params = state.parameters.as_ref();
                    // When the child carries a selection vector, evaluate
                    // each output expression only for the visible rows — the
                    // output chunk is fully materialized (small).
                    if chunk.selection().is_some() {
                        let mut columns = Vec::with_capacity(output_expressions.len());
                        for expr in output_expressions.iter() {
                            let col =
                                chunk
                                    .evaluate_expression_visible(expr, params)
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
                                Arc::clone(&base.output_layout),
                            )));
                        }
                        continue;
                    }
                    let columns = chunk
                        .evaluate_expressions(output_expressions, params)
                        .map_err(|e| {
                            QueryError::execution(format!(
                                "Project expression evaluation failed: {}",
                                e
                            ))
                        })?;
                    if !columns.is_empty() && !columns[0].is_empty() {
                        return Ok(Some(DataChunk::from_columns(
                            columns,
                            Arc::clone(&base.output_layout),
                        )));
                    }
                } else {
                    return Ok(None);
                }
            },
            Self::Limit {
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
            Self::Dedup { seen_rows } => {
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
                            Arc::clone(&base.output_layout),
                        )));
                    }
                }
                Ok(None)
            }
            Self::Assign { assignments, state } => loop {
                if let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("Assign");
                    let params = state.parameters.as_ref();
                    // Batch-evaluate all assignment expressions first
                    let mut new_cols: Vec<Vec<Value>> = Vec::with_capacity(assignments.len());
                    for (_col_name, expr) in assignments.iter() {
                        let col = match chunk.evaluate_expression(expr, params) {
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
                            Arc::clone(&base.output_layout),
                        )));
                    }
                } else {
                    return Ok(None);
                }
            },
            Self::Remove { columns_to_remove } => loop {
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
                            Arc::clone(&base.output_layout),
                        )));
                    }
                } else {
                    return Ok(None);
                }
            },
            Self::Unwind {
                unwind_column,
                list_expression,
                col_index,
                layout,
                all_rows,
                current_row_index,
                current_unwind_index,
                input_done,
            } => loop {
                base.ensure_not_cancelled()?;
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
                        Arc::clone(&base.output_layout),
                    )));
                }
                *current_row_index += 1;
                *current_unwind_index = 0;
            },
            Self::AppendVertices {
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
                        let mut ctx = if let Some(ref params) = state.parameters {
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
                        let vid = match crate::core::types::storage_ids::VertexId::try_from(
                            &entity,
                        ) {
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
                                        new_row.push(
                                            vertex
                                                .property_value(prop)
                                                .unwrap_or_else(|| {
                                                    Value::Null(crate::core::NullType::Null)
                                                }),
                                        );
                                    }
                                } else {
                                    new_row.push(Value::Vertex(Box::new(vertex)));
                                }
                            }
                            Ok(None) => {
                                if flat {
                                    for _ in prop_names.iter() {
                                        new_row.push(Value::Null(
                                            crate::core::NullType::Null,
                                        ));
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
                            Arc::clone(&base.output_layout),
                        )));
                    }
                } else {
                    return Ok(None);
                }
            },
            Self::Sample { count, consumed } => {
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
                                    Arc::clone(&base.output_layout),
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

    pub fn stop(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        base.lifecycle.mark_stopped();
        Ok(())
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        if base.lifecycle.can_close() {
            base.lifecycle.mark_closed();
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
    use crate::core::types::storage_ids::VertexId;
    use crate::core::{Tag, Vertex};
    use crate::query::executor::base::MemoryBudget;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;
    use crate::query::executor::streaming::runtime::{ExecutionRuntime, QueryIdentity};
    use crate::query::executor::streaming::slot::SlotLayout;
    use crate::storage::StorageWriter;
    use parking_lot::RwLock;

    fn runtime_with_storage(storage: Arc<RwLock<dyn crate::storage::QueryStorage>>) -> Arc<ExecutionRuntime> {
        Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            Some(storage),
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "qdrant")]
            None,
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
        let input = Box::new(StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                buffer: vec![vec![Value::string("1")]],
                current_index: 0,
                col_names: vec!["vid".to_string()],
            },
        ));
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::AppendVertices {
                entity_var: "v".to_string(),
                entity_expr: Expression::Variable("vid".to_string()),
                prop_names: vec!["name".to_string(), "age".to_string()],
                storage: None,
                space_name: "test".to_string(),
                state: UnaryOperatorState { parameters: None },
            },
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

        let input = Box::new(StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                buffer: vec![vec![Value::Int(7)]],
                current_index: 0,
                col_names: vec!["vid".to_string()],
            },
        ));
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::AppendVertices {
                entity_var: "v".to_string(),
                entity_expr: Expression::Variable("vid".to_string()),
                prop_names: vec![],
                storage: None,
                space_name: "test".to_string(),
                state: UnaryOperatorState { parameters: None },
            },
        );
        append.base_mut().runtime = Some(runtime_with_storage(storage.clone()));
        append.open().expect("open should succeed");
        let chunk = append.advance().expect("advance").expect("one chunk");
        let row = &chunk.rows[0];
        assert_eq!(row.len(), 2, "vid + full vertex");
        match &row[1] {
            Value::Vertex(vertex) => {
                assert_eq!(vertex.vid, VertexId::from_int64(7));
                assert_eq!(
                    vertex.property_value("name"),
                    Some(Value::string("Bob"))
                );
            }
            other => panic!("expected full vertex, got {:?}", other),
        }
    }

    #[test]
    fn append_vertices_missing_vertex_yields_null_columns() {
        let storage: Arc<RwLock<dyn crate::storage::QueryStorage>> =
            Arc::new(RwLock::new(
                crate::storage::MockStorage::new().expect("MockStorage should be created"),
            ));
        let input = Box::new(StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                buffer: vec![vec![Value::string("missing")]],
                current_index: 0,
                col_names: vec!["vid".to_string()],
            },
        ));
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::AppendVertices {
                entity_var: "v".to_string(),
                entity_expr: Expression::Variable("vid".to_string()),
                prop_names: vec!["name".to_string()],
                storage: None,
                space_name: "test".to_string(),
                state: UnaryOperatorState { parameters: None },
            },
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
        let input = Box::new(StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                buffer: vec![vec![Value::Int(1)]],
                current_index: 0,
                col_names: vec!["vid".to_string()],
            },
        ));
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::AppendVertices {
                entity_var: "v".to_string(),
                entity_expr: Expression::Variable("vid".to_string()),
                prop_names: vec!["name".to_string()],
                storage: None,
                space_name: "test".to_string(),
                state: UnaryOperatorState { parameters: None },
            },
        );
        append.open().expect("open should succeed");
        let error = append.advance().expect_err("storage required");
        assert!(error.to_string().contains("requires storage"));
    }
}
