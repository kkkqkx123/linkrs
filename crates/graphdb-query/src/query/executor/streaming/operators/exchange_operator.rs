use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Instant;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::{
    SortDirection, StreamingExecutor, ValueRowContext,
};
use crate::query::executor::streaming::helpers::compare_values;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::spec::ExchangeSpec;
use crate::query::executor::streaming::operators::state::{ExchangeState, MergeInputState};
use crate::query::executor::streaming::pool::{PartitionBatch, PartitionHandle};
use crate::query::executor::streaming::slot::SlotLayout;

const CHUNK_SIZE: usize = 1024;

#[derive(Debug)]
pub struct ExchangeOperator {
    pub state: ExchangeState,
    pub(crate) handle: Option<PartitionHandle>,
}

impl ExchangeOperator {
    pub fn from_spec(spec: &ExchangeSpec) -> Self {
        let state = match spec {
            ExchangeSpec::Concatenate { .. } => ExchangeState::Concatenate {
                current_index: 0,
                col_names: None,
            },
            ExchangeSpec::MergeSort {
                sort_expressions,
                sort_directions,
                limit,
            } => ExchangeState::MergeSort {
                sort_expressions: sort_expressions.clone(),
                sort_directions: sort_directions.clone(),
                inputs: Vec::new(),
                col_names: None,
                limit: *limit,
                emitted: 0,
            },
        };
        Self {
            state,
            handle: None,
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        children: &mut Vec<StreamingExecutor>,
    ) -> Result<(), QueryError> {
        match &mut self.state {
            ExchangeState::Concatenate { .. } => {}
            ExchangeState::MergeSort {
                inputs, emitted, ..
            } => {
                *inputs = (0..children.len())
                    .map(|_| MergeInputState::Pending)
                    .collect();
                *emitted = 0;
            }
        }

        let runtime = base.runtime.clone();
        if let Some(rt) = &runtime {
            let pool = rt.worker_pool.lock().clone();
            if let Some(pool) = pool {
                if children.len() > 1 && pool.max_workers() > 1 {
                    let max_buffered = base.chunk_size.clamp(1, 10);
                    let (batch, receivers, error_rx) =
                        PartitionBatch::new(std::mem::take(children), rt.clone(), max_buffered);
                    let batch = Arc::new(batch);
                    let handle = PartitionHandle::from_batch(
                        &batch,
                        receivers,
                        error_rx,
                        rt.clone(),
                        Instant::now(),
                        pool.max_workers(),
                    );
                    pool.submit(batch);
                    self.handle = Some(handle);
                    base.lifecycle.mark_opened();
                    return Ok(());
                }
            }
        }

        for child in children.iter_mut() {
            child.open()?;
        }
        base.lifecycle.mark_opened();
        Ok(())
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        children: &mut Vec<StreamingExecutor>,
    ) -> Result<Option<DataChunk>, QueryError> {
        base.ensure_not_cancelled()?;
        match &mut self.state {
            ExchangeState::Concatenate {
                current_index,
                col_names,
            } => {
                while *current_index < input_count(children, &self.handle) {
                    base.ensure_not_cancelled()?;
                    if let Some(chunk) = advance_input(children, &mut self.handle, *current_index)?
                    {
                        validate_schema(*current_index, &chunk, col_names)?;
                        return Ok(Some(chunk));
                    }
                    *current_index += 1;
                }
                Ok(None)
            }
            ExchangeState::MergeSort {
                sort_expressions,
                sort_directions,
                inputs,
                col_names,
                limit,
                emitted,
            } => {
                if limit.is_some_and(|value| *emitted >= value) {
                    return Ok(None);
                }

                let mut result_rows = Vec::with_capacity(CHUNK_SIZE);
                while result_rows.len() < CHUNK_SIZE {
                    base.ensure_not_cancelled()?;
                    if limit.is_some_and(|value| *emitted >= value) {
                        break;
                    }
                    match next_merge_row(
                        base,
                        children,
                        &mut self.handle,
                        sort_expressions,
                        sort_directions,
                        inputs,
                        col_names,
                    )? {
                        Some(row) => {
                            result_rows.push(row);
                            *emitted += 1;
                        }
                        None => break,
                    }
                }

                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    let layout =
                        Arc::new(SlotLayout::from_names(col_names.as_deref().unwrap_or(&[])));
                    Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
                }
            }
        }
    }

    pub fn stop(
        &mut self,
        _base: &mut OperatorBase,
        children: &mut Vec<StreamingExecutor>,
    ) -> Result<(), QueryError> {
        if let Some(handle) = self.handle.as_mut() {
            return handle.stop_and_join();
        }
        stop_children(children)
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        children: &mut Vec<StreamingExecutor>,
    ) -> Result<(), QueryError> {
        if let Some(handle) = self.handle.as_mut() {
            let _ = handle.stop_and_join();
            self.handle = None;
        }
        base.lifecycle.mark_closed();
        close_children(children).map_or(Ok(()), Err)
    }
}

fn input_count(serial: &[StreamingExecutor], handle: &Option<PartitionHandle>) -> usize {
    handle
        .as_ref()
        .map(|h| h.partition_count)
        .unwrap_or(serial.len())
}

fn advance_input(
    children: &mut Vec<StreamingExecutor>,
    handle: &mut Option<PartitionHandle>,
    index: usize,
) -> Result<Option<DataChunk>, QueryError> {
    if let Some(handle) = handle {
        handle.next_for_partition(index)
    } else if index < children.len() {
        children[index].advance()
    } else {
        Ok(None)
    }
}

