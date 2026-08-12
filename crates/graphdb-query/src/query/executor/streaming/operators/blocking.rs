use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::base::{MemoryBudget, MemoryTracker};
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::SortDirection;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::executor::ValueRowContext;
use crate::query::executor::streaming::helpers::accumulator_states::{
    accumulator_to_value, AggregateAccumulator,
};
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::spill::{
    HashPartitionConfig, HashPartitionSpiller, SpillManager, SpilledFile,
};

pub mod aggregate;
pub mod materialize;
pub mod sort;
pub mod window;

pub use aggregate::{AggregateState, FinalAggregateState, GroupByState, PartialAggregateState};
pub use materialize::{DataCollectState, DistinctState, MaterializeState, RollUpApplyState};
pub use sort::{MergeState, RunBuffer, SortState, TopNState};

type BatchEvalResult = Option<(Vec<Vec<Value>>, Vec<Vec<Value>>)>;
pub use window::{WindowFunctionState, WindowState};

use crate::query::executor::streaming::chunk::ColumnarBatch;
use aggregate::{value_to_partial_accumulator, ACCUMULATOR_OVERHEAD_BYTES};
use sort::{find_min_run, refill_run_buffer, sort_columnar_batch, spill_sorted_run};
use window::{compute_window_partition_result, sort_partition_rows};

/// Reject spill for operators that do not support disk-based overflow.
/// Per D9: may return resource-exhaustion error for non-spillable operators,
/// but must not claim budget protection.
///
/// Always returns an error — the `memory_tracker` parameter is kept so that
/// caller sites can uniformly follow the Sort spill pattern without branching.
fn spill_not_supported(
    _buffer: &mut Vec<Vec<Value>>,
    _sm: &SpillManager,
    _spill_files: &mut Vec<SpilledFile>,
    _memory_tracker: &mut MemoryTracker,
) -> Result<(), QueryError> {
    Err(QueryError::execution(
        "Spill is not implemented for this blocking operator; query memory budget exceeded"
            .to_string(),
    ))
}

/// Reject replay of spilled files for operators that cannot stream from disk.
/// Per C4: prevents unbounded memory growth during spill recovery.
fn reject_spill_replay(_spill_files: &[SpilledFile]) -> Result<Vec<Vec<Value>>, QueryError> {
    Err(QueryError::execution(
        "This blocking operator cannot replay spilled data within the query memory budget"
            .to_string(),
    ))
}

#[derive(Debug)]
pub enum BlockingOperator {
    Sort {
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        memory_tracker: MemoryTracker,
        state: Option<SortState>,
    },
    Aggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<(AggregateFunction, Expression)>,
        output_col_names: Vec<String>,
        memory_tracker: MemoryTracker,
        state: Option<AggregateState>,
    },
    GroupBy {
        group_by_expressions: Vec<Expression>,
        memory_tracker: MemoryTracker,
        state: Option<GroupByState>,
    },
    WindowFunction {
        window_exprs: Vec<Expression>,
        partition_by_exprs: Vec<Expression>,
        order_by_exprs: Vec<Expression>,
        order_by_directions: Vec<SortDirection>,
        memory_tracker: MemoryTracker,
        state: Option<WindowFunctionState>,
    },
    Window {
        window_exprs: Vec<Expression>,
        partition_by_exprs: Vec<Expression>,
        order_by_exprs: Vec<Expression>,
        order_by_directions: Vec<SortDirection>,
        memory_tracker: MemoryTracker,
        state: Option<WindowState>,
    },
    TopN {
        n: u32,
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        memory_tracker: MemoryTracker,
        state: Option<TopNState>,
    },
    Distinct {
        memory_tracker: MemoryTracker,
        state: Option<DistinctState>,
    },
    Materialize {
        memory_tracker: MemoryTracker,
        state: Option<MaterializeState>,
    },
    DataCollect {
        memory_tracker: MemoryTracker,
        state: Option<DataCollectState>,
    },
    RollUpApply {
        rollup_expressions: Vec<Expression>,
        memory_tracker: MemoryTracker,
        state: Option<RollUpApplyState>,
    },
    PartialAggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<AggregateFunction>,
        output_col_names: Vec<String>,
        memory_tracker: MemoryTracker,
        state: Option<PartialAggregateState>,
    },
    FinalAggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<AggregateFunction>,
        output_col_names: Vec<String>,
        memory_tracker: MemoryTracker,
        state: Option<FinalAggregateState>,
    },
}

