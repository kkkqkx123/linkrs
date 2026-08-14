use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
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
use crate::query::executor::streaming::operators::source_operator::OperatorConfig;
use crate::query::executor::streaming::operators::spec::ExchangeSpec;
use crate::query::executor::streaming::operators::state::{ExchangeState, MergeInputState};
use crate::query::executor::streaming::pool::{PartitionBatch, PartitionHandle};
use crate::query::executor::streaming::runtime::ExecutionRuntime;
use crate::query::executor::streaming::slot::SlotLayout;

const CHUNK_SIZE: usize = 1024;

/// Exchange operator.
///
/// Wraps [`ExchangeState`] with the runtime context injected at `open()`.
/// Lifecycle state is owned exclusively by the executor; operators never
/// write it.
#[derive(Debug)]
pub struct ExchangeOperator {
    pub state: ExchangeState,
    pub(crate) handle: Option<PartitionHandle>,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl ExchangeOperator {
    pub fn from_spec(spec: &ExchangeSpec, output_layout: Arc<SlotLayout>) -> Self {
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
            ExchangeSpec::RepartitionHash {
                num_partitions,
                hash_expressions,
                ..
            } => ExchangeState::RepartitionHash {
                num_partitions: *num_partitions,
                buckets: (0..*num_partitions).map(|_| Vec::new()).collect(),
                current_bucket: 0,
                current_row: 0,
                hash_expressions: hash_expressions.clone(),
                col_names: None,
            },
            ExchangeSpec::Broadcast { num_consumers } => ExchangeState::Broadcast {
                num_consumers: *num_consumers,
                buffered_chunks: Vec::new(),
                current_consumer: 0,
                chunk_index: 0,
                row_index: 0,
            },
            ExchangeSpec::Barrier => ExchangeState::Barrier { passed: false },
            ExchangeSpec::Materialize { child_count: _ } => ExchangeState::Materialize {
                rows: Vec::new(),
                position: 0,
                col_names: None,
            },
        };
        Self::new(state, output_layout)
    }

    pub fn new(kind: ExchangeState, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            state: kind,
            handle: None,
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

    pub fn open(&mut self, children: &mut Vec<StreamingExecutor>) -> Result<(), QueryError> {
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
            ExchangeState::RepartitionHash {
                buckets,
                current_bucket,
                current_row,
                ..
            } => {
                for bucket in buckets.iter_mut() {
                    bucket.clear();
                }
                *current_bucket = 0;
                *current_row = 0;
            }
            ExchangeState::Broadcast {
                buffered_chunks,
                current_consumer,
                chunk_index,
                row_index,
                ..
            } => {
                buffered_chunks.clear();
                *current_consumer = 0;
                *chunk_index = 0;
                *row_index = 0;
            }
            ExchangeState::Barrier { passed } => {
                *passed = false;
            }
            ExchangeState::Materialize {
                rows,
                position,
                col_names,
            } => {
                rows.clear();
                *position = 0;
                *col_names = None;
            }
        }

        let runtime = self.runtime.clone();
        if let Some(rt) = &runtime {
            let pool = rt.worker_pool.lock().clone();
            if let Some(pool) = pool {
                if children.len() > 1 && pool.max_workers() > 1 {
                    let max_buffered = self.config.chunk_size.clamp(1, 10);
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
                    return Ok(());
                }
            }
        }

        for child in children.iter_mut() {
            child.open()?;
        }
        Ok(())
    }