fn next_merge_row(
    base: &OperatorBase,
    children: &mut Vec<StreamingExecutor>,
    handle: &mut Option<PartitionHandle>,
    sort_expressions: &[Expression],
    sort_directions: &[SortDirection],
    inputs: &mut [MergeInputState],
    col_names: &mut Option<Vec<String>>,
) -> Result<Option<Vec<Value>>, QueryError> {
    let mut best_child = None;

    for index in 0..input_count(children, handle) {
        base.ensure_not_cancelled()?;
        fill_input(index, children, handle, inputs, col_names)?;
        let MergeInputState::Buffered { chunk, row_index } = &inputs[index] else {
            continue;
        };
        let row = chunk.rows.get(*row_index).ok_or_else(|| {
            QueryError::execution("Exchange merge input has an invalid row index".to_string())
        })?;

        let is_better = match best_child {
            None => true,
            Some(best_index) => {
                let MergeInputState::Buffered {
                    chunk: best_chunk,
                    row_index: best_row_index,
                } = &inputs[best_index]
                else {
                    return Err(QueryError::execution(
                        "Exchange merge state changed while selecting a row".to_string(),
                    ));
                };
                let best_row = best_chunk.rows.get(*best_row_index).ok_or_else(|| {
                    QueryError::execution(
                        "Exchange merge input has an invalid best-row index".to_string(),
                    )
                })?;
                compare_rows(
                    row,
                    best_row,
                    sort_expressions,
                    sort_directions,
                    col_names.as_deref().unwrap_or_default(),
                )? == Ordering::Less
            }
        };
        if is_better {
            best_child = Some(index);
        }
    }

    let Some(index) = best_child else {
        return Ok(None);
    };
    let MergeInputState::Buffered { chunk, row_index } = &mut inputs[index] else {
        return Err(QueryError::execution(
            "Exchange selected a non-buffered input".to_string(),
        ));
    };
    let row = chunk.rows.get(*row_index).cloned().ok_or_else(|| {
        QueryError::execution("Exchange merge input has an invalid selected row index".to_string())
    })?;
    *row_index += 1;
    if *row_index >= chunk.rows.len() {
        inputs[index] = MergeInputState::Pending;
    }
    Ok(Some(row))
}

fn fill_input(
    index: usize,
    children: &mut Vec<StreamingExecutor>,
    handle: &mut Option<PartitionHandle>,
    inputs: &mut [MergeInputState],
    col_names: &mut Option<Vec<String>>,
) -> Result<(), QueryError> {
    if !matches!(inputs[index], MergeInputState::Pending) {
        return Ok(());
    }

    loop {
        match advance_input(children, handle, index)? {
            Some(chunk) if chunk.is_empty() => continue,
            Some(chunk) => {
                validate_schema(index, &chunk, col_names)?;
                inputs[index] = MergeInputState::Buffered {
                    chunk,
                    row_index: 0,
                };
                return Ok(());
            }
            None => {
                inputs[index] = MergeInputState::Exhausted;
                return Ok(());
            }
        }
    }
}

fn compare_rows(
    a: &[Value],
    b: &[Value],
    sort_expressions: &[Expression],
    sort_directions: &[SortDirection],
    col_names: &[String],
) -> Result<Ordering, QueryError> {
    let layout = Arc::new(SlotLayout::from_names(col_names));
    for (index, expression) in sort_expressions.iter().enumerate() {
        let direction = sort_directions
            .get(index)
            .copied()
            .unwrap_or(SortDirection::Ascending);
        let mut left_context = ValueRowContext::new(a.to_vec(), layout.clone());
        let mut right_context = ValueRowContext::new(b.to_vec(), layout.clone());
        let left =
            ExpressionEvaluator::evaluate(expression, &mut left_context).map_err(|error| {
                QueryError::execution(format!(
                    "Exchange failed to evaluate left sort key: {error}"
                ))
            })?;
        let right =
            ExpressionEvaluator::evaluate(expression, &mut right_context).map_err(|error| {
                QueryError::execution(format!(
                    "Exchange failed to evaluate right sort key: {error}"
                ))
            })?;
        let comparison = match direction {
            SortDirection::Ascending => compare_values(&left, &right),
            SortDirection::Descending => compare_values(&left, &right).reverse(),
        };
        if comparison != Ordering::Equal {
            return Ok(comparison);
        }
    }
    Ok(Ordering::Equal)
}

fn validate_schema(
    partition_id: usize,
    chunk: &DataChunk,
    col_names: &mut Option<Vec<String>>,
) -> Result<(), QueryError> {
    let child_columns = chunk.col_names();
    if let Some(expected) = col_names.as_ref() {
        if expected != &child_columns {
            return Err(QueryError::execution(format!(
                "Exchange schema mismatch in partition {}: expected {:?}, got {:?}",
                partition_id, expected, child_columns
            )));
        }
    } else {
        *col_names = Some(child_columns);
    }
    Ok(())
}

fn close_children(children: &mut [StreamingExecutor]) -> Option<QueryError> {
    let mut first_error = None;
    for child in children.iter_mut() {
        if let Err(error) = child.close() {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    first_error
}

fn stop_children(children: &mut [StreamingExecutor]) -> Result<(), QueryError> {
    let mut first_error = None;
    for child in children.iter_mut() {
        if let Err(error) = child.stop() {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}
