use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::context::BorrowedRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::executor::ValueRowContext;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::slot::SlotLayout;

#[derive(Debug)]
pub enum UnaryOperator {
    Filter {
        predicate: Expression,
    },
    Project {
        output_expressions: Vec<Expression>,
        output_col_names: Vec<String>,
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
    },
    Remove {
        columns_to_remove: Vec<String>,
    },
    Unwind {
        unwind_column: String,
        col_index: Option<usize>,
        all_rows: Vec<Vec<Value>>,
        current_row_index: usize,
        current_unwind_index: usize,
    },
    AppendVertices {
        vertex_properties: Vec<(String, Expression)>,
    },
    Sample {
        count: u64,
        consumed: u64,
    },
}

impl UnaryOperator {
    /// Create a UnaryOperator with fresh mutable state from an immutable spec.
    pub fn from_spec(spec: &super::spec::UnarySpec) -> Self {
        match spec {
            super::spec::UnarySpec::Filter { predicate } => Self::Filter {
                predicate: predicate.clone(),
            },
            super::spec::UnarySpec::Project {
                output_expressions,
                output_col_names,
            } => Self::Project {
                output_expressions: output_expressions.clone(),
                output_col_names: output_col_names.clone(),
            },
            super::spec::UnarySpec::Limit { offset, limit } => Self::Limit {
                offset: *offset,
                limit: *limit,
                skipped: 0,
                consumed: 0,
            },
            super::spec::UnarySpec::Assign { assignments } => Self::Assign {
                assignments: assignments.clone(),
            },
            super::spec::UnarySpec::Remove { columns_to_remove } => Self::Remove {
                columns_to_remove: columns_to_remove.clone(),
            },
            super::spec::UnarySpec::Unwind { unwind_column } => Self::Unwind {
                unwind_column: unwind_column.clone(),
                col_index: None,
                all_rows: Vec::new(),
                current_row_index: 0,
                current_unwind_index: 0,
            },
            super::spec::UnarySpec::AppendVertices { vertex_properties } => Self::AppendVertices {
                vertex_properties: vertex_properties.clone(),
            },
            super::spec::UnarySpec::Sample { count } => Self::Sample {
                count: *count,
                consumed: 0,
            },
        }
    }