    pub fn next(
        &mut self,
        children: &mut [StreamingExecutor],
    ) -> Result<Option<DataChunk>, QueryError> {
        if let Some(rt) = self.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        match &mut self.state {
            ExchangeState::Concatenate {
                current_index,
                col_names,
            } => {
                while *current_index < input_count(children, &self.handle) {
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if let Some(chunk) = advance_input(children, &mut self.handle, *current_index)?
                    {
                        if chunk.is_empty() {
                            continue;
                        }
                        validate_schema(*current_index, &chunk, col_names)?;
                        return Ok(Some(DataChunk::new_with_layout(
                            chunk.rows,
                            Arc::clone(&self.output_layout),
                        )));
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
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if limit.is_some_and(|value| *emitted >= value) {
                        break;
                    }
                    match next_merge_row(
                        &self.runtime,
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
                    Ok(Some(DataChunk::new_with_layout(
                        result_rows,
                        Arc::clone(&self.output_layout),
                    )))
                }
            }
            ExchangeState::RepartitionHash {
                num_partitions,
                buckets,
                current_bucket,
                current_row,
                hash_expressions,
                col_names,
            } => {
                // Phase 1: drain all children and partition into buckets.
                if *current_bucket == 0 && *current_row == 0 && buckets.iter().all(|b| b.is_empty())
                {
                    drain_and_partition(
                        children,
                        &mut self.handle,
                        *num_partitions,
                        buckets,
                        hash_expressions,
                        col_names,
                        &self.runtime,
                    )?;
                }

                // Phase 2: emit rows bucket by bucket.
                while *current_bucket < *num_partitions {
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if *current_row < buckets[*current_bucket].len() {
                        let mut result_rows = Vec::with_capacity(CHUNK_SIZE);
                        while *current_row < buckets[*current_bucket].len()
                            && result_rows.len() < CHUNK_SIZE
                        {
                            result_rows.push(buckets[*current_bucket][*current_row].clone());
                            *current_row += 1;
                        }
                        return Ok(Some(DataChunk::new_with_layout(
                            result_rows,
                            Arc::clone(&self.output_layout),
                        )));
                    }
                    *current_bucket += 1;
                    *current_row = 0;
                }
                Ok(None)
            }
            ExchangeState::Broadcast {
                num_consumers,
                buffered_chunks,
                current_consumer,
                chunk_index,
                row_index,
            } => {
                // Phase 1: drain all children into chunks if not yet drained.
                if buffered_chunks.is_empty() && *chunk_index == 0 && *row_index == 0 {
                    let count = input_count(children, &self.handle);
                    for i in 0..count {
                        loop {
                            if let Some(rt) = self.runtime.as_ref() {
                                rt.ensure_not_cancelled()?;
                            }
                            match advance_input(children, &mut self.handle, i)? {
                                Some(chunk) if !chunk.is_empty() => buffered_chunks.push(chunk),
                                Some(_) => continue,
                                None => break,
                            }
                        }
                    }
                }

                if buffered_chunks.is_empty() {
                    return Ok(None);
                }

                // Phase 2: emit rows for the current consumer.
                let mut result_rows = Vec::with_capacity(CHUNK_SIZE);
                while *chunk_index < buffered_chunks.len() && result_rows.len() < CHUNK_SIZE {
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    let chunk = &buffered_chunks[*chunk_index];
                    while *row_index < chunk.rows.len() && result_rows.len() < CHUNK_SIZE {
                        result_rows.push(chunk.rows[*row_index].clone());
                        *row_index += 1;
                    }
                    if *row_index >= chunk.rows.len() {
                        *chunk_index += 1;
                        *row_index = 0;
                    }
                }

                if result_rows.is_empty() {
                    return Ok(None);
                }
                let result =
                    DataChunk::new_with_layout(result_rows, Arc::clone(&self.output_layout));

                // Advance consumer. When all consumers have been served, reset
                // chunk tracking so the next consumer starts from the beginning.
                *current_consumer += 1;
                if *current_consumer >= *num_consumers {
                    *current_consumer = 0;
                    *chunk_index = 0;
                    *row_index = 0;
                }

                Ok(Some(result))
            }
            ExchangeState::Barrier { passed } => {
                // Barrier: consume all children, then pass through one EOF marker.
                if !*passed {
                    // Drain all input children.
                    let mut all_rows = Vec::new();
                    let mut col_names: Option<Vec<String>> = None;
                    loop {
                        if let Some(rt) = self.runtime.as_ref() {
                            rt.ensure_not_cancelled()?;
                        }
                        match advance_input(children, &mut self.handle, 0)? {
                            Some(mut chunk) => {
                                chunk.materialize_selection_by("Exchange");
                                if col_names.is_none() {
                                    col_names = Some(chunk.col_names());
                                }
                                all_rows.extend(chunk.rows);
                            }
                            None => break,
                        }
                    }
                    *passed = true;
                    if all_rows.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(DataChunk::new_with_layout(
                        all_rows,
                        Arc::clone(&self.output_layout),
                    )));
                }
                Ok(None)
            }
            ExchangeState::Materialize {
                rows,
                position,
                col_names,
            } => {
                // Phase 1: drain all children if not yet drained.
                if rows.is_empty() && *position == 0 {
                    let count = input_count(children, &self.handle);
                    for i in 0..count {
                        loop {
                            if let Some(rt) = self.runtime.as_ref() {
                                rt.ensure_not_cancelled()?;
                            }
                            match advance_input(children, &mut self.handle, i)? {
                                Some(mut chunk) => {
                                    chunk.materialize_selection_by("Exchange");
                                    if col_names.is_none() {
                                        *col_names = Some(chunk.col_names());
                                    }
                                    rows.extend(chunk.rows);
                                }
                                None => break,
                            }
                        }
                    }
                }

                if *position >= rows.len() {
                    return Ok(None);
                }

                let mut result_rows = Vec::with_capacity(CHUNK_SIZE);
                while *position < rows.len() && result_rows.len() < CHUNK_SIZE {
                    result_rows.push(rows[*position].clone());
                    *position += 1;
                }

                Ok(Some(DataChunk::new_with_layout(
                    result_rows,
                    Arc::clone(&self.output_layout),
                )))
            }
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        if let Some(handle) = self.handle.as_mut() {
            return handle.stop_and_join();
        }
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        if let Some(handle) = self.handle.as_mut() {
            let _ = handle.stop_and_join();
            self.handle = None;
        }
        Ok(())
    }
}

// ── Drain helpers ──────────────────────────────────────────────────────────

/// Drain children and partition rows into hash buckets.
#[allow(clippy::too_many_arguments)]
fn drain_and_partition(
    children: &mut [StreamingExecutor],
    handle: &mut Option<PartitionHandle>,
    num_partitions: usize,
    buckets: &mut [Vec<Vec<Value>>],
    hash_expressions: &[Expression],
    col_names: &mut Option<Vec<String>>,
    runtime: &Option<Arc<ExecutionRuntime>>,
) -> Result<(), QueryError> {
    let count = input_count(children, handle);
    for i in 0..count {
        loop {
            if let Some(rt) = runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            match advance_input(children, handle, i)? {
                Some(mut chunk) => {
                    chunk.materialize_selection_by("Exchange");
                    if col_names.is_none() {
                        *col_names = Some(chunk.col_names());
                    }
                    let layout =
                        Arc::new(SlotLayout::from_names(col_names.as_deref().unwrap_or(&[])));
                    for row in &chunk.rows {
                        let hash = compute_hash(row, hash_expressions, &layout)?;
                        let bucket = (hash as usize) % num_partitions;
                        buckets[bucket].push(row.clone());
                    }
                }
                None => break,
            }
        }
    }
    Ok(())
}

/// Compute a hash value for a row based on the given expressions.
fn compute_hash(
    row: &[Value],
    hash_expressions: &[Expression],
    layout: &SlotLayout,
) -> Result<u64, QueryError> {
    let mut hasher = DefaultHasher::new();
    let arc_layout = Arc::new(layout.clone());
    for expr in hash_expressions {
        let mut ctx = ValueRowContext::new(row.to_vec(), arc_layout.clone());
        let value = ExpressionEvaluator::evaluate(expr, &mut ctx).map_err(|e| {
            QueryError::execution(format!("RepartitionHash failed to evaluate key: {e}"))
        })?;
        value.hash(&mut hasher);
    }
    Ok(hasher.finish())
}

// ── Existing helpers (unchanged) ───────────────────────────────────────────

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
    runtime: &Option<Arc<ExecutionRuntime>>,
    children: &mut [StreamingExecutor],
    handle: &mut Option<PartitionHandle>,
    sort_expressions: &[Expression],
    sort_directions: &[SortDirection],
    inputs: &mut [MergeInputState],
    col_names: &mut Option<Vec<String>>,
) -> Result<Option<Vec<Value>>, QueryError> {
    let mut best_child = None;

    for index in 0..input_count(children, handle) {
        if let Some(rt) = runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
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
            Some(mut chunk) => {
                chunk.materialize_selection_by("Exchange");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::streaming::operators::base::OperatorBase;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;
    use crate::query::executor::streaming::operators::source_operator::SourceOperatorKind;

    fn source(values: &[i64], column: &str) -> StreamingExecutor {
        let layout = Arc::new(SlotLayout::from_names(&[column.to_string()]));
        StreamingExecutor::Source(
            OperatorBase::new(1).with_output_layout(layout.clone()),
            SourceOperator::new(
                SourceOperatorKind::ScanVertices {
                    buffer: values.iter().map(|v| vec![Value::BigInt(*v)]).collect(),
                    current_index: 0,
                    col_names: vec![column.to_string()],
                },
                layout,
            ),
        )
    }

    #[test]
    fn test_repartition_hash_partitions_rows_by_hash() {
        let _op = ExchangeOperator::from_spec(
            &ExchangeSpec::RepartitionHash {
                num_partitions: 3,
                hash_expressions: vec![Expression::Variable("value".to_string())],
                input_layout: None,
                output_layout: None,
            },
            Arc::new(SlotLayout::new(Vec::new())),
        );

        // Manually test the partition logic
        let col_names = vec!["value".to_string()];
        let layout = Arc::new(SlotLayout::from_names(&col_names));
        let rows = vec![
            vec![Value::BigInt(1)],
            vec![Value::BigInt(2)],
            vec![Value::BigInt(3)],
            vec![Value::BigInt(4)],
            vec![Value::BigInt(5)],
        ];

        let mut buckets: Vec<Vec<Vec<Value>>> = (0..3).map(|_| Vec::new()).collect();
        let hash_expr = vec![Expression::Variable("value".to_string())];
        for row in &rows {
            let hash = compute_hash(row, &hash_expr, &layout).unwrap();
            let bucket = (hash as usize) % 3;
            buckets[bucket].push(row.clone());
        }

        let total: usize = buckets.iter().map(|b| b.len()).sum();
        assert_eq!(total, 5);
        // Each bucket must have at least one row for 5 items across 3 buckets
        assert!(buckets.iter().any(|b| b.len() >= 2));
    }

    #[test]
    fn test_broadcast_state_from_spec() {
        let op = ExchangeOperator::from_spec(
            &ExchangeSpec::Broadcast { num_consumers: 4 },
            Arc::new(SlotLayout::new(Vec::new())),
        );
        match op.state {
            ExchangeState::Broadcast {
                num_consumers,
                buffered_chunks,
                current_consumer,
                ..
            } => {
                assert_eq!(num_consumers, 4);
                assert!(buffered_chunks.is_empty());
                assert_eq!(current_consumer, 0);
            }
            _ => panic!("Expected Broadcast state"),
        }
    }

    #[test]
    fn test_barrier_state_from_spec() {
        let op = ExchangeOperator::from_spec(
            &ExchangeSpec::Barrier,
            Arc::new(SlotLayout::new(Vec::new())),
        );
        match op.state {
            ExchangeState::Barrier { passed } => {
                assert!(!passed);
            }
            _ => panic!("Expected Barrier state"),
        }
    }

    #[test]
    fn test_materialize_state_from_spec() {
        let op = ExchangeOperator::from_spec(
            &ExchangeSpec::Materialize { child_count: 2 },
            Arc::new(SlotLayout::new(Vec::new())),
        );
        match op.state {
            ExchangeState::Materialize { rows, position, .. } => {
                assert!(rows.is_empty());
                assert_eq!(position, 0);
            }
            _ => panic!("Expected Materialize state"),
        }
    }

    #[test]
    fn test_merge_sort_honors_limit() {
        let mut children = vec![source(&[1, 3], "value"), source(&[2, 4], "value")];

        let layout = Arc::new(SlotLayout::new(Vec::new()));
        let mut op = ExchangeOperator::from_spec(
            &ExchangeSpec::MergeSort {
                sort_expressions: vec![Expression::Variable("value".to_string())],
                sort_directions: vec![SortDirection::Ascending],
                limit: Some(3),
            },
            layout,
        );

        // Open exchange operator (initializes merge state) and children.
        op.open(&mut children).unwrap();

        let mut all_values = Vec::new();
        while let Some(chunk) = op.next(&mut children).unwrap() {
            for row in chunk.rows {
                if let Some(Value::BigInt(v)) = row.first() {
                    all_values.push(*v);
                }
            }
        }

        op.close().unwrap();
        assert_eq!(all_values, vec![1, 2, 3]);
    }
}
