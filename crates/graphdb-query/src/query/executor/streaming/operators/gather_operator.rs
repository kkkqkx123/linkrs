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
use crate::query::executor::streaming::pool::{PartitionBatch, PartitionHandle};
use crate::query::executor::streaming::slot::SlotLayout;

const CHUNK_SIZE: usize = 1024;

/// Internal merge cursor state. It is public only because `GatherOperator` is
/// a public enum; callers should use [`GatherOperator::merge_sort`] instead
/// of constructing this state directly.
#[doc(hidden)]
#[derive(Debug)]
pub enum MergeInputState {
    Pending,
    Buffered { chunk: DataChunk, row_index: usize },
    Exhausted,
}

#[derive(Debug)]
pub enum GatherOperator {
    Concatenate {
        current_index: usize,
        col_names: Option<Vec<String>>,
        handle: Option<PartitionHandle>,
    },
    MergeSort {
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        inputs: Vec<MergeInputState>,
        col_names: Option<Vec<String>>,
        limit: Option<usize>,
        emitted: usize,
        handle: Option<PartitionHandle>,
    },
}

impl GatherOperator {
    pub fn concatenate() -> Self {
        Self::Concatenate {
            current_index: 0,
            col_names: None,
            handle: None,
        }
    }

    pub fn merge_sort(
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        limit: Option<usize>,
    ) -> Self {
        Self::MergeSort {
            sort_expressions,
            sort_directions,
            inputs: Vec::new(),
            col_names: None,
            limit,
            emitted: 0,
            handle: None,
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        children: &mut Vec<StreamingExecutor>,
    ) -> Result<(), QueryError> {
        let handle = match self {
            Self::Concatenate {
                current_index,
                col_names,
                handle,
            } => {
                *current_index = 0;
                *col_names = None;
                std::mem::take(handle)
            }
            Self::MergeSort {
                inputs,
                col_names,
                emitted,
                handle,
                ..
            } => {
                *inputs = (0..children.len())
                    .map(|_| MergeInputState::Pending)
                    .collect();
                *col_names = None;
                *emitted = 0;
                std::mem::take(handle)
            }
        };

        // Try parallel path via the runtime's morsel worker pool.
        if handle.is_none() {
            if let Some(rt) = base.runtime.clone() {
                let pool = rt.worker_pool.lock().clone();
                if let Some(pool) = pool {
                    if children.len() > 1 && pool.max_workers() > 1 {
                        let max_buffered = rt
                            .max_buffered_chunks
                            .load(std::sync::atomic::Ordering::Relaxed)
                            .max(1);
                        let (batch, receivers, error_rx) =
                            PartitionBatch::new(std::mem::take(children), rt.clone(), max_buffered);
                        let batch = Arc::new(batch);
                        let h = PartitionHandle::from_batch(
                            &batch,
                            receivers,
                            error_rx,
                            rt.clone(),
                            Instant::now(),
                            pool.max_workers(),
                        );
                        pool.submit(batch);
                        Self::set_handle(self, Some(h));
                        base.lifecycle.mark_opened();
                        return Ok(());
                    }
                }
            }
        }

        // Serial fallback: open children normally.
        for (opened_children, child) in children.iter_mut().enumerate() {
            if let Err(error) = child.open() {
                let close_error = close_children(&mut children[..opened_children]);
                return Err(close_error.unwrap_or(error));
            }
        }
        base.lifecycle.mark_opened();
        Ok(())
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        children: &mut [StreamingExecutor],
    ) -> Result<Option<DataChunk>, QueryError> {
        base.ensure_not_cancelled()?;
        match self {
            Self::Concatenate {
                current_index,
                col_names,
                handle,
            } => {
                while *current_index < input_count(children, handle) {
                    base.ensure_not_cancelled()?;
                    if let Some(chunk) = advance_input(children, handle, *current_index)? {
                        validate_schema(*current_index, &chunk, col_names)?;
                        return Ok(Some(chunk));
                    }
                    *current_index += 1;
                }
                Ok(None)
            }
            Self::MergeSort {
                sort_expressions,
                sort_directions,
                inputs,
                col_names,
                limit,
                emitted,
                handle,
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
                        handle,
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
        children: &mut [StreamingExecutor],
    ) -> Result<(), QueryError> {
        if let Some(mut handle) = Self::take_handle(self) {
            return handle.stop_and_join();
        }
        stop_children(children)
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        children: &mut [StreamingExecutor],
    ) -> Result<(), QueryError> {
        if let Self::MergeSort {
            inputs,
            col_names,
            emitted,
            ..
        } = self
        {
            inputs.clear();
            *col_names = None;
            *emitted = 0;
        }
        if let Self::Concatenate { col_names, .. } = self {
            *col_names = None;
        }
        let parallel_result = if let Some(mut handle) = Self::take_handle(self) {
            handle.stop_and_join()
        } else {
            Ok(())
        };
        base.lifecycle.mark_closed();
        parallel_result.and(close_children(children).map_or(Ok(()), Err))
    }

    fn set_handle(op: &mut Self, h: Option<PartitionHandle>) {
        match op {
            Self::Concatenate { handle, .. } | Self::MergeSort { handle, .. } => *handle = h,
        }
    }

    fn take_handle(op: &mut Self) -> Option<PartitionHandle> {
        match op {
            Self::Concatenate { ref mut handle, .. } | Self::MergeSort { ref mut handle, .. } => {
                handle.take()
            }
        }
    }
}

fn input_count(serial: &[StreamingExecutor], handle: &Option<PartitionHandle>) -> usize {
    handle
        .as_ref()
        .map(|h| h.partition_count)
        .unwrap_or(serial.len())
}

fn advance_input(
    children: &mut [StreamingExecutor],
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
    children: &mut [StreamingExecutor],
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
            QueryError::execution("Gather merge input has an invalid row index".to_string())
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
                        "Gather merge state changed while selecting a row".to_string(),
                    ));
                };
                let best_row = best_chunk.rows.get(*best_row_index).ok_or_else(|| {
                    QueryError::execution(
                        "Gather merge input has an invalid best-row index".to_string(),
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
            "Gather selected a non-buffered input".to_string(),
        ));
    };
    let row = chunk.rows.get(*row_index).cloned().ok_or_else(|| {
        QueryError::execution("Gather merge input has an invalid selected row index".to_string())
    })?;
    *row_index += 1;
    if *row_index >= chunk.rows.len() {
        inputs[index] = MergeInputState::Pending;
    }
    Ok(Some(row))
}

