use std::cmp::Ordering;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::coordinator::ParallelPartitionCoordinator;
use crate::query::executor::streaming::executor::{
    SortDirection, StreamingExecutor, ValueRowContext,
};
use crate::query::executor::streaming::helpers::compare_values;
use crate::query::executor::streaming::operator_base::OperatorBase;
use crate::query::executor::streaming::parallel_safety::is_parallel_safe;

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

/// Runtime configuration and state for the P8 path below a Gather node.
#[derive(Debug)]
#[doc(hidden)]
pub struct ParallelGatherState {
    max_workers: usize,
    max_buffered_chunks: usize,
    partition_count: usize,
    coordinator: Option<ParallelPartitionCoordinator>,
    /// Recorded reason when P8 parallel execution was not activated,
    /// populated during `start_parallel` for EXPLAIN/PROFILE output.
    pub fallback_reason: Option<String>,
}

impl Default for ParallelGatherState {
    fn default() -> Self {
        Self {
            max_workers: 1,
            max_buffered_chunks: 1,
            partition_count: 0,
            coordinator: None,
            fallback_reason: None,
        }
    }
}

#[derive(Debug)]
pub enum GatherOperator {
    Concatenate {
        current_index: usize,
        col_names: Option<Vec<String>>,
        parallel: ParallelGatherState,
    },
    MergeSort {
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        inputs: Vec<MergeInputState>,
        col_names: Option<Vec<String>>,
        limit: Option<usize>,
        emitted: usize,
        parallel: ParallelGatherState,
    },
}

impl GatherOperator {
    pub fn concatenate() -> Self {
        Self::Concatenate {
            current_index: 0,
            col_names: None,
            parallel: ParallelGatherState::default(),
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
            parallel: ParallelGatherState::default(),
        }
    }