    pub fn open(
        &mut self,
        _base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
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
                _base.lifecycle.mark_opened();
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        _base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::Filter { predicate } => loop {
                match input.advance()? {
                    Some(mut chunk) => {
                        let layout = chunk.get_layout();
                        let mut selected = Vec::new();
                        for i in 0..chunk.len() {
                            let row = &chunk.rows[i];
                            let mut context = BorrowedRowContext::new(row, layout.clone());
                            let keep = match ExpressionEvaluator::evaluate(predicate, &mut context)
                            {
                                Ok(value) => match value {
                                    Value::Bool(b) => b,
                                    Value::Null(_) => false,
                                    Value::Int(i) => i != 0,
                                    Value::BigInt(i) => i != 0,
                                    Value::Float(f) => f != 0.0,
                                    Value::Double(f) => f != 0.0,
                                    Value::String(s) => !s.is_empty(),
                                    _ => true,
                                },
                                Err(e) => {
                                    return Err(QueryError::execution(format!(
                                        "Filter predicate evaluation failed: {}",
                                        e
                                    )));
                                }
                            };
                            if keep {
                                selected.push(i);
                            }
                        }
                        if !selected.is_empty() {
                            return Ok(Some(chunk.take_indices(&selected)));
                        }
                    }
                    None => return Ok(None),
                }
            },
            Self::Project {
                output_expressions,
                output_col_names,
            } => {
                if let Some(chunk) = input.advance()? {
                    let input_col_names = chunk.col_names();
                    let input_layout = chunk.get_layout();
                    let output_col_names_final: Vec<String> = if output_col_names.is_empty() {
                        input_col_names.clone()
                    } else {
                        output_col_names.clone()
                    };
                    let layout = Arc::new(SlotLayout::from_names(&output_col_names_final));
                    let mut projected_rows = Vec::new();
                    for row in chunk.rows {
                        let mut context = ValueRowContext::new(row, input_layout.clone());
                        let mut projected_row = Vec::new();
                        for expr in output_expressions.iter() {
                            match ExpressionEvaluator::evaluate(expr, &mut context) {
                                Ok(value) => projected_row.push(value),
                                Err(e) => {
                                    return Err(QueryError::execution(format!(
                                        "Project expression evaluation failed: {}",
                                        e
                                    )));
                                }
                            }
                        }
                        projected_rows.push(projected_row);
                    }
                    Ok(Some(DataChunk::new_with_layout(projected_rows, layout)))
                } else {
                    Ok(None)
                }
            }
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
                    return Ok(Some(chunk));
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
                        return Ok(Some(DataChunk::from_rows(result_rows)));
                    }
                }
                Ok(None)
            }
            Self::Assign { assignments } => {
                if let Some(chunk) = input.advance()? {
                    let layout = chunk.get_layout();
                    let mut result_rows = vec![];
                    for row in chunk.rows {
                        let mut new_row = row.clone();
                        for (_col_name, expr) in assignments.iter() {
                            let mut context = ValueRowContext::new(row.clone(), layout.clone());
                            match ExpressionEvaluator::evaluate(expr, &mut context) {
                                Ok(val) => new_row.push(val),
                                Err(_) => {
                                    new_row.push(Value::Null(crate::core::value::NullType::Null))
                                }
                            }
                        }
                        result_rows.push(new_row);
                    }
                    Ok(Some(DataChunk::from_rows(result_rows)))
                } else {
                    Ok(None)
                }
            }
            Self::Remove { columns_to_remove } => {
                if let Some(chunk) = input.advance()? {
                    let col_names = chunk.col_names();
                    let mut new_col_names = vec![];
                    let mut indices_to_keep = vec![];
                    for (idx, col_name) in col_names.iter().enumerate() {
                        if !columns_to_remove.contains(col_name) {
                            new_col_names.push(col_name.clone());
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
                    Ok(Some(DataChunk::from_rows(result_rows)))
                } else {
                    Ok(None)
                }
            }
            Self::Unwind {
                unwind_column,
                col_index,
                all_rows,
                current_row_index,
                current_unwind_index,
            } => {
                while *current_row_index < all_rows.len() || {
                    if let Some(chunk) = input.advance()? {
                        let col_names = chunk.col_names();
                        let idx = col_names.iter().position(|c| c == unwind_column.as_str());
                        *col_index = idx;
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
                                    return Ok(Some(DataChunk::from_rows(vec![result_row])));
                                }
                            }
                        }
                    }
                    *current_row_index += 1;
                    *current_unwind_index = 0;
                }
                Ok(None)
            }
            Self::AppendVertices { vertex_properties } => {
                if let Some(chunk) = input.advance()? {
                    let layout = chunk.get_layout();
                    let mut result_rows = Vec::new();
                    for row in chunk.rows {
                        let mut new_row = row.clone();
                        let mut ctx = ValueRowContext::new(row.clone(), layout.clone());
                        for (_prop_name, expr) in vertex_properties.iter() {
                            match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                Ok(val) => new_row.push(val),
                                Err(_) => new_row.push(Value::Null(crate::core::NullType::Null)),
                            }
                        }
                        result_rows.push(new_row);
                    }
                    Ok(Some(DataChunk::from_rows(result_rows)))
                } else {
                    Ok(None)
                }
            }
            Self::Sample { count, consumed } => {
                if *consumed >= *count {
                    return Ok(None);
                }
                match input.advance()? {
                    Some(chunk) => {
                        let remaining = (*count - *consumed) as usize;
                        let layout = chunk.get_layout();
                        let take_count = chunk.rows.len().min(remaining);
                        let rows: Vec<Vec<Value>> =
                            chunk.rows.into_iter().take(take_count).collect();
                        *consumed += take_count as u64;
                        if !rows.is_empty() {
                            Ok(Some(DataChunk::new_with_layout(rows, layout)))
                        } else {
                            Ok(None)
                        }
                    }
                    None => Ok(None),
                }
            }
        }
    }

    pub fn stop(
        &mut self,
        _base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::Filter { .. }
            | Self::Project { .. }
            | Self::Limit { .. }
            | Self::Dedup { .. }
            | Self::Assign { .. }
            | Self::Remove { .. }
            | Self::Unwind { .. }
            | Self::AppendVertices { .. }
            | Self::Sample { .. } => input.stop(),
        }
    }

    pub fn close(
        &mut self,
        _base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::Filter { .. }
            | Self::Project { .. }
            | Self::Limit { .. }
            | Self::Dedup { .. }
            | Self::Unwind { .. }
            | Self::AppendVertices { .. }
            | Self::Sample { .. } => {
                if _base.lifecycle.can_close() {
                    input.close()?;
                    _base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::Assign { .. } | Self::Remove { .. } => {
                if _base.lifecycle.can_close() {
                    input.close()?;
                    _base.lifecycle.mark_closed();
                }
                Ok(())
            }
        }
    }
}