fn fill_input(
    index: usize,
    children: &mut [StreamingExecutor],
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
                QueryError::execution(format!("Gather failed to evaluate left sort key: {error}"))
            })?;
        let right =
            ExpressionEvaluator::evaluate(expression, &mut right_context).map_err(|error| {
                QueryError::execution(format!("Gather failed to evaluate right sort key: {error}"))
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
                "Gather schema mismatch in partition {}: expected {:?}, got {:?}",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;

    fn source(values: &[i64], column: &str) -> StreamingExecutor {
        StreamingExecutor::Source(
            OperatorBase::new(1),
            SourceOperator::ScanVertices {
                buffer: values
                    .iter()
                    .map(|value| vec![Value::BigInt(*value)])
                    .collect(),
                current_index: 0,
                col_names: vec![column.to_string()],
            },
        )
    }

    fn merge_executor(children: Vec<StreamingExecutor>, limit: Option<usize>) -> StreamingExecutor {
        StreamingExecutor::Gather(
            OperatorBase::new(i64::MIN).with_global(true),
            children,
            GatherOperator::merge_sort(
                vec![Expression::Variable("value".to_string())],
                vec![SortDirection::Ascending],
                limit,
            ),
        )
    }

    fn collect_values(
        executor: &mut StreamingExecutor,
    ) -> Result<(Vec<i64>, Vec<String>), QueryError> {
        executor.open()?;
        let mut values = Vec::new();
        let mut columns = Vec::new();
        while let Some(chunk) = executor.advance()? {
            if columns.is_empty() {
                columns = chunk.col_names();
            }
            for row in chunk.rows {
                if let Some(Value::BigInt(value)) = row.first() {
                    values.push(*value);
                }
            }
        }
        executor.close()?;
        Ok((values, columns))
    }

    #[test]
    fn merge_sort_merges_sorted_partition_streams_and_preserves_schema() {
        let mut executor = merge_executor(
            vec![source(&[1, 3], "value"), source(&[2, 4], "value")],
            None,
        );

        let (values, columns) = collect_values(&mut executor).expect("merge should succeed");

        assert_eq!(values, vec![1, 2, 3, 4]);
        assert_eq!(columns, vec!["value"]);
    }

    #[test]
    fn merge_sort_honors_global_limit() {
        let mut executor = merge_executor(
            vec![source(&[1, 3], "value"), source(&[2, 4], "value")],
            Some(3),
        );

        let (values, _) = collect_values(&mut executor).expect("merge should succeed");

        assert_eq!(values, vec![1, 2, 3]);
    }

    #[test]
    fn merge_sort_rejects_incompatible_partition_schema() {
        let mut executor = merge_executor(vec![source(&[1], "value"), source(&[2], "other")], None);

        executor.open().expect("open should succeed");
        let error = executor.advance().expect_err("schema mismatch must fail");
        executor
            .close()
            .expect("close should succeed after failure");

        assert!(error.to_string().contains("schema mismatch"));
    }

    #[test]
    fn concatenate_rejects_incompatible_partition_schema() {
        let mut executor = StreamingExecutor::Gather(
            OperatorBase::new(10),
            vec![source(&[1], "id"), source(&[2], "other_id")],
            GatherOperator::concatenate(),
        );

        executor.open().expect("open gather");
        assert!(executor.advance().expect("first partition").is_some());
        let error = executor.advance().expect_err("schema mismatch must fail");
        executor
            .close()
            .expect("close should succeed after failure");

        assert!(error.to_string().contains("schema mismatch"));
    }
}