    /// Configure the formal P8 path. Values at or below one retain the
    /// sequential Gather implementation.
    pub fn configure_parallel(&mut self, max_workers: usize, max_buffered_chunks: usize) {
        let state = match self {
            Self::Concatenate { parallel, .. } | Self::MergeSort { parallel, .. } => parallel,
        };
        state.max_workers = max_workers.max(1);
        state.max_buffered_chunks = max_buffered_chunks.max(1);
        state.fallback_reason = None;
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        children: &mut Vec<StreamingExecutor>,
    ) -> Result<(), QueryError> {
        match self {
            Self::Concatenate {
                current_index,
                col_names,
                parallel,
            } => {
                *current_index = 0;
                *col_names = None;
                Self::start_parallel(base, children, parallel, false)?;
            }
            Self::MergeSort {
                inputs,
                col_names,
                emitted,
                parallel,
                ..
            } => {
                *inputs = (0..children.len())
                    .map(|_| MergeInputState::Pending)
                    .collect();
                *col_names = None;
                *emitted = 0;
                Self::start_parallel(base, children, parallel, true)?;
            }
        }

        if Self::uses_parallel(self) {
            base.lifecycle.mark_opened();
            return Ok(());
        }

        let mut opened_children = 0;
        for child in children.iter_mut() {
            if let Err(error) = child.open() {
                let close_error = close_children(&mut children[..opened_children]);
                return Err(close_error.unwrap_or(error));
            }
            opened_children += 1;
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
                parallel,
            } => {
                while *current_index < Self::input_count(parallel, children.len()) {
                    base.ensure_not_cancelled()?;
                    if let Some(chunk) = Self::advance_input(parallel, children, *current_index)? {
                        Self::validate_schema(*current_index, &chunk, col_names)?;
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
                parallel,
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
                    match Self::next_merge_row(
                        base,
                        children,
                        sort_expressions,
                        sort_directions,
                        inputs,
                        col_names,
                        parallel,
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
                    Ok(Some(DataChunk::from_rows_with_col_names(
                        result_rows,
                        col_names.clone(),
                    )))
                }
            }
        }
    }

    fn next_merge_row(
        base: &OperatorBase,
        children: &mut [StreamingExecutor],
        sort_expressions: &[Expression],
        sort_directions: &[SortDirection],
        inputs: &mut [MergeInputState],
        col_names: &mut Option<Vec<String>>,
        parallel: &mut ParallelGatherState,
    ) -> Result<Option<Vec<Value>>, QueryError> {
        let mut best_child = None;

        for index in 0..Self::input_count(parallel, children.len()) {
            base.ensure_not_cancelled()?;
            Self::fill_input(index, children, inputs, col_names, parallel)?;
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
                    Self::compare_rows(
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
            QueryError::execution(
                "Gather merge input has an invalid selected row index".to_string(),
            )
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
        inputs: &mut [MergeInputState],
        col_names: &mut Option<Vec<String>>,
        parallel: &mut ParallelGatherState,
    ) -> Result<(), QueryError> {
        if !matches!(inputs[index], MergeInputState::Pending) {
            return Ok(());
        }

        loop {
            match Self::advance_input(parallel, children, index)? {
                Some(chunk) if chunk.is_empty() => continue,
                Some(chunk) => {
                    Self::validate_schema(index, &chunk, col_names)?;
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
        for (index, expression) in sort_expressions.iter().enumerate() {
            let direction = sort_directions
                .get(index)
                .copied()
                .unwrap_or(SortDirection::Ascending);
            let mut left_context = ValueRowContext::new(a.to_vec(), col_names.to_vec());
            let mut right_context = ValueRowContext::new(b.to_vec(), col_names.to_vec());
            let left =
                ExpressionEvaluator::evaluate(expression, &mut left_context).map_err(|error| {
                    QueryError::execution(format!(
                        "Gather failed to evaluate left sort key: {error}"
                    ))
                })?;
            let right =
                ExpressionEvaluator::evaluate(expression, &mut right_context).map_err(|error| {
                    QueryError::execution(format!(
                        "Gather failed to evaluate right sort key: {error}"
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
                    "Gather schema mismatch in partition {}: expected {:?}, got {:?}",
                    partition_id, expected, child_columns
                )));
            }
        } else {
            *col_names = Some(child_columns);
        }
        Ok(())
    }

    pub fn stop(
        &mut self,
        _base: &mut OperatorBase,
        children: &mut [StreamingExecutor],
    ) -> Result<(), QueryError> {
        if let Some(parallel) = Self::parallel_state_mut(self) {
            if let Some(coordinator) = parallel.coordinator.as_mut() {
                return coordinator.stop_and_join();
            }
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
        if let Some(parallel) = Self::parallel_state_mut(self) {
            parallel.fallback_reason = None;
        }
        let parallel_result = if let Some(parallel) = Self::parallel_state_mut(self) {
            if let Some(coordinator) = parallel.coordinator.as_mut() {
                coordinator.stop_and_join()
            } else {
                Ok(())
            }
        } else {
            Ok(())
        };
        base.lifecycle.mark_closed();
        parallel_result.and(close_children(children).map_or(Ok(()), Err))
    }

    fn start_parallel(
        base: &OperatorBase,
        children: &mut Vec<StreamingExecutor>,
        parallel: &mut ParallelGatherState,
        requires_worker_per_partition: bool,
    ) -> Result<(), QueryError> {
        if parallel.max_workers <= 1 {
            parallel.fallback_reason = Some(format!(
                "parallel disabled (max_workers={})",
                parallel.max_workers
            ));
            return Ok(());
        }
        if children.len() <= 1 {
            parallel.fallback_reason = Some("only one partition, no parallel benefit".to_string());
            return Ok(());
        }
        if requires_worker_per_partition && parallel.max_workers < children.len() {
            parallel.fallback_reason = Some(format!(
                "parallel merge requires one worker per partition (workers={}, partitions={})",
                parallel.max_workers,
                children.len()
            ));
            return Ok(());
        }
        let unsafe_children: Vec<String> = children
            .iter()
            .enumerate()
            .filter_map(|(i, tree)| {
                if !tree.is_partition_local() || !is_parallel_safe(tree) {
                    Some(format!("child[{}]", i))
                } else {
                    None
                }
            })
            .collect();
        if !unsafe_children.is_empty() {
            parallel.fallback_reason = Some(format!(
                "not parallel-safe: {}",
                unsafe_children.join(", ")
            ));
            return Ok(());
        }
        let Some(runtime) = base.runtime.clone() else {
            parallel.fallback_reason = Some("no execution runtime available".to_string());
            return Ok(());
        };
        parallel.partition_count = children.len();
        let local_trees = std::mem::take(children);
        let coordinator = ParallelPartitionCoordinator::start(
            local_trees,
            runtime,
            parallel.max_workers,
            parallel.max_buffered_chunks,
        )?;
        parallel.coordinator = Some(coordinator);
        parallel.fallback_reason = None;
        Ok(())
    }

    fn uses_parallel(&self) -> bool {
        Self::parallel_state(self).is_some_and(|parallel| parallel.coordinator.is_some())
    }

    fn parallel_state(&self) -> Option<&ParallelGatherState> {
        match self {
            Self::Concatenate { parallel, .. } | Self::MergeSort { parallel, .. } => Some(parallel),
        }
    }

    fn parallel_state_mut(&mut self) -> Option<&mut ParallelGatherState> {
        match self {
            Self::Concatenate { parallel, .. } | Self::MergeSort { parallel, .. } => Some(parallel),
        }
    }

    fn advance_input(
        parallel: &mut ParallelGatherState,
        children: &mut [StreamingExecutor],
        index: usize,
    ) -> Result<Option<DataChunk>, QueryError> {
        if let Some(coordinator) = parallel.coordinator.as_mut() {
            coordinator.next_for_partition(index)
        } else {
            children[index].advance()
        }
    }

    fn input_count(parallel: &ParallelGatherState, serial_count: usize) -> usize {
        if parallel.coordinator.is_some() {
            parallel.partition_count
        } else {
            serial_count
        }
    }
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
                partition_id: 0,
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

    // ── P8 fallback reason tests ──

    #[test]
    fn parallel_fallback_reason_with_max_workers_one() {
        let mut op = GatherOperator::concatenate();
        op.configure_parallel(1, 10);

        let children = &mut vec![source(&[1], "id"), source(&[2], "id")];
        let mut base = OperatorBase::new(5);

        op.open(&mut base, children)
            .expect("open with max_workers=1 should succeed");
        let state = match &op {
            GatherOperator::Concatenate { parallel, .. } => parallel,
            _ => unreachable!(),
        };
        assert!(
            state.fallback_reason.is_some(),
            "expected fallback reason when max_workers=1"
        );
        assert!(state.fallback_reason.as_ref().unwrap().contains("max_workers"));

        op.close(&mut base, children).expect("close should succeed");
    }

    #[test]
    fn parallel_fallback_reason_with_single_partition() {
        let mut op = GatherOperator::concatenate();
        op.configure_parallel(4, 10);

        let children = &mut vec![source(&[1], "id")];
        let mut base = OperatorBase::new(5);

        op.open(&mut base, children)
            .expect("open with single partition should succeed");
        let state = match &op {
            GatherOperator::Concatenate { parallel, .. } => parallel,
            _ => unreachable!(),
        };
        assert!(
            state.fallback_reason.is_some(),
            "expected fallback reason when only one partition"
        );
        assert!(state.fallback_reason.as_ref().unwrap().contains("one partition"));

        op.close(&mut base, children).expect("close should succeed");
    }

    #[test]
    fn parallel_fallback_reason_without_runtime() {
        let mut op = GatherOperator::concatenate();
        op.configure_parallel(2, 10);

        let children = &mut vec![source(&[1], "id"), source(&[2], "id")];
        let mut base = OperatorBase::new(5);

        op.open(&mut base, children)
            .expect("open should succeed");
        let state = match &op {
            GatherOperator::Concatenate { parallel, .. } => parallel,
            _ => unreachable!(),
        };
        // With no runtime attached, start_parallel falls back on "no runtime"
        assert!(
            state.fallback_reason.is_some(),
            "expected fallback reason without a runtime"
        );

        op.close(&mut base, children).expect("close should succeed");
    }

    #[test]
    fn parallel_merge_falls_back_when_workers_are_fewer_than_partitions() {
        let mut op = GatherOperator::merge_sort(
            vec![Expression::Variable("id".to_string())],
            vec![SortDirection::Ascending],
            None,
        );
        op.configure_parallel(2, 1);

        let children = &mut vec![
            source(&[1, 4, 7], "id"),
            source(&[2, 5, 8], "id"),
            source(&[3, 6, 9], "id"),
        ];
        let mut base = OperatorBase::new(5);

        op.open(&mut base, children)
            .expect("merge should use the serial fallback");
        let state = match &op {
            GatherOperator::MergeSort { parallel, .. } => parallel,
            _ => unreachable!(),
        };
        assert!(state
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("one worker per partition")));

        op.close(&mut base, children)
            .expect("serial fallback should close children");
    }

    #[test]
    fn streaming_executor_parallel_fallback_reason_is_none_by_default() {
        let gather = StreamingExecutor::Gather(
            OperatorBase::new(i64::MIN).with_global(true),
            vec![source(&[1], "id"), source(&[2], "id")],
            GatherOperator::concatenate(),
        );

        // Before configure/open, fallback_reason is None
        assert!(
            gather.parallel_fallback_reason().is_none(),
            "no fallback reason before configure/open"
        );
    }
}
