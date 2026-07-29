use std::collections::HashMap;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
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
        col_index: Option<usize>,
        layout: Option<Arc<SlotLayout>>,
        all_rows: Vec<Vec<Value>>,
        current_row_index: usize,
        current_unwind_index: usize,
    },
    AppendVertices {
        vertex_properties: Vec<(String, Expression)>,
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
            super::spec::UnarySpec::Unwind { unwind_column } => Self::Unwind {
                unwind_column: unwind_column.clone(),
                col_index: None,
                layout: None,
                all_rows: Vec::new(),
                current_row_index: 0,
                current_unwind_index: 0,
            },
            super::spec::UnarySpec::AppendVertices { vertex_properties } => Self::AppendVertices {
                vertex_properties: vertex_properties.clone(),
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
        match self {
            Self::Filter { state, .. }
            | Self::Project { state, .. }
            | Self::Assign { state, .. }
            | Self::AppendVertices { state, .. } => {
                state.parameters = params;
            }
            _ => {}
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
                        let mut selected = Vec::new();
                        for (i, val) in results.into_iter().enumerate() {
                            if matches_value(&val) {
                                selected.push(i);
                            }
                        }
                        if !selected.is_empty() {
                            let selected_chunk = chunk.take_indices(&selected);
                            return Ok(Some(selected_chunk));
                        }
                    }
                    None => return Ok(None),
                }
            },
            Self::Project {
                output_expressions,
                output_col_names: _,
                state,
            } => loop {
                if let Some(chunk) = input.advance()? {
                    let params = state.parameters.as_ref();
                    let mut columns = Vec::with_capacity(output_expressions.len());
                    for expr in output_expressions.iter() {
                        let col = chunk.evaluate_expression(expr, params).map_err(|e| {
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

                    if *skipped < *offset {
                        let remaining_offset = (*offset - *skipped) as usize;
                        let rows_to_skip = remaining_offset.min(chunk.rows.len());
                        chunk.rows.drain(..rows_to_skip);
                        *skipped += rows_to_skip as u32;
                    }

                    if chunk.rows.is_empty() {
                        continue;
                    }

                    let remaining_limit = (*limit - *consumed) as usize;
                    if chunk.rows.len() > remaining_limit {
                        chunk.rows.truncate(remaining_limit);
                    }
                    *consumed += chunk.rows.len() as u32;
                    return Ok(Some(DataChunk::new_with_layout(
                        chunk.rows,
                        Arc::clone(&base.output_layout),
                    )));
                }
            }
            Self::Dedup { seen_rows } => {
                while let Some(chunk) = input.advance()? {
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
                    let params = state.parameters.as_ref();
                    // Batch-evaluate all assignment expressions first
                    let mut new_cols: Vec<Vec<Value>> = Vec::with_capacity(assignments.len());
                    for (_col_name, expr) in assignments.iter() {
                        let col = match chunk.evaluate_expression(expr, params) {
                            Ok(col) => col,
                            Err(_) => vec![Value::Null(crate::core::value::NullType::Null); chunk.len()],
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
                        // Invalidate columnar cache since rows changed
                        chunk.columns = None;
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
                if let Some(chunk) = input.advance()? {
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
                col_index,
                layout,
                all_rows,
                current_row_index,
                current_unwind_index,
            } => {
                while *current_row_index < all_rows.len() || {
                    if let Some(chunk) = input.advance()? {
                        let col_names = chunk.col_names();
                        let idx = col_names.iter().position(|c| c == unwind_column.as_str());
                        *col_index = idx;
                        *layout = Some(chunk.get_layout());
                        *all_rows = chunk.rows;
                        *current_row_index = 0;
                        *current_unwind_index = 0;
                        true
                    } else {
                        false
                    }
                } {
                    if *current_row_index >= all_rows.len() {
                        break;
                    }
                    let row = &all_rows[*current_row_index];
                    if let Some(idx) = col_index {
                        if *idx < row.len() {
                            let list_val = &row[*idx];
                            if let Value::List(items) = list_val {
                                if *current_unwind_index < items.len() {
                                    let mut result_row = row.clone();
                                    result_row[*idx] = items[*current_unwind_index].clone();
                                    *current_unwind_index += 1;
                                    return Ok(Some(DataChunk::new_with_layout(
                                        vec![result_row],
                                        Arc::clone(&base.output_layout),
                                    )));
                                }
                            }
                        }
                    }
                    *current_row_index += 1;
                    *current_unwind_index = 0;
                }
                Ok(None)
            }
            Self::AppendVertices {
                vertex_properties,
                state,
            } => loop {
                if let Some(chunk) = input.advance()? {
                    let layout = chunk.get_layout();
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
                        for (_prop_name, expr) in vertex_properties.iter() {
                            match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                Ok(val) => new_row.push(val),
                                Err(_) => new_row.push(Value::Null(crate::core::NullType::Null)),
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
                        Some(chunk) => {
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