impl BlockingOperator {
    pub fn from_spec(
        spec: &super::spec::BlockingSpec,
        memory_budget: &crate::query::executor::base::MemoryBudget,
    ) -> Self {
        match spec {
            super::spec::BlockingSpec::Sort {
                sort_expressions,
                sort_directions,
            } => Self::Sort {
                sort_expressions: sort_expressions.clone(),
                sort_directions: sort_directions.clone(),
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::spec::BlockingSpec::Aggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names,
            } => Self::Aggregate {
                group_by_expressions: group_by_expressions.clone(),
                aggregate_functions: aggregate_functions.clone(),
                output_col_names: output_col_names.clone(),
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::spec::BlockingSpec::GroupBy {
                group_by_expressions,
            } => Self::GroupBy {
                group_by_expressions: group_by_expressions.clone(),
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::spec::BlockingSpec::WindowFunction {
                window_exprs,
                partition_by_exprs,
                order_by_exprs,
                order_by_directions,
            } => Self::WindowFunction {
                window_exprs: window_exprs.clone(),
                partition_by_exprs: partition_by_exprs.clone(),
                order_by_exprs: order_by_exprs.clone(),
                order_by_directions: order_by_directions.clone(),
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::spec::BlockingSpec::Window {
                window_exprs,
                partition_by_exprs,
                order_by_exprs,
                order_by_directions,
            } => Self::Window {
                window_exprs: window_exprs.clone(),
                partition_by_exprs: partition_by_exprs.clone(),
                order_by_exprs: order_by_exprs.clone(),
                order_by_directions: order_by_directions.clone(),
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::spec::BlockingSpec::TopN {
                n,
                sort_expressions,
                sort_directions,
            } => Self::TopN {
                n: *n,
                sort_expressions: sort_expressions.clone(),
                sort_directions: sort_directions.clone(),
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::spec::BlockingSpec::Distinct => Self::Distinct {
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::spec::BlockingSpec::Materialize => Self::Materialize {
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::spec::BlockingSpec::DataCollect => Self::DataCollect {
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::spec::BlockingSpec::RollUpApply { rollup_expressions } => Self::RollUpApply {
                rollup_expressions: rollup_expressions.clone(),
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::spec::BlockingSpec::PartialAggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names,
            } => Self::PartialAggregate {
                group_by_expressions: group_by_expressions.clone(),
                aggregate_functions: aggregate_functions.clone(),
                output_col_names: output_col_names.clone(),
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::spec::BlockingSpec::FinalAggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names,
            } => Self::FinalAggregate {
                group_by_expressions: group_by_expressions.clone(),
                aggregate_functions: aggregate_functions.clone(),
                output_col_names: output_col_names.clone(),
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
        }
    }

    pub fn memory_tracker(&self) -> &MemoryTracker {
        match self {
            Self::Sort { memory_tracker, .. }
            | Self::Aggregate { memory_tracker, .. }
            | Self::GroupBy { memory_tracker, .. }
            | Self::WindowFunction { memory_tracker, .. }
            | Self::Window { memory_tracker, .. }
            | Self::TopN { memory_tracker, .. }
            | Self::Distinct { memory_tracker, .. }
            | Self::Materialize { memory_tracker, .. }
            | Self::DataCollect { memory_tracker, .. }
            | Self::RollUpApply { memory_tracker, .. }
            | Self::PartialAggregate { memory_tracker, .. }
            | Self::FinalAggregate { memory_tracker, .. } => memory_tracker,
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::Sort { state, .. } => {
                *state = Some(SortState {
                    col_names: vec![],
                    input_layout: None,
                    columnar_batch: None,
                    all_rows: vec![],
                    row_iter: None,
                    spill_files: vec![],
                    runs: vec![],
                    has_spilled: false,
                    merge_state: None,
                });
            }
            Self::Aggregate {
                state,
                aggregate_functions,
                ..
            } => {
                *state = Some(AggregateState {
                    group_map: HashMap::new(),
                    accumulator_overhead: aggregate_functions.len() * ACCUMULATOR_OVERHEAD_BYTES,
                    result_iter: None,
                    spill_files: vec![],
                    partition_spiller: None,
                    spilled_runs: vec![],
                    current_partition: 0,
                    has_spilled: false,
                    output_complete: false,
                    col_names: vec![],
                });
            }
            Self::GroupBy { state, .. } => {
                *state = Some(GroupByState {
                    all_rows: vec![],
                    col_names: vec![],
                    result_iter: None,
                    spill_files: vec![],
                    partition_spiller: None,
                    spilled_runs: vec![],
                    current_partition: 0,
                    has_spilled: false,
                    output_complete: false,
                });
            }
            Self::WindowFunction { state, .. } => {
                *state = Some(WindowFunctionState {
                    all_rows: vec![],
                    col_names: vec![],
                    result_iter: None,
                    spill_files: vec![],
                    partition_spiller: None,
                    spilled_runs: vec![],
                    current_partition: 0,
                    has_spilled: false,
                    output_complete: false,
                });
            }
            Self::Window { state, .. } => {
                *state = Some(WindowState {
                    all_rows: vec![],
                    col_names: vec![],
                    result_iter: None,
                    spill_files: vec![],
                    partition_spiller: None,
                    spilled_runs: vec![],
                    current_partition: 0,
                    has_spilled: false,
                    output_complete: false,
                });
            }
            Self::TopN { state, .. } => {
                *state = Some(TopNState {
                    columnar_batch: None,
                    col_names: vec![],
                    input_layout: None,
                    result_iter: None,
                });
            }
            Self::Distinct { state, .. } => {
                *state = Some(DistinctState {
                    seen_rows: std::collections::HashSet::new(),
                    col_names: Vec::new(),
                    input_layout: None,
                    spill_files: vec![],
                    partition_spiller: None,
                    spilled_runs: vec![],
                    current_partition: 0,
                    partition_seen: std::collections::HashSet::new(),
                    has_spilled: false,
                    output_iter: None,
                });
            }
            Self::Materialize { state, .. } => {
                *state = Some(MaterializeState {
                    materialized_rows: vec![],
                    result_iter: None,
                    materialized: false,
                    spill_files: vec![],
                    input_layout: None,
                });
            }
            Self::DataCollect { state, .. } => {
                *state = Some(DataCollectState {
                    all_rows: vec![],
                    emitted: false,
                    spill_files: vec![],
                    input_layout: None,
                });
            }
            Self::RollUpApply { state, .. } => {
                *state = Some(RollUpApplyState {
                    all_rows: vec![],
                    result_iter: None,
                    spill_files: vec![],
                });
            }
            Self::PartialAggregate { state, .. } => {
                *state = Some(PartialAggregateState {
                    group_map: HashMap::new(),
                    col_names: vec![],
                    result_iter: None,
                    spill_files: vec![],
                });
            }
            Self::FinalAggregate { state, .. } => {
                *state = Some(FinalAggregateState {
                    group_map: HashMap::new(),
                    col_names: vec![],
                    result_iter: None,
                    spill_files: vec![],
                });
            }
        }
        input.open()?;
        base.lifecycle.mark_opened();
        Ok(())
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::Sort {
                sort_expressions,
                sort_directions,
                memory_tracker,
                state,
                ..
            } => {
                let st = state.as_mut().unwrap();

                if st.merge_state.is_none() && st.row_iter.is_none() {
                    while let Some(mut chunk) = input.advance()? {
                        chunk.materialize_selection_by("Sort");
                        base.ensure_not_cancelled()?;
                        if st.col_names.is_empty() {
                            st.col_names = chunk.col_names();
                            st.input_layout = Some(chunk.get_layout());
                        }
                        // Column-major accumulation below the spill
                        // boundary. Memory is accounted per appended row
                        // (same model as the legacy row path); once the
                        // budget is exhausted the batch is materialized and
                        // spilled row-wise, keeping the spill machinery
                        // unchanged.
                        let batch = st
                            .columnar_batch
                            .get_or_insert_with(|| ColumnarBatch::new(chunk.num_columns()));
                        for idx in chunk.visible_indices() {
                            let row = &chunk.rows[idx];
                            if let Err(e) = memory_tracker.try_reserve_row(row) {
                                if let Some(sm) = base.spill_manager() {
                                    let mut rows = batch.to_rows();
                                    spill_sorted_run(
                                        &mut rows,
                                        &st.col_names,
                                        sort_expressions,
                                        sort_directions,
                                        &sm,
                                        memory_tracker,
                                        &mut st.runs,
                                    )?;
                                    st.has_spilled = true;
                                    batch.clear();
                                    memory_tracker.reset();
                                    memory_tracker.try_reserve_row(row)?;
                                } else {
                                    return Err(e);
                                }
                            }
                            batch.append_chunk_row(&chunk, idx);
                        }
                    }

                    if !st.spill_files.is_empty() {
                        return reject_spill_replay(&st.spill_files).map(|_| None);
                    }

                    if st.has_spilled {
                        if let Some(batch) = st.columnar_batch.take() {
                            if batch.num_rows() > 0 {
                                if let Some(sm) = base.spill_manager() {
                                    let mut rows = batch.to_rows();
                                    spill_sorted_run(
                                        &mut rows,
                                        &st.col_names,
                                        sort_expressions,
                                        sort_directions,
                                        &sm,
                                        memory_tracker,
                                        &mut st.runs,
                                    )?;
                                }
                            }
                        }

                        let mut run_buffers = Vec::with_capacity(st.runs.len());
                        for run in &st.runs {
                            let reader =
                                crate::query::executor::streaming::spill::RunReader::open(run)?;
                            run_buffers.push(RunBuffer {
                                rows: Vec::new(),
                                index: 0,
                                reader,
                            });
                        }

                        for buf in &mut run_buffers {
                            refill_run_buffer(buf, 1024)?;
                        }

                        st.merge_state = Some(MergeState {
                            run_buffers,
                            col_names: st.col_names.clone(),
                        });
                    } else {
                        if let Some(mut batch) = st.columnar_batch.take() {
                            if !sort_expressions.is_empty() {
                                sort_columnar_batch(
                                    &mut batch,
                                    &st.col_names,
                                    sort_expressions,
                                    sort_directions,
                                );
                            }
                            st.row_iter = Some(batch.to_rows().into_iter());
                        }
                    }
                }

                if let Some(ref mut merge) = st.merge_state {
                    let batch_size = 1024;
                    let mut out_rows = Vec::with_capacity(batch_size);

                    while out_rows.len() < batch_size {
                        base.ensure_not_cancelled()?;
                        let min_idx = find_min_run(
                            &merge.run_buffers,
                            &merge.col_names,
                            sort_expressions,
                            sort_directions,
                        );

                        match min_idx {
                            None => break,
                            Some(idx) => {
                                let buf = &mut merge.run_buffers[idx];
                                let row = buf.rows[buf.index].clone();
                                out_rows.push(row);
                                buf.index += 1;

                                if buf.index >= buf.rows.len() {
                                    refill_run_buffer(buf, 1024)?;
                                }
                            }
                        }
                    }

                    if out_rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::new_with_layout(
                            out_rows,
                            Arc::clone(&base.output_layout),
                        )))
                    }
                } else if let Some(ref mut iter) = st.row_iter {
                    let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                    if chunk_rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::new_with_layout(
                            chunk_rows,
                            Arc::clone(&base.output_layout),
                        )))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::Aggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names: _,
                memory_tracker,
                state,
                ..
            } => {
                let state = state.as_mut().unwrap();

                loop {
                    if state.output_complete {
                        return Ok(None);
                    }

                    let num_group_keys = group_by_expressions.len();
                    let has_group_keys = num_group_keys > 0;
                    // Per-group overhead beyond the group key: one accumulator
                    // instance per aggregate function. Without this, workloads
                    // with many small keys would under-report memory usage.
                    let group_overhead = state.accumulator_overhead;

                    // Evaluate group key expressions on a single input row
                    // (fallback path for selection-bearing chunks).
                    let eval_group_key = |row: &[Value], col_names: &[String]| -> Vec<Value> {
                        if !has_group_keys {
                            return Vec::new();
                        }
                        let mut key = Vec::with_capacity(num_group_keys);
                        for expr in group_by_expressions.iter() {
                            let mut ctx =
                                ValueRowContext::from_names(row.to_vec(), col_names.to_vec());
                            match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                Ok(value) => key.push(value),
                                Err(_) => key.push(Value::Null(NullType::Null)),
                            }
                        }
                        key
                    };

                    // Output phase: drain result iterator
                    if let Some(ref mut iter) = state.result_iter {
                        let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                        if chunk_rows.is_empty() {
                            state.result_iter = None;
                            if !state.has_spilled
                                || state.current_partition >= state.spilled_runs.len()
                            {
                                state.output_complete = true;
                                return Ok(None);
                            }
                        } else {
                            return Ok(Some(DataChunk::new_with_layout(
                                chunk_rows,
                                Arc::clone(&base.output_layout),
                            )));
                        }
                    }

                    // Replay phase: process spilled partitions one at a time
                    if state.has_spilled && state.partition_spiller.is_none() {
                        while state.current_partition < state.spilled_runs.len() {
                            base.ensure_not_cancelled()?;

                            let run = match &state.spilled_runs[state.current_partition] {
                                Some(r) => r,
                                None => {
                                    state.current_partition += 1;
                                    continue;
                                }
                            };

                            let mut reader =
                                crate::query::executor::streaming::spill::RunReader::open(run)?;

                            let mut partition_results = Vec::new();
                            // Rebuild accumulators from partial-accumulator rows.
                            let mut group_map: HashMap<Vec<Value>, Vec<AggregateAccumulator>> =
                                HashMap::new();
                            while let Some(row) = reader.read_row()? {
                                let group_key: Vec<Value> =
                                    row.iter().take(num_group_keys).cloned().collect();
                                let accs = group_map.entry(group_key).or_insert_with(|| {
                                    aggregate_functions
                                        .iter()
                                        .map(|(f, _)| {
                                            AggregateAccumulator::for_function(f).expect(
                                                "every aggregate function has an accumulator",
                                            )
                                        })
                                        .collect()
                                });
                                for (i, func) in aggregate_functions.iter().enumerate() {
                                    if let Some(acc) = accs.get_mut(i) {
                                        let partial_value = row
                                            .get(num_group_keys + i)
                                            .cloned()
                                            .unwrap_or(Value::Null(NullType::Null));
                                        if let Some(other) =
                                            value_to_partial_accumulator(&func.0, &partial_value)
                                        {
                                            acc.merge(&other);
                                        }
                                    }
                                }
                            }
                            for (group_key, accs) in group_map {
                                let mut result_row = if has_group_keys {
                                    group_key
                                } else {
                                    Vec::new()
                                };
                                for acc in accs {
                                    result_row.push(acc.finalize());
                                }
                                partition_results.push(result_row);
                            }

                            let _ = std::fs::remove_file(&run.path);
                            state.current_partition += 1;

                            if !partition_results.is_empty() {
                                state.result_iter = Some(partition_results.into_iter());
                                let chunk_rows: Vec<Vec<Value>> = state
                                    .result_iter
                                    .as_mut()
                                    .unwrap()
                                    .by_ref()
                                    .take(1024)
                                    .collect();
                                if !chunk_rows.is_empty() {
                                    return Ok(Some(DataChunk::new_with_layout(
                                        chunk_rows,
                                        Arc::clone(&base.output_layout),
                                    )));
                                }
                                state.result_iter = None;
                            }
                        }
                        state.output_complete = true;
                        return Ok(None);
                    }

                    // Accumulation phase: read input and aggregate.
                    // Once the memory budget is exhausted the remaining input is
                    // routed directly to the partition spiller (spill mode).
                    let mut accumulating = true;
                    while accumulating {
                        match input.advance()? {
                            Some(mut chunk) => {
                                base.ensure_not_cancelled()?;
                                if state.col_names.is_empty() {
                                    state.col_names = chunk.col_names();
                                }
                                let sm = base.spill_manager();
                                // Columnar fast path: batch-evaluate the
                                // group keys and aggregate arguments once
                                // per chunk on the typed columns (no per-row
                                // `ValueRowContext` construction). Chunks
                                // with a selection vector fall back to
                                // per-row evaluation.
                                let batch_eval: BatchEvalResult =
                                    if chunk.selection.is_none() && !chunk.rows.is_empty() {
                                        match chunk.evaluate_expressions(group_by_expressions, None)
                                        {
                                            Ok(keys) => {
                                                let mut args =
                                                    Vec::with_capacity(aggregate_functions.len());
                                                let mut ok = true;
                                                for (_func, expr) in aggregate_functions.iter() {
                                                    match chunk.evaluate_expression(expr, None) {
                                                        Ok(col) => args.push(col),
                                                        Err(_) => {
                                                            ok = false;
                                                            break;
                                                        }
                                                    }
                                                }
                                                if ok {
                                                    Some((keys, args))
                                                } else {
                                                    None
                                                }
                                            }
                                            Err(_) => None,
                                        }
                                    } else {
                                        None
                                    };

                                // Consume the child's selection vector — only
                                // visible rows are aggregated (no row moves).
                                for idx in chunk.visible_indices() {
                                    let row = &chunk.rows[idx];
                                    let group_key: Vec<Value> = match &batch_eval {
                                        Some((keys, _)) => {
                                            keys.iter().map(|c| c[idx].clone()).collect()
                                        }
                                        None => eval_group_key(row, &state.col_names),
                                    };
                                    let arg_values: Vec<Value> = match &batch_eval {
                                        Some((_, args)) => {
                                            args.iter().map(|c| c[idx].clone()).collect()
                                        }
                                        None => {
                                            let mut values =
                                                Vec::with_capacity(aggregate_functions.len());
                                            for (_func, expr) in aggregate_functions.iter() {
                                                let mut ctx = ValueRowContext::from_names(
                                                    row.to_vec(),
                                                    state.col_names.clone(),
                                                );
                                                values.push(
                                                    match ExpressionEvaluator::evaluate(
                                                        expr, &mut ctx,
                                                    ) {
                                                        Ok(v) => v,
                                                        Err(_) => Value::Null(NullType::Null),
                                                    },
                                                );
                                            }
                                            values
                                        }
                                    };
                                    let partial_row_of =
                                        |group_key: &[Value], arg_values: &[Value]| -> Vec<Value> {
                                            let mut partial_row = group_key.to_vec();
                                            for (i, (func, _)) in
                                                aggregate_functions.iter().enumerate()
                                            {
                                                let value = arg_values
                                                    .get(i)
                                                    .cloned()
                                                    .unwrap_or_else(|| Value::Null(NullType::Null));
                                                let mut acc = AggregateAccumulator::for_function(
                                                    func,
                                                )
                                                .expect(
                                                    "every aggregate function has an accumulator",
                                                );
                                                acc.accumulate(&value);
                                                partial_row.push(accumulator_to_value(&acc));
                                            }
                                            partial_row
                                        };
                                    if let Some(ref mut spiller) = state.partition_spiller {
                                        // Spill mode: route row directly, keeping
                                        // the remainder of this chunk intact.
                                        let manager = sm.clone().ok_or_else(|| {
                                            QueryError::execution(
                                                "Spill manager not available".to_string(),
                                            )
                                        })?;
                                        let p =
                                        crate::query::executor::streaming::spill::hash_row_partition(
                                            &group_key,
                                            spiller.num_partitions(),
                                        ) as usize;
                                        let partial_row = partial_row_of(&group_key, &arg_values);
                                        spiller.insert_row_to_partition(
                                            &partial_row,
                                            p,
                                            &manager,
                                        )?;
                                        continue;
                                    }
                                    if !state.group_map.contains_key(&group_key) {
                                        if let Err(e) = memory_tracker.try_reserve(
                                            MemoryBudget::estimate_row_memory(&group_key)
                                                + group_overhead,
                                        ) {
                                            if let Some(sm) = base.spill_manager() {
                                                let config = HashPartitionConfig::default();
                                                let num_partitions = config.num_partitions;
                                                let mut spiller =
                                                    HashPartitionSpiller::new(config, &sm, 0)?;

                                                // Spill accumulated groups as partial-accumulator rows.
                                                for (key, accs) in
                                                    std::mem::take(&mut state.group_map)
                                                {
                                                    let p = crate::query::executor::streaming::spill::hash_row_partition(
                                                &key,
                                                num_partitions,
                                            ) as usize;
                                                    let mut partial_row = key.clone();
                                                    for acc in &accs {
                                                        partial_row.push(accumulator_to_value(acc));
                                                    }
                                                    spiller.insert_row_to_partition(
                                                        &partial_row,
                                                        p,
                                                        &sm,
                                                    )?;
                                                    memory_tracker.release(
                                                        MemoryBudget::estimate_row_memory(&key)
                                                            + group_overhead,
                                                    );
                                                }

                                                // Route current row by group key hash.
                                                let p = crate::query::executor::streaming::spill::hash_row_partition(
                                            &group_key,
                                            num_partitions,
                                        ) as usize;
                                                let partial_row =
                                                    partial_row_of(&group_key, &arg_values);
                                                spiller.insert_row_to_partition(
                                                    &partial_row,
                                                    p,
                                                    &sm,
                                                )?;

                                                state.partition_spiller = Some(spiller);
                                                state.has_spilled = true;
                                                // Current row already routed above.
                                                continue;
                                            } else {
                                                return Err(e);
                                            }
                                        }
                                    }

                                    let accs =
                                        state.group_map.entry(group_key).or_insert_with(|| {
                                            aggregate_functions
                                                .iter()
                                                .map(|(f, _)| {
                                                    AggregateAccumulator::for_function(f).expect(
                                                "every aggregate function has an accumulator",
                                            )
                                                })
                                                .collect()
                                        });
                                    for (i, (_func, _expr)) in
                                        aggregate_functions.iter().enumerate()
                                    {
                                        if let Some(acc) = accs.get_mut(i) {
                                            let value = arg_values
                                                .get(i)
                                                .cloned()
                                                .unwrap_or_else(|| Value::Null(NullType::Null));
                                            acc.accumulate(&value);
                                        }
                                    }
                                }
                            }
                            None => {
                                accumulating = false;
                            }
                        }
                    }

                    // Input is fully consumed above (accumulated or spilled).
                    // Finalize spilled runs and replay them within this same call;
                    // the executor protocol treats Ok(None) as end-of-stream.
                    if state.partition_spiller.is_some() {
                        let runs = state.partition_spiller.take().unwrap().finalize()?;
                        state.spilled_runs = runs;
                        state.current_partition = 0;
                        continue;
                    }

                    // In-memory output: finalize accumulated groups
                    let group_map = std::mem::take(&mut state.group_map);
                    let mut result_rows = Vec::new();
                    for (group_key, accs) in group_map {
                        let mut result_row = if has_group_keys {
                            group_key
                        } else {
                            Vec::new()
                        };
                        for acc in accs {
                            result_row.push(acc.finalize());
                        }
                        result_rows.push(result_row);
                    }

                    let mut result_iter = result_rows.into_iter();
                    let chunk_rows: Vec<Vec<Value>> = result_iter.by_ref().take(1024).collect();
                    state.result_iter = Some(result_iter);
                    if chunk_rows.is_empty() {
                        state.output_complete = true;
                        return Ok(None);
                    }
                    return Ok(Some(DataChunk::new_with_layout(
                        chunk_rows,
                        Arc::clone(&base.output_layout),
                    )));
                }
            }

            Self::GroupBy {
                group_by_expressions,
                memory_tracker,
                state,
                ..
            } => {
                use crate::query::executor::streaming::spill::RunReader;
                let state = state.as_mut().unwrap();

                // Evaluate the group key of a row against `col_names`.
                let eval_group_key = |row: &[Value], col_names: &[String]| -> Vec<Value> {
                    let mut key = Vec::with_capacity(group_by_expressions.len());
                    for expr in group_by_expressions.iter() {
                        let mut ctx = ValueRowContext::from_names(row.to_vec(), col_names.to_vec());
                        key.push(
                            ExpressionEvaluator::evaluate(expr, &mut ctx)
                                .unwrap_or(Value::Null(NullType::Null)),
                        );
                    }
                    key
                };

                // Group rows by their key and flatten values of each group.
                let group_rows = |rows: Vec<Vec<Value>>, col_names: &[String]| -> Vec<Vec<Value>> {
                    let mut groups: HashMap<String, Vec<Vec<Value>>> = HashMap::new();
                    for row in rows {
                        let key_parts: Vec<String> = eval_group_key(&row, col_names)
                            .iter()
                            .map(|v| format!("{:?}", v))
                            .collect();
                        let key = key_parts.join("|");
                        groups.entry(key).or_default().push(row);
                    }
                    groups.into_values().flatten().collect()
                };

                loop {
                    if state.output_complete {
                        return Ok(None);
                    }

                    // Output phase: drain the result iterator.
                    if let Some(ref mut iter) = state.result_iter {
                        let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                        if chunk_rows.is_empty() {
                            state.result_iter = None;
                            if !state.has_spilled
                                || state.current_partition >= state.spilled_runs.len()
                            {
                                state.output_complete = true;
                                return Ok(None);
                            }
                        } else {
                            return Ok(Some(DataChunk::new_with_layout(
                                chunk_rows,
                                Arc::clone(&base.output_layout),
                            )));
                        }
                    }

                    // Replay phase: group each spilled partition in memory.
                    if state.has_spilled && state.partition_spiller.is_none() {
                        while state.current_partition < state.spilled_runs.len() {
                            base.ensure_not_cancelled()?;

                            let run = match &state.spilled_runs[state.current_partition] {
                                Some(r) => r,
                                None => {
                                    state.current_partition += 1;
                                    continue;
                                }
                            };

                            let mut reader = RunReader::open(run)?;
                            let mut partition_rows = Vec::new();
                            while let Some(row) = reader.read_row()? {
                                memory_tracker.try_reserve_row(&row)?;
                                partition_rows.push(row);
                            }

                            let col_names = if state.col_names.is_empty() {
                                (0..partition_rows.first().map_or(0, |r| r.len()))
                                    .map(|i| format!("col_{}", i))
                                    .collect()
                            } else {
                                state.col_names.clone()
                            };
                            let result_rows = group_rows(partition_rows, &col_names);

                            let _ = std::fs::remove_file(&run.path);
                            state.current_partition += 1;
                            memory_tracker.reset();

                            if !result_rows.is_empty() {
                                state.result_iter = Some(result_rows.into_iter());
                                let chunk_rows: Vec<Vec<Value>> = state
                                    .result_iter
                                    .as_mut()
                                    .unwrap()
                                    .by_ref()
                                    .take(1024)
                                    .collect();
                                if !chunk_rows.is_empty() {
                                    return Ok(Some(DataChunk::new_with_layout(
                                        chunk_rows,
                                        Arc::clone(&base.output_layout),
                                    )));
                                }
                                state.result_iter = None;
                            }
                        }
                        state.output_complete = true;
                        return Ok(None);
                    }

                    // Accumulation phase: read input into memory until the
                    // budget is exhausted, then route to the partition spiller.
                    let mut accumulating = true;
                    while accumulating {
                        match input.advance()? {
                            Some(chunk) => {
                                base.ensure_not_cancelled()?;
                                if state.col_names.is_empty() {
                                    state.col_names = match chunk.col_names() {
                                        names if !names.is_empty() => names,
                                        _ => (0..chunk.rows.first().map_or(0, |r| r.len()))
                                            .map(|i| format!("col_{}", i))
                                            .collect(),
                                    };
                                }
                                for row in chunk.rows {
                                    if let Some(ref mut spiller) = state.partition_spiller {
                                        let manager = base.spill_manager().ok_or_else(|| {
                                            QueryError::execution(
                                                "Spill manager not available".to_string(),
                                            )
                                        })?;
                                        let group_key = eval_group_key(&row, &state.col_names);
                                        let p = crate::query::executor::streaming::spill::hash_row_partition(
                                            &group_key,
                                            spiller.num_partitions(),
                                        ) as usize;
                                        spiller.insert_row_to_partition(&row, p, &manager)?;
                                        continue;
                                    }
                                    if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                        if let Some(sm) = base.spill_manager() {
                                            let config = HashPartitionConfig::default();
                                            let num_partitions = config.num_partitions;
                                            let mut spiller =
                                                HashPartitionSpiller::new(config, &sm, 0)?;

                                            // Spill accumulated rows by group-key hash.
                                            for pending in std::mem::take(&mut state.all_rows) {
                                                let group_key =
                                                    eval_group_key(&pending, &state.col_names);
                                                let p = crate::query::executor::streaming::spill::hash_row_partition(
                                                    &group_key,
                                                    num_partitions,
                                                ) as usize;
                                                spiller
                                                    .insert_row_to_partition(&pending, p, &sm)?;
                                                memory_tracker.release(
                                                    MemoryBudget::estimate_row_memory(&pending),
                                                );
                                            }

                                            // Route the current row by group-key hash.
                                            let group_key = eval_group_key(&row, &state.col_names);
                                            let p = crate::query::executor::streaming::spill::hash_row_partition(
                                                &group_key,
                                                num_partitions,
                                            ) as usize;
                                            spiller.insert_row_to_partition(&row, p, &sm)?;

                                            state.partition_spiller = Some(spiller);
                                            state.has_spilled = true;
                                            continue;
                                        } else {
                                            return Err(e);
                                        }
                                    }
                                    state.all_rows.push(row);
                                }
                            }
                            None => {
                                accumulating = false;
                            }
                        }
                    }

                    // Finalize spilled runs and replay them within the loop.
                    if state.partition_spiller.is_some() {
                        let runs = state.partition_spiller.take().unwrap().finalize()?;
                        state.spilled_runs = runs;
                        state.current_partition = 0;
                        continue;
                    }

                    // In-memory output: group all accumulated rows.
                    if state.all_rows.is_empty() {
                        state.output_complete = true;
                        return Ok(None);
                    }
                    let col_names = if state.col_names.is_empty() {
                        (0..state.all_rows[0].len())
                            .map(|i| format!("col_{}", i))
                            .collect()
                    } else {
                        state.col_names.clone()
                    };
                    let result_rows = group_rows(std::mem::take(&mut state.all_rows), &col_names);
                    let mut result_iter = result_rows.into_iter();
                    let chunk_rows: Vec<Vec<Value>> = result_iter.by_ref().take(1024).collect();
                    state.result_iter = Some(result_iter);
                    if chunk_rows.is_empty() {
                        state.output_complete = true;
                        return Ok(None);
                    }
                    return Ok(Some(DataChunk::new_with_layout(
                        chunk_rows,
                        Arc::clone(&base.output_layout),
                    )));
                }
            }

            Self::WindowFunction {
                window_exprs,
                partition_by_exprs,
                order_by_exprs,
                order_by_directions,
                memory_tracker,
                state,
                ..
            } => {
                use crate::query::executor::streaming::spill::RunReader;
                let state = state.as_mut().unwrap();

                // Evaluate the partition key of a row.
                let eval_partition_key = |row: &[Value], col_names: &[String]| -> Vec<Value> {
                    if partition_by_exprs.is_empty() {
                        return vec![Value::Null(NullType::Null)];
                    }
                    let mut key = Vec::with_capacity(partition_by_exprs.len());
                    for expr in partition_by_exprs.iter() {
                        let mut ctx = ValueRowContext::from_names(row.to_vec(), col_names.to_vec());
                        key.push(
                            ExpressionEvaluator::evaluate(expr, &mut ctx)
                                .unwrap_or(Value::Null(NullType::Null)),
                        );
                    }
                    key
                };

                loop {
                    if state.output_complete {
                        return Ok(None);
                    }

                    // Output phase: drain the result iterator.
                    if let Some(ref mut iter) = state.result_iter {
                        let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                        if chunk_rows.is_empty() {
                            state.result_iter = None;
                            if !state.has_spilled
                                || state.current_partition >= state.spilled_runs.len()
                            {
                                state.output_complete = true;
                                return Ok(None);
                            }
                        } else {
                            return Ok(Some(DataChunk::new_with_layout(
                                chunk_rows,
                                Arc::clone(&base.output_layout),
                            )));
                        }
                    }

                    // Replay phase: compute each spilled partition in memory.
                    if state.has_spilled && state.partition_spiller.is_none() {
                        while state.current_partition < state.spilled_runs.len() {
                            base.ensure_not_cancelled()?;

                            let run = match &state.spilled_runs[state.current_partition] {
                                Some(r) => r,
                                None => {
                                    state.current_partition += 1;
                                    continue;
                                }
                            };

                            let mut reader = RunReader::open(run)?;
                            // A run may hold several distinct partition keys
                            // (hash collisions); re-group rows by their exact
                            // partition key before computing window results.
                            let mut partitions: BTreeMap<Vec<Value>, Vec<(usize, Vec<Value>)>> =
                                BTreeMap::new();
                            let mut run_row_count: u64 = 0;
                            while let Some(row) = reader.read_row()? {
                                memory_tracker.try_reserve_row(&row)?;
                                let partition_key = eval_partition_key(&row, &state.col_names);
                                partitions
                                    .entry(partition_key)
                                    .or_default()
                                    .push((run_row_count as usize, row));
                                run_row_count += 1;
                            }

                            let mut result_rows = Vec::with_capacity(run_row_count as usize);
                            for (_key, mut partition_rows) in partitions {
                                sort_partition_rows(
                                    &mut partition_rows,
                                    &state.col_names,
                                    order_by_exprs,
                                    order_by_directions,
                                );
                                result_rows.extend(compute_window_partition_result(
                                    &partition_rows,
                                    &state.col_names,
                                    window_exprs,
                                ));
                            }

                            let _ = std::fs::remove_file(&run.path);
                            state.current_partition += 1;
                            memory_tracker.reset();

                            if !result_rows.is_empty() {
                                state.result_iter = Some(result_rows.into_iter());
                                let chunk_rows: Vec<Vec<Value>> = state
                                    .result_iter
                                    .as_mut()
                                    .unwrap()
                                    .by_ref()
                                    .take(1024)
                                    .collect();
                                if !chunk_rows.is_empty() {
                                    return Ok(Some(DataChunk::new_with_layout(
                                        chunk_rows,
                                        Arc::clone(&base.output_layout),
                                    )));
                                }
                                state.result_iter = None;
                            }
                        }
                        state.output_complete = true;
                        return Ok(None);
                    }

                    // Accumulation phase: read input into memory until the
                    // budget is exhausted, then route to the partition spiller.
                    let mut accumulating = true;
                    while accumulating {
                        match input.advance()? {
                            Some(mut chunk) => {
                                chunk.materialize_selection_by("WindowFunction");
                                base.ensure_not_cancelled()?;
                                if state.col_names.is_empty() {
                                    state.col_names = chunk.col_names();
                                }
                                for row in chunk.rows {
                                    if let Some(ref mut spiller) = state.partition_spiller {
                                        let manager = base.spill_manager().ok_or_else(|| {
                                            QueryError::execution(
                                                "Spill manager not available".to_string(),
                                            )
                                        })?;
                                        let partition_key =
                                            eval_partition_key(&row, &state.col_names);
                                        let p =
                                        crate::query::executor::streaming::spill::hash_row_partition(
                                            &partition_key,
                                            spiller.num_partitions(),
                                        ) as usize;
                                        spiller.insert_row_to_partition(&row, p, &manager)?;
                                        continue;
                                    }
                                    if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                        if let Some(sm) = base.spill_manager() {
                                            let config = HashPartitionConfig::default();
                                            let num_partitions = config.num_partitions;
                                            let mut spiller =
                                                HashPartitionSpiller::new(config, &sm, 0)?;

                                            // Spill accumulated rows by partition-key hash.
                                            for pending in std::mem::take(&mut state.all_rows) {
                                                let partition_key =
                                                    eval_partition_key(&pending, &state.col_names);
                                                let p =
                                                crate::query::executor::streaming::spill::hash_row_partition(
                                                    &partition_key,
                                                    num_partitions,
                                                ) as usize;
                                                spiller
                                                    .insert_row_to_partition(&pending, p, &sm)?;
                                                memory_tracker.release(
                                                    MemoryBudget::estimate_row_memory(&pending),
                                                );
                                            }

                                            // Route the current row as well.
                                            let partition_key =
                                                eval_partition_key(&row, &state.col_names);
                                            let p =
                                            crate::query::executor::streaming::spill::hash_row_partition(
                                                &partition_key,
                                                num_partitions,
                                            ) as usize;
                                            spiller.insert_row_to_partition(&row, p, &sm)?;

                                            state.partition_spiller = Some(spiller);
                                            state.has_spilled = true;
                                            continue;
                                        } else {
                                            return Err(e);
                                        }
                                    }
                                    state.all_rows.push(row);
                                }
                            }
                            None => {
                                accumulating = false;
                            }
                        }
                    }

                    // Finalize spilled runs and replay them within the loop.
                    if state.partition_spiller.is_some() {
                        let runs = state.partition_spiller.take().unwrap().finalize()?;
                        state.spilled_runs = runs;
                        state.current_partition = 0;
                        continue;
                    }

                    // In-memory output: partition, order and compute windows.
                    if state.all_rows.is_empty() {
                        state.output_complete = true;
                        return Ok(None);
                    }
                    let mut partitions: BTreeMap<Vec<Value>, Vec<(usize, Vec<Value>)>> =
                        BTreeMap::new();
                    for (idx, row) in std::mem::take(&mut state.all_rows).into_iter().enumerate() {
                        let partition_key = eval_partition_key(&row, &state.col_names);
                        partitions
                            .entry(partition_key)
                            .or_default()
                            .push((idx, row));
                    }
                    let mut result_rows = Vec::new();
                    for (_key, mut partition_rows) in partitions {
                        sort_partition_rows(
                            &mut partition_rows,
                            &state.col_names,
                            order_by_exprs,
                            order_by_directions,
                        );
                        result_rows.extend(compute_window_partition_result(
                            &partition_rows,
                            &state.col_names,
                            window_exprs,
                        ));
                    }

                    let mut result_iter = result_rows.into_iter();
                    let chunk_rows: Vec<Vec<Value>> = result_iter.by_ref().take(1024).collect();
                    state.result_iter = Some(result_iter);
                    if chunk_rows.is_empty() {
                        state.output_complete = true;
                        return Ok(None);
                    }
                    return Ok(Some(DataChunk::new_with_layout(
                        chunk_rows,
                        Arc::clone(&base.output_layout),
                    )));
                }
            }

            Self::Window {
                window_exprs,
                partition_by_exprs,
                order_by_exprs,
                order_by_directions,
                memory_tracker,
                state,
                ..
            } => {
                use crate::query::executor::streaming::spill::RunReader;
                let state = state.as_mut().unwrap();

                // Evaluate the partition key of a row.
                let eval_partition_key = |row: &[Value], col_names: &[String]| -> Vec<Value> {
                    if partition_by_exprs.is_empty() {
                        return vec![Value::Null(NullType::Null)];
                    }
                    let mut key = Vec::with_capacity(partition_by_exprs.len());
                    for expr in partition_by_exprs.iter() {
                        let mut ctx = ValueRowContext::from_names(row.to_vec(), col_names.to_vec());
                        key.push(
                            ExpressionEvaluator::evaluate(expr, &mut ctx)
                                .unwrap_or(Value::Null(NullType::Null)),
                        );
                    }
                    key
                };

                loop {
                    if state.output_complete {
                        return Ok(None);
                    }

                    // Output phase: drain the result iterator.
                    if let Some(ref mut iter) = state.result_iter {
                        let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                        if chunk_rows.is_empty() {
                            state.result_iter = None;
                            if !state.has_spilled
                                || state.current_partition >= state.spilled_runs.len()
                            {
                                state.output_complete = true;
                                return Ok(None);
                            }
                        } else {
                            return Ok(Some(DataChunk::new_with_layout(
                                chunk_rows,
                                Arc::clone(&base.output_layout),
                            )));
                        }
                    }

                    // Replay phase: compute each spilled partition in memory.
                    if state.has_spilled && state.partition_spiller.is_none() {
                        while state.current_partition < state.spilled_runs.len() {
                            base.ensure_not_cancelled()?;

                            let run = match &state.spilled_runs[state.current_partition] {
                                Some(r) => r,
                                None => {
                                    state.current_partition += 1;
                                    continue;
                                }
                            };

                            let mut reader = RunReader::open(run)?;
                            // A run may hold several distinct partition keys
                            // (hash collisions); re-group rows by their exact
                            // partition key before computing window results.
                            let mut partitions: BTreeMap<Vec<Value>, Vec<(usize, Vec<Value>)>> =
                                BTreeMap::new();
                            let mut run_row_count: u64 = 0;
                            while let Some(row) = reader.read_row()? {
                                memory_tracker.try_reserve_row(&row)?;
                                let partition_key = eval_partition_key(&row, &state.col_names);
                                partitions
                                    .entry(partition_key)
                                    .or_default()
                                    .push((run_row_count as usize, row));
                                run_row_count += 1;
                            }

                            let mut result_rows = Vec::with_capacity(run_row_count as usize);
                            for (_key, mut partition_rows) in partitions {
                                sort_partition_rows(
                                    &mut partition_rows,
                                    &state.col_names,
                                    order_by_exprs,
                                    order_by_directions,
                                );
                                result_rows.extend(compute_window_partition_result(
                                    &partition_rows,
                                    &state.col_names,
                                    window_exprs,
                                ));
                            }

                            let _ = std::fs::remove_file(&run.path);
                            state.current_partition += 1;
                            memory_tracker.reset();

                            if !result_rows.is_empty() {
                                state.result_iter = Some(result_rows.into_iter());
                                let chunk_rows: Vec<Vec<Value>> = state
                                    .result_iter
                                    .as_mut()
                                    .unwrap()
                                    .by_ref()
                                    .take(1024)
                                    .collect();
                                if !chunk_rows.is_empty() {
                                    return Ok(Some(DataChunk::new_with_layout(
                                        chunk_rows,
                                        Arc::clone(&base.output_layout),
                                    )));
                                }
                                state.result_iter = None;
                            }
                        }
                        state.output_complete = true;
                        return Ok(None);
                    }

                    // Accumulation phase: read input into memory until the
                    // budget is exhausted, then route to the partition spiller.
                    let mut accumulating = true;
                    while accumulating {
                        match input.advance()? {
                            Some(mut chunk) => {
                                chunk.materialize_selection_by("Window");
                                base.ensure_not_cancelled()?;
                                if state.col_names.is_empty() {
                                    state.col_names = chunk.col_names();
                                }
                                for row in chunk.rows {
                                    if let Some(ref mut spiller) = state.partition_spiller {
                                        let manager = base.spill_manager().ok_or_else(|| {
                                            QueryError::execution(
                                                "Spill manager not available".to_string(),
                                            )
                                        })?;
                                        let partition_key =
                                            eval_partition_key(&row, &state.col_names);
                                        let p =
                                        crate::query::executor::streaming::spill::hash_row_partition(
                                            &partition_key,
                                            spiller.num_partitions(),
                                        ) as usize;
                                        spiller.insert_row_to_partition(&row, p, &manager)?;
                                        continue;
                                    }
                                    if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                        if let Some(sm) = base.spill_manager() {
                                            let config = HashPartitionConfig::default();
                                            let num_partitions = config.num_partitions;
                                            let mut spiller =
                                                HashPartitionSpiller::new(config, &sm, 0)?;

                                            // Spill accumulated rows by partition-key hash.
                                            for pending in std::mem::take(&mut state.all_rows) {
                                                let partition_key =
                                                    eval_partition_key(&pending, &state.col_names);
                                                let p =
                                                crate::query::executor::streaming::spill::hash_row_partition(
                                                    &partition_key,
                                                    num_partitions,
                                                ) as usize;
                                                spiller
                                                    .insert_row_to_partition(&pending, p, &sm)?;
                                                memory_tracker.release(
                                                    MemoryBudget::estimate_row_memory(&pending),
                                                );
                                            }

                                            // Route the current row as well.
                                            let partition_key =
                                                eval_partition_key(&row, &state.col_names);
                                            let p =
                                            crate::query::executor::streaming::spill::hash_row_partition(
                                                &partition_key,
                                                num_partitions,
                                            ) as usize;
                                            spiller.insert_row_to_partition(&row, p, &sm)?;

                                            state.partition_spiller = Some(spiller);
                                            state.has_spilled = true;
                                            continue;
                                        } else {
                                            return Err(e);
                                        }
                                    }
                                    state.all_rows.push(row);
                                }
                            }
                            None => {
                                accumulating = false;
                            }
                        }
                    }

                    // Finalize spilled runs and replay them within the loop.
                    if state.partition_spiller.is_some() {
                        let runs = state.partition_spiller.take().unwrap().finalize()?;
                        state.spilled_runs = runs;
                        state.current_partition = 0;
                        continue;
                    }

                    // In-memory output: partition, order and compute windows.
                    if state.all_rows.is_empty() {
                        state.output_complete = true;
                        return Ok(None);
                    }
                    let mut partitions: BTreeMap<Vec<Value>, Vec<(usize, Vec<Value>)>> =
                        BTreeMap::new();
                    for (idx, row) in std::mem::take(&mut state.all_rows).into_iter().enumerate() {
                        let partition_key = eval_partition_key(&row, &state.col_names);
                        partitions
                            .entry(partition_key)
                            .or_default()
                            .push((idx, row));
                    }
                    let mut result_rows = Vec::new();
                    for (_key, mut partition_rows) in partitions {
                        sort_partition_rows(
                            &mut partition_rows,
                            &state.col_names,
                            order_by_exprs,
                            order_by_directions,
                        );
                        result_rows.extend(compute_window_partition_result(
                            &partition_rows,
                            &state.col_names,
                            window_exprs,
                        ));
                    }

                    let mut result_iter = result_rows.into_iter();
                    let chunk_rows: Vec<Vec<Value>> = result_iter.by_ref().take(1024).collect();
                    state.result_iter = Some(result_iter);
                    if chunk_rows.is_empty() {
                        state.output_complete = true;
                        return Ok(None);
                    }
                    return Ok(Some(DataChunk::new_with_layout(
                        chunk_rows,
                        Arc::clone(&base.output_layout),
                    )));
                }
            }

            Self::TopN {
                n,
                sort_expressions,
                sort_directions,
                memory_tracker,
                state,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("TopN not opened".to_string()));
                }

                let state = state.as_mut().unwrap();
                if state.result_iter.is_none() {
                    let limit = *n as usize;

                    while let Some(mut chunk) = input.advance()? {
                        chunk.materialize_selection_by("TopN");
                        base.ensure_not_cancelled()?;
                        if state.col_names.is_empty() {
                            state.col_names = chunk.col_names();
                            state.input_layout = Some(chunk.get_layout());
                        }
                        // Bounded columnar accumulation: account each row
                        // against the budget (error propagates, as before),
                        // append the chunk, then sort + truncate back to
                        // `limit`.
                        let batch = state
                            .columnar_batch
                            .get_or_insert_with(|| ColumnarBatch::new(chunk.num_columns()));
                        for idx in chunk.visible_indices() {
                            memory_tracker.try_reserve_row(&chunk.rows[idx])?;
                            batch.append_chunk_row(&chunk, idx);
                        }
                        if batch.num_rows() > limit {
                            if !sort_expressions.is_empty() {
                                sort_columnar_batch(
                                    batch,
                                    &state.col_names,
                                    sort_expressions,
                                    sort_directions,
                                );
                            }
                            batch.truncate(limit);
                        }
                    }

                    if let Some(mut batch) = state.columnar_batch.take() {
                        if !sort_expressions.is_empty() && batch.num_rows() > 1 {
                            sort_columnar_batch(
                                &mut batch,
                                &state.col_names,
                                sort_expressions,
                                sort_directions,
                            );
                        }
                        state.result_iter = Some(batch.to_rows().into_iter());
                    } else {
                        state.result_iter = Some(Vec::new().into_iter());
                    }
                }

                if let Some(iter) = &mut state.result_iter {
                    if let Some(row) = iter.next() {
                        Ok(Some(DataChunk::new_with_layout(
                            vec![row],
                            Arc::clone(&base.output_layout),
                        )))
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }

            Self::Distinct {
                state,
                memory_tracker,
                ..
            } => {
                let state = state.as_mut().unwrap();

                // Output phase: return pre-computed results
                if let Some(ref mut iter) = state.output_iter {
                    let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                    if chunk_rows.is_empty() {
                        state.output_iter = None;
                    } else {
                        return Ok(Some(DataChunk::new_with_layout(
                            chunk_rows,
                            Arc::clone(&base.output_layout),
                        )));
                    }
                }

                // Replay phase: process spilled partitions one at a time
                if state.has_spilled && state.partition_spiller.is_none() {
                    while state.current_partition < state.spilled_runs.len() {
                        base.ensure_not_cancelled()?;

                        let run = match &state.spilled_runs[state.current_partition] {
                            Some(r) => r,
                            None => {
                                state.current_partition += 1;
                                continue;
                            }
                        };

                        let mut reader =
                            crate::query::executor::streaming::spill::RunReader::open(run)?;
                        let mut partition_rows = Vec::new();

                        while let Some(row) = reader.read_row()? {
                            if !state.partition_seen.contains(&row) {
                                memory_tracker.try_reserve_row(&row)?;
                                state.partition_seen.insert(row.clone());
                                partition_rows.push(row);
                            }
                        }

                        let _ = std::fs::remove_file(&run.path);
                        state.partition_seen.clear();
                        memory_tracker.reset();
                        state.current_partition += 1;

                        if !partition_rows.is_empty() {
                            state.output_iter = Some(partition_rows.into_iter());
                            let chunk_rows: Vec<Vec<Value>> = state
                                .output_iter
                                .as_mut()
                                .unwrap()
                                .by_ref()
                                .take(1024)
                                .collect();
                            if !chunk_rows.is_empty() {
                                return Ok(Some(DataChunk::new_with_layout(
                                    chunk_rows,
                                    Arc::clone(&base.output_layout),
                                )));
                            }
                            state.output_iter = None;
                        }
                    }
                    return Ok(None);
                }

                // Accumulation phase: read all input, dedup in seen_rows
                let mut accumulating = true;
                while accumulating {
                    match input.advance()? {
                        Some(mut chunk) => {
                            chunk.materialize_selection_by("Distinct");
                            base.ensure_not_cancelled()?;
                            if state.col_names.is_empty() {
                                state.col_names = chunk.col_names();
                                state.input_layout = Some(chunk.get_layout());
                            }
                            for row in chunk.rows {
                                if !state.seen_rows.contains(&row) {
                                    if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                        if let Some(sm) = base.spill_manager() {
                                            let config = HashPartitionConfig::default();
                                            let mut spiller =
                                                HashPartitionSpiller::new(config, &sm, 0)?;

                                            for seen_row in state.seen_rows.drain() {
                                                spiller.insert_row(&seen_row, &sm)?;
                                                memory_tracker.release(
                                                    MemoryBudget::estimate_row_memory(&seen_row),
                                                );
                                            }

                                            spiller.insert_row(&row, &sm)?;
                                            memory_tracker
                                                .release(MemoryBudget::estimate_row_memory(&row));

                                            state.partition_spiller = Some(spiller);
                                            state.has_spilled = true;
                                            accumulating = false;
                                            break;
                                        } else {
                                            return Err(e);
                                        }
                                    }
                                    state.seen_rows.insert(row.clone());
                                }
                            }
                        }
                        None => {
                            accumulating = false;
                        }
                    }
                }

                // Spill consumption phase: route remaining input to partition spiller
                if let Some(ref mut spiller) = state.partition_spiller {
                    while let Some(mut chunk) = input.advance()? {
                        chunk.materialize_selection_by("Distinct");
                        base.ensure_not_cancelled()?;
                        let sm = base.spill_manager().ok_or_else(|| {
                            QueryError::execution("Spill manager not available".to_string())
                        })?;
                        for row in chunk.rows {
                            spiller.insert_row(&row, &sm)?;
                        }
                    }

                    let runs = state.partition_spiller.take().unwrap().finalize()?;
                    state.spilled_runs = runs;
                    state.current_partition = 0;
                    state.partition_seen.clear();

                    return Ok(None);
                }

                // In-memory output phase: drain seen_rows into output iterator
                let unique_rows: Vec<Vec<Value>> = state.seen_rows.drain().collect();
                state.output_iter = Some(unique_rows.into_iter());

                let chunk_rows: Vec<Vec<Value>> = state
                    .output_iter
                    .as_mut()
                    .unwrap()
                    .by_ref()
                    .take(1024)
                    .collect();
                if chunk_rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::new_with_layout(
                        chunk_rows,
                        Arc::clone(&base.output_layout),
                    )))
                }
            }

            Self::Materialize {
                memory_tracker,
                state,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("Materialize not opened".to_string()));
                }

                let state = state.as_mut().unwrap();
                if !state.materialized {
                    while let Some(mut chunk) = input.advance()? {
                        chunk.materialize_selection_by("Materialize");
                        base.ensure_not_cancelled()?;
                        if state.input_layout.is_none() {
                            state.input_layout = Some(chunk.get_layout());
                        }
                        for row in chunk.rows {
                            if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                if let Some(sm) = base.spill_manager() {
                                    spill_not_supported(
                                        &mut state.materialized_rows,
                                        &sm,
                                        &mut state.spill_files,
                                        memory_tracker,
                                    )?;
                                    memory_tracker.try_reserve_row(&row)?;
                                } else {
                                    return Err(e);
                                }
                            }
                            state.materialized_rows.push(row);
                        }
                    }

                    if !state.spill_files.is_empty() {
                        return reject_spill_replay(&state.spill_files).map(|_| None);
                    }

                    state.materialized = true;
                    state.result_iter =
                        Some(std::mem::take(&mut state.materialized_rows).into_iter());
                }

                if let Some(iter) = &mut state.result_iter {
                    let rows: Vec<Vec<Value>> = iter.by_ref().take(base.chunk_size).collect();
                    if !rows.is_empty() {
                        Ok(Some(DataChunk::new_with_layout(
                            rows,
                            Arc::clone(&base.output_layout),
                        )))
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }

            Self::DataCollect {
                memory_tracker,
                state,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("DataCollect not opened".to_string()));
                }

                let state = state.as_mut().unwrap();
                if state.emitted {
                    return Ok(None);
                }

                while let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection_by("DataCollect");
                    base.ensure_not_cancelled()?;
                    if state.input_layout.is_none() {
                        state.input_layout = Some(chunk.get_layout());
                    }
                    for row in chunk.rows {
                        if let Err(e) = memory_tracker.try_reserve_row(&row) {
                            if let Some(sm) = base.spill_manager() {
                                spill_not_supported(
                                    &mut state.all_rows,
                                    &sm,
                                    &mut state.spill_files,
                                    memory_tracker,
                                )?;
                                memory_tracker.try_reserve_row(&row)?;
                            } else {
                                return Err(e);
                            }
                        }
                        state.all_rows.push(row);
                    }
                }

                if !state.spill_files.is_empty() {
                    return reject_spill_replay(&state.spill_files).map(|_| None);
                }

                if !state.all_rows.is_empty() {
                    state.emitted = true;
                    let rows = std::mem::take(&mut state.all_rows);
                    return Ok(Some(DataChunk::new_with_layout(
                        rows,
                        Arc::clone(&base.output_layout),
                    )));
                }

                Ok(None)
            }

            Self::RollUpApply {
                rollup_expressions,
                memory_tracker,
                state,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("RollUpApply not opened".to_string()));
                }

                let state = state.as_mut().unwrap();
                if state.result_iter.is_none() {
                    let mut col_names: Vec<String> = Vec::new();
                    while let Some(mut chunk) = input.advance()? {
                        chunk.materialize_selection_by("RollUpApply");
                        base.ensure_not_cancelled()?;
                        if col_names.is_empty() {
                            col_names = chunk.col_names();
                        }
                        for row in chunk.rows {
                            if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                if let Some(sm) = base.spill_manager() {
                                    spill_not_supported(
                                        &mut state.all_rows,
                                        &sm,
                                        &mut state.spill_files,
                                        memory_tracker,
                                    )?;
                                    memory_tracker.try_reserve_row(&row)?;
                                } else {
                                    return Err(e);
                                }
                            }
                            let mut ctx =
                                ValueRowContext::from_names(row.clone(), col_names.clone());
                            let mut aggregated = row.clone();
                            for expr in rollup_expressions.iter() {
                                match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                    Ok(val) => aggregated.push(val),
                                    Err(_) => aggregated.push(Value::Null(NullType::Null)),
                                }
                            }
                            state.all_rows.push(aggregated);
                        }
                    }

                    if !state.spill_files.is_empty() {
                        return reject_spill_replay(&state.spill_files).map(|_| None);
                    }

                    state.result_iter = Some(std::mem::take(&mut state.all_rows).into_iter());
                }

                if let Some(iter) = &mut state.result_iter {
                    let rows: Vec<Vec<Value>> = iter.by_ref().take(base.chunk_size).collect();
                    if !rows.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(
                            rows,
                            Arc::clone(&base.output_layout),
                        )));
                    }
                }

                Ok(None)
            }

            Self::PartialAggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names: _,
                memory_tracker,
                state,
                ..
            } => {
                let state = state.as_mut().unwrap();
                if state.result_iter.is_none() {
                    let mut col_names: Vec<String> = vec![];
                    while let Some(mut chunk) = input.advance()? {
                        base.ensure_not_cancelled()?;
                        if col_names.is_empty() {
                            col_names = chunk.col_names();
                        }
                        // Consume the child's selection vector.
                        let visible = chunk.visible_indices();
                        for idx in &visible {
                            memory_tracker.try_reserve_row(&chunk.rows[*idx])?;
                        }
                        // Columnar fast path: batch-evaluate group keys once
                        // per chunk (typed columns, no per-row
                        // `ValueRowContext`); selection-bearing chunks fall
                        // back to per-row evaluation. Aggregate argument
                        // fields are resolved to column indices once per
                        // function.
                        let batch_keys: Option<Vec<Vec<Value>>> =
                            if chunk.selection.is_none() && !chunk.rows.is_empty() {
                                chunk.evaluate_expressions(group_by_expressions, None).ok()
                            } else {
                                None
                            };
                        let field_indices: Vec<Option<usize>> = aggregate_functions
                            .iter()
                            .map(|func| match func {
                                // count(*) is a constant 1 (no field access).
                                AggregateFunction::Count(None) => None,
                                AggregateFunction::Count(Some(f))
                                | AggregateFunction::Sum(f)
                                | AggregateFunction::Avg(f)
                                | AggregateFunction::Min(f)
                                | AggregateFunction::Max(f) => {
                                    col_names.iter().position(|c| c == f)
                                }
                                _ => None,
                            })
                            .collect();
                        for idx in visible {
                            let row = &chunk.rows[idx];
                            let mut group_key = Vec::new();
                            if group_by_expressions.is_empty() {
                                group_key.push(Value::Null(NullType::Null));
                            } else {
                                match &batch_keys {
                                    Some(keys) => {
                                        group_key = keys.iter().map(|c| c[idx].clone()).collect();
                                    }
                                    None => {
                                        for expr in group_by_expressions.iter() {
                                            let mut ctx = ValueRowContext::from_names(
                                                row.clone(),
                                                col_names.clone(),
                                            );
                                            match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                                Ok(value) => group_key.push(value),
                                                Err(_) => {
                                                    group_key.push(Value::Null(NullType::Null))
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            let group_accs =
                                state.group_map.entry(group_key).or_insert_with(|| {
                                    aggregate_functions
                                        .iter()
                                        .filter_map(AggregateAccumulator::for_function)
                                        .collect()
                                });

                            for (i, func) in aggregate_functions.iter().enumerate() {
                                if let Some(acc) = group_accs.get_mut(i) {
                                    let value = match &field_indices[i] {
                                        Some(j) => row
                                            .get(*j)
                                            .cloned()
                                            .unwrap_or_else(|| Value::Null(NullType::Null)),
                                        None => match func {
                                            AggregateFunction::Count(None) => Value::Int(1),
                                            _ => Value::Null(NullType::Null),
                                        },
                                    };
                                    acc.accumulate(&value);
                                }
                            }
                        }
                    }

                    let mut result_rows: Vec<Vec<Value>> = Vec::new();
                    let num_group_keys = group_by_expressions.len();
                    for (group_key, accs) in std::mem::take(&mut state.group_map) {
                        let mut row = if num_group_keys == 0 {
                            vec![]
                        } else {
                            group_key
                        };
                        for (i, _func) in aggregate_functions.iter().enumerate() {
                            if let Some(acc) = accs.get(i) {
                                row.push(accumulator_to_value(acc));
                            } else {
                                row.push(Value::Null(NullType::Null));
                            }
                        }
                        result_rows.push(row);
                    }

                    state.result_iter = Some(result_rows.into_iter());
                }

                if let Some(iter) = &mut state.result_iter {
                    let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                    if chunk_rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::new_with_layout(
                            chunk_rows,
                            Arc::clone(&base.output_layout),
                        )))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::FinalAggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names: _,
                memory_tracker,
                state,
                ..
            } => {
                let state = state.as_mut().unwrap();
                if state.result_iter.is_none() {
                    let mut col_names: Vec<String> = vec![];
                    while let Some(chunk) = input.advance()? {
                        base.ensure_not_cancelled()?;
                        if col_names.is_empty() {
                            col_names = chunk.col_names();
                        }
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        for row in chunk.rows {
                            let num_group_keys = group_by_expressions.len();
                            let num_agg_funcs = aggregate_functions.len();
                            let group_key: Vec<Value> = if num_group_keys == 0 {
                                vec![Value::Null(NullType::Null)]
                            } else {
                                row[0..num_group_keys].to_vec()
                            };

                            let group_accs =
                                state.group_map.entry(group_key).or_insert_with(|| {
                                    aggregate_functions
                                        .iter()
                                        .filter_map(AggregateAccumulator::for_function)
                                        .collect()
                                });

                            for (i, func) in
                                aggregate_functions.iter().enumerate().take(num_agg_funcs)
                            {
                                if let Some(acc) = group_accs.get_mut(i) {
                                    let acc_col_idx = num_group_keys + i;
                                    let partial_value = row.get(acc_col_idx);
                                    if let Some(val) = partial_value {
                                        let partial_acc = value_to_partial_accumulator(func, val);
                                        if let Some(other) = partial_acc {
                                            acc.merge(&other);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let mut result_rows: Vec<Vec<Value>> = Vec::new();
                    for (group_key, accs) in std::mem::take(&mut state.group_map) {
                        let mut row = if group_by_expressions.is_empty() {
                            vec![]
                        } else {
                            group_key
                        };
                        for (_i, _func) in aggregate_functions.iter().enumerate() {
                            if let Some(acc) = accs.get(_i) {
                                row.push(acc.finalize());
                            } else {
                                row.push(Value::Null(NullType::Null));
                            }
                        }
                        result_rows.push(row);
                    }

                    state.result_iter = Some(result_rows.into_iter());
                }

                if let Some(iter) = &mut state.result_iter {
                    let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                    if chunk_rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::new_with_layout(
                            chunk_rows,
                            Arc::clone(&base.output_layout),
                        )))
                    }
                } else {
                    Ok(None)
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
        match self {
            Self::Sort {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    if let Some(ref s) = state {
                        for run in &s.runs {
                            let _ = std::fs::remove_file(&run.path);
                        }
                        for sf in &s.spill_files {
                            let _ = std::fs::remove_file(&sf.path);
                        }
                    }
                    memory_tracker.reset();
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::Aggregate {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    if let Some(ref s) = state {
                        for r in s.spilled_runs.iter().flatten() {
                            let _ = std::fs::remove_file(&r.path);
                        }
                    }
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::GroupBy {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    if let Some(ref s) = state {
                        for r in s.spilled_runs.iter().flatten() {
                            let _ = std::fs::remove_file(&r.path);
                        }
                    }
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::WindowFunction {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    if let Some(ref s) = state {
                        for r in s.spilled_runs.iter().flatten() {
                            let _ = std::fs::remove_file(&r.path);
                        }
                    }
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::Window {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    if let Some(ref s) = state {
                        for r in s.spilled_runs.iter().flatten() {
                            let _ = std::fs::remove_file(&r.path);
                        }
                    }
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::TopN {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::RollUpApply {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::Distinct {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    if let Some(ref s) = state {
                        for r in s.spilled_runs.iter().flatten() {
                            let _ = std::fs::remove_file(&r.path);
                        }
                    }
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::PartialAggregate {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::FinalAggregate {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::Materialize {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
            Self::DataCollect {
                state,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    *state = None;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
        }
    }

    pub fn spill_with_manager(&mut self, sm: &SpillManager) -> Result<(), QueryError> {
        match self {
            Self::Sort {
                state,
                memory_tracker,
                sort_expressions,
                sort_directions,
                ..
            } => {
                if let Some(ref mut s) = state {
                    if !s.all_rows.is_empty() {
                        spill_sorted_run(
                            &mut s.all_rows,
                            &s.col_names,
                            sort_expressions,
                            sort_directions,
                            sm,
                            memory_tracker,
                            &mut s.runs,
                        )?;
                        s.has_spilled = true;
                    }
                }
                Ok(())
            }
            Self::Aggregate {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    if s.partition_spiller.is_none() && !s.group_map.is_empty() {
                        let config = HashPartitionConfig::default();
                        let num_partitions = config.num_partitions;
                        let mut spiller = HashPartitionSpiller::new(config, sm, 0)?;
                        for (key, accs) in std::mem::take(&mut s.group_map) {
                            let p = crate::query::executor::streaming::spill::hash_row_partition(
                                &key,
                                num_partitions,
                            ) as usize;
                            let mut partial_row = key.clone();
                            for acc in &accs {
                                partial_row.push(accumulator_to_value(acc));
                            }
                            spiller.insert_row_to_partition(&partial_row, p, sm)?;
                            memory_tracker.release(
                                MemoryBudget::estimate_row_memory(&key) + s.accumulator_overhead,
                            );
                        }
                        s.partition_spiller = Some(spiller);
                        s.has_spilled = true;
                    }
                }
                Ok(())
            }
            Self::GroupBy {
                state,
                memory_tracker,
                group_by_expressions,
                ..
            } => {
                if let Some(ref mut s) = state {
                    if s.partition_spiller.is_none() && !s.all_rows.is_empty() {
                        let config = HashPartitionConfig::default();
                        let num_partitions = config.num_partitions;
                        let mut spiller = HashPartitionSpiller::new(config, sm, 0)?;
                        for row in std::mem::take(&mut s.all_rows) {
                            let mut group_key = Vec::new();
                            for expr in group_by_expressions.iter() {
                                let mut ctx =
                                    ValueRowContext::from_names(row.clone(), s.col_names.clone());
                                group_key.push(
                                    ExpressionEvaluator::evaluate(expr, &mut ctx)
                                        .unwrap_or(Value::Null(NullType::Null)),
                                );
                            }
                            let p = crate::query::executor::streaming::spill::hash_row_partition(
                                &group_key,
                                num_partitions,
                            ) as usize;
                            spiller.insert_row_to_partition(&row, p, sm)?;
                            memory_tracker.release(MemoryBudget::estimate_row_memory(&row));
                        }
                        s.partition_spiller = Some(spiller);
                        s.has_spilled = true;
                    }
                }
                Ok(())
            }
            Self::WindowFunction {
                state,
                memory_tracker,
                partition_by_exprs,
                ..
            } => {
                if let Some(ref mut s) = state {
                    if s.partition_spiller.is_none() && !s.all_rows.is_empty() {
                        let config = HashPartitionConfig::default();
                        let num_partitions = config.num_partitions;
                        let mut spiller = HashPartitionSpiller::new(config, sm, 0)?;
                        for row in std::mem::take(&mut s.all_rows) {
                            let mut partition_key = Vec::new();
                            if partition_by_exprs.is_empty() {
                                partition_key.push(Value::Null(NullType::Null));
                            } else {
                                for expr in partition_by_exprs.iter() {
                                    let mut ctx = ValueRowContext::from_names(
                                        row.clone(),
                                        s.col_names.clone(),
                                    );
                                    partition_key.push(
                                        ExpressionEvaluator::evaluate(expr, &mut ctx)
                                            .unwrap_or(Value::Null(NullType::Null)),
                                    );
                                }
                            }
                            let p = crate::query::executor::streaming::spill::hash_row_partition(
                                &partition_key,
                                num_partitions,
                            ) as usize;
                            spiller.insert_row_to_partition(&row, p, sm)?;
                            memory_tracker.release(MemoryBudget::estimate_row_memory(&row));
                        }
                        s.partition_spiller = Some(spiller);
                        s.has_spilled = true;
                    }
                }
                Ok(())
            }
            Self::Window {
                state,
                memory_tracker,
                partition_by_exprs,
                ..
            } => {
                if let Some(ref mut s) = state {
                    if s.partition_spiller.is_none() && !s.all_rows.is_empty() {
                        let config = HashPartitionConfig::default();
                        let num_partitions = config.num_partitions;
                        let mut spiller = HashPartitionSpiller::new(config, sm, 0)?;
                        for row in std::mem::take(&mut s.all_rows) {
                            let mut partition_key = Vec::new();
                            if partition_by_exprs.is_empty() {
                                partition_key.push(Value::Null(NullType::Null));
                            } else {
                                for expr in partition_by_exprs.iter() {
                                    let mut ctx = ValueRowContext::from_names(
                                        row.clone(),
                                        s.col_names.clone(),
                                    );
                                    partition_key.push(
                                        ExpressionEvaluator::evaluate(expr, &mut ctx)
                                            .unwrap_or(Value::Null(NullType::Null)),
                                    );
                                }
                            }
                            let p = crate::query::executor::streaming::spill::hash_row_partition(
                                &partition_key,
                                num_partitions,
                            ) as usize;
                            spiller.insert_row_to_partition(&row, p, sm)?;
                            memory_tracker.release(MemoryBudget::estimate_row_memory(&row));
                        }
                        s.partition_spiller = Some(spiller);
                        s.has_spilled = true;
                    }
                }
                Ok(())
            }
            Self::TopN { .. } => Ok(()),
            Self::Distinct {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    if !s.seen_rows.is_empty() {
                        let config = HashPartitionConfig::default();
                        let mut spiller = HashPartitionSpiller::new(config, sm, 0)?;
                        for row in s.seen_rows.drain() {
                            spiller.insert_row(&row, sm)?;
                            memory_tracker.release(MemoryBudget::estimate_row_memory(&row));
                        }
                        s.partition_spiller = Some(spiller);
                        s.has_spilled = true;
                    }
                }
                Ok(())
            }
            Self::Materialize {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    spill_not_supported(
                        &mut s.materialized_rows,
                        sm,
                        &mut s.spill_files,
                        memory_tracker,
                    )?;
                }
                Ok(())
            }
            Self::DataCollect {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    spill_not_supported(&mut s.all_rows, sm, &mut s.spill_files, memory_tracker)?;
                }
                Ok(())
            }
            Self::PartialAggregate { state, .. } => {
                if state.as_ref().is_some_and(|s| !s.group_map.is_empty()) {
                    return Err(QueryError::execution(
                        "Partial aggregate spill is not implemented; query memory budget exceeded"
                            .to_string(),
                    ));
                }
                Ok(())
            }
            Self::RollUpApply {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    spill_not_supported(&mut s.all_rows, sm, &mut s.spill_files, memory_tracker)?;
                }
                Ok(())
            }
            Self::FinalAggregate { state, .. } => {
                if state.as_ref().is_some_and(|s| !s.group_map.is_empty()) {
                    return Err(QueryError::execution(
                        "Final aggregate spill is not implemented; query memory budget exceeded"
                            .to_string(),
                    ));
                }
                Ok(())
            }
        }
    }

    pub fn spill_count(&self) -> u64 {
        match self {
            Self::Sort { state, .. } => state.as_ref().map_or(0, |s| s.runs.len() as u64),
            Self::Aggregate { state, .. } => state.as_ref().map_or(0, |s| {
                s.spilled_runs.iter().filter_map(|r| r.as_ref()).count() as u64
            }),
            Self::GroupBy { state, .. } => state.as_ref().map_or(0, |s| {
                s.spilled_runs.iter().filter_map(|r| r.as_ref()).count() as u64
            }),
            Self::WindowFunction { state, .. } => state.as_ref().map_or(0, |s| {
                s.spilled_runs.iter().filter_map(|r| r.as_ref()).count() as u64
            }),
            Self::Window { state, .. } => state.as_ref().map_or(0, |s| {
                s.spilled_runs.iter().filter_map(|r| r.as_ref()).count() as u64
            }),
            Self::Distinct { state, .. } => state.as_ref().map_or(0, |s| {
                s.spilled_runs.iter().filter_map(|r| r.as_ref()).count() as u64
            }),
            _ => 0,
        }
    }

    pub fn spilled_bytes(&self) -> u64 {
        macro_rules! sum_spill {
            ($state:expr) => {
                $state.as_ref().map_or(0, |s| {
                    s.spill_files.iter().map(|f| f.byte_size).sum::<u64>()
                })
            };
        }
        match self {
            Self::Sort { state, .. } => {
                let base = sum_spill!(state);
                let run_bytes: u64 = state
                    .as_ref()
                    .map_or(0, |s| s.runs.iter().map(|r| r.byte_size).sum::<u64>());
                base + run_bytes
            }
            Self::Aggregate { state, .. } => {
                let base = sum_spill!(state);
                let run_bytes: u64 = state.as_ref().map_or(0, |s| {
                    s.spilled_runs
                        .iter()
                        .filter_map(|r| r.as_ref())
                        .map(|r| r.byte_size)
                        .sum::<u64>()
                });
                base + run_bytes
            }
            Self::GroupBy { state, .. } => {
                let base = sum_spill!(state);
                let run_bytes: u64 = state.as_ref().map_or(0, |s| {
                    s.spilled_runs
                        .iter()
                        .filter_map(|r| r.as_ref())
                        .map(|r| r.byte_size)
                        .sum::<u64>()
                });
                base + run_bytes
            }
            Self::WindowFunction { state, .. } => {
                let base = sum_spill!(state);
                let run_bytes: u64 = state.as_ref().map_or(0, |s| {
                    s.spilled_runs
                        .iter()
                        .filter_map(|r| r.as_ref())
                        .map(|r| r.byte_size)
                        .sum::<u64>()
                });
                base + run_bytes
            }
            Self::Window { state, .. } => {
                let base = sum_spill!(state);
                let run_bytes: u64 = state.as_ref().map_or(0, |s| {
                    s.spilled_runs
                        .iter()
                        .filter_map(|r| r.as_ref())
                        .map(|r| r.byte_size)
                        .sum::<u64>()
                });
                base + run_bytes
            }
            Self::TopN { .. } => 0,
            Self::Distinct { state, .. } => {
                let base = sum_spill!(state);
                let run_bytes: u64 = state.as_ref().map_or(0, |s| {
                    s.spilled_runs
                        .iter()
                        .filter_map(|r| r.as_ref())
                        .map(|r| r.byte_size)
                        .sum::<u64>()
                });
                base + run_bytes
            }
            Self::Materialize { state, .. } => sum_spill!(state),
            Self::DataCollect { state, .. } => sum_spill!(state),
            Self::RollUpApply { state, .. } => sum_spill!(state),
            Self::PartialAggregate { state, .. } => sum_spill!(state),
            Self::FinalAggregate { state, .. } => sum_spill!(state),
        }
    }
}

#[cfg(test)]
#[path = "blocking/test.rs"]
mod tests;
