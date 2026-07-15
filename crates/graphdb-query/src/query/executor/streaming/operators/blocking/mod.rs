use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::base::MemoryTracker;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::SortDirection;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::executor::ValueRowContext;
use crate::query::executor::streaming::helpers::accumulator_states::{
    accumulator_to_value, AggregateAccumulator,
};
use crate::query::executor::streaming::helpers::compare_values;
use crate::query::executor::streaming::helpers::compute_aggregate;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::executor::streaming::spill::{SpillManager, SpillReader, SpilledFile};

pub mod aggregate;
pub mod materialize;
pub mod sort;
pub mod window;

pub use aggregate::{AggregateState, FinalAggregateState, GroupByState, PartialAggregateState};
pub use materialize::{DataCollectState, DistinctState, MaterializeState, RollUpApplyState};
pub use sort::{MergeState, RunBuffer, SortState, TopNState};
pub use window::{WindowFunctionState, WindowState};

use aggregate::{extract_field_value, value_to_partial_accumulator};
use sort::{compare_rows_for_topn, find_min_run, refill_run_buffer, sort_rows, spill_sorted_run};
use window::compute_window_function;

fn spill_buffer(
    buffer: &mut Vec<Vec<Value>>,
    sm: &SpillManager,
    spill_files: &mut Vec<SpilledFile>,
    tracker: &mut MemoryTracker,
) -> Result<(), QueryError> {
    if buffer.is_empty() {
        return Ok(());
    }
    let mut writer = sm.create_writer()?;
    writer.write_rows(buffer)?;
    let file = writer.finalize()?;
    buffer.clear();
    tracker.reset();
    spill_files.push(file);
    Ok(())
}

fn drain_spill_files(
    spill_files: &mut Vec<SpilledFile>,
    _sm: &SpillManager,
) -> Result<Vec<Vec<Value>>, QueryError> {
    let mut all = Vec::new();
    for sf in spill_files.drain(..) {
        let mut reader = SpillReader::open(&sf)?;
        while let Some(row) = reader.read_row()? {
            all.push(row);
        }
    }
    Ok(all)
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
                    all_rows: vec![],
                    row_iter: None,
                    spill_files: vec![],
                    runs: vec![],
                    has_spilled: false,
                    merge_state: None,
                });
            }
            Self::Aggregate { state, .. } => {
                *state = Some(AggregateState {
                    all_rows: vec![],
                    result_iter: None,
                    spill_files: vec![],
                });
            }
            Self::GroupBy { state, .. } => {
                *state = Some(GroupByState {
                    all_rows: vec![],
                    result_iter: None,
                    spill_files: vec![],
                });
            }
            Self::WindowFunction { state, .. } => {
                *state = Some(WindowFunctionState {
                    all_rows: vec![],
                    result_iter: None,
                    spill_files: vec![],
                });
            }
            Self::Window { state, .. } => {
                *state = Some(WindowState {
                    all_rows: vec![],
                    result_iter: None,
                    spill_files: vec![],
                });
            }
            Self::TopN { state, .. } => {
                *state = Some(TopNState {
                    all_rows: vec![],
                    col_names: vec![],
                    result_iter: None,
                });
            }
            Self::Distinct { state, .. } => {
                *state = Some(DistinctState {
                    seen_rows: std::collections::HashSet::new(),
                    col_names: Vec::new(),
                    spill_files: vec![],
                });
            }
            Self::Materialize { state, .. } => {
                *state = Some(MaterializeState {
                    materialized_rows: vec![],
                    result_iter: None,
                    materialized: false,
                    spill_files: vec![],
                });
            }
            Self::DataCollect { state, .. } => {
                *state = Some(DataCollectState {
                    all_rows: vec![],
                    emitted: false,
                    spill_files: vec![],
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
                    while let Some(chunk) = input.advance()? {
                        base.ensure_not_cancelled()?;
                        if st.col_names.is_empty() {
                            st.col_names = chunk.col_names();
                        }
                        for row in chunk.rows {
                            if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                if let Some(sm) = base.spill_manager() {
                                    spill_sorted_run(
                                        &mut st.all_rows,
                                        &st.col_names,
                                        sort_expressions,
                                        sort_directions,
                                        &sm,
                                        memory_tracker,
                                        &mut st.runs,
                                    )?;
                                    st.has_spilled = true;
                                    memory_tracker.try_reserve_row(&row)?;
                                } else {
                                    return Err(e);
                                }
                            }
                            st.all_rows.push(row);
                        }
                    }

                    if !st.spill_files.is_empty() {
                        if let Some(sm) = base.spill_manager() {
                            let spilled = drain_spill_files(&mut st.spill_files, &sm)?;
                            st.all_rows.extend(spilled);
                        }
                    }

                    if st.has_spilled {
                        if !st.all_rows.is_empty() {
                            if let Some(sm) = base.spill_manager() {
                                spill_sorted_run(
                                    &mut st.all_rows,
                                    &st.col_names,
                                    sort_expressions,
                                    sort_directions,
                                    &sm,
                                    memory_tracker,
                                    &mut st.runs,
                                )?;
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
                        if !sort_expressions.is_empty() {
                            sort_rows(
                                &mut st.all_rows,
                                &st.col_names,
                                sort_expressions,
                                sort_directions,
                            );
                        }
                        let taken = std::mem::take(&mut st.all_rows);
                        st.row_iter = Some(taken.into_iter());
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
                        let result_layout = Arc::new(SlotLayout::from_names(&merge.col_names));
                        Ok(Some(DataChunk::new_with_layout(out_rows, result_layout)))
                    }
                } else if let Some(ref mut iter) = st.row_iter {
                    let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                    if chunk_rows.is_empty() {
                        Ok(None)
                    } else {
                        let result_layout = Arc::new(SlotLayout::from_names(&st.col_names));
                        Ok(Some(DataChunk::new_with_layout(chunk_rows, result_layout)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::Aggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names,
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
                        for row in chunk.rows {
                            if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                if let Some(sm) = base.spill_manager() {
                                    spill_buffer(
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
                        if let Some(sm) = base.spill_manager() {
                            let spilled = drain_spill_files(&mut state.spill_files, &sm)?;
                            let mut combined = spilled;
                            combined.append(&mut state.all_rows);
                            state.all_rows = combined;
                        }
                    }

                    let mut group_map: HashMap<Vec<Value>, Vec<Vec<Value>>> = HashMap::new();

                    for row in state.all_rows.iter().cloned() {
                        let mut group_key = Vec::new();
                        if group_by_expressions.is_empty() {
                            group_key.push(Value::Null(NullType::Null));
                        } else {
                            for expr in group_by_expressions.iter() {
                                let mut context =
                                    ValueRowContext::from_names(row.clone(), col_names.clone());
                                match ExpressionEvaluator::evaluate(expr, &mut context) {
                                    Ok(value) => group_key.push(value),
                                    Err(_) => group_key.push(Value::Null(NullType::Null)),
                                }
                            }
                        }

                        group_map.entry(group_key).or_default().push(row);
                    }

                    let mut result_rows = Vec::new();
                    for (_group_key, group_rows) in group_map {
                        let mut result_row = Vec::new();

                        for expr in group_by_expressions.iter() {
                            if let Some(first_row) = group_rows.first() {
                                let mut context = ValueRowContext::from_names(
                                    first_row.clone(),
                                    col_names.clone(),
                                );
                                match ExpressionEvaluator::evaluate(expr, &mut context) {
                                    Ok(value) => result_row.push(value),
                                    Err(_) => result_row.push(Value::Null(NullType::Null)),
                                }
                            }
                        }

                        for (agg_func, _expr) in aggregate_functions.iter() {
                            let agg_value = compute_aggregate(agg_func, &group_rows, &col_names);
                            result_row.push(agg_value);
                        }

                        result_rows.push(result_row);
                    }

                    state.result_iter = Some(result_rows.into_iter());
                }

                if let Some(iter) = &mut state.result_iter {
                    let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                    if chunk_rows.is_empty() {
                        Ok(None)
                    } else {
                        let result_layout = Arc::new(SlotLayout::from_names(output_col_names));
                        Ok(Some(DataChunk::new_with_layout(chunk_rows, result_layout)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::GroupBy {
                group_by_expressions,
                memory_tracker,
                state,
                ..
            } => {
                let state = state.as_mut().unwrap();
                if state.result_iter.is_none() {
                    while let Some(chunk) = input.advance()? {
                        base.ensure_not_cancelled()?;
                        for row in chunk.rows {
                            if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                if let Some(sm) = base.spill_manager() {
                                    spill_buffer(
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
                        if let Some(sm) = base.spill_manager() {
                            let spilled = drain_spill_files(&mut state.spill_files, &sm)?;
                            let mut combined = spilled;
                            combined.append(&mut state.all_rows);
                            state.all_rows = combined;
                        }
                    }

                    if state.all_rows.is_empty() {
                        return Ok(None);
                    }

                    let col_names = (0..state.all_rows[0].len())
                        .map(|i| format!("col_{}", i))
                        .collect::<Vec<_>>();

                    let mut groups: HashMap<String, Vec<Vec<Value>>> = HashMap::new();
                    for row in state.all_rows.iter() {
                        let mut key_parts: Vec<String> = Vec::new();
                        for expr in group_by_expressions.iter() {
                            let mut context =
                                ValueRowContext::from_names(row.clone(), col_names.clone());
                            let key_val = ExpressionEvaluator::evaluate(expr, &mut context)
                                .unwrap_or(Value::Null(NullType::Null));
                            key_parts.push(format!("{:?}", key_val));
                        }
                        let key = key_parts.join("|");
                        groups.entry(key).or_default().push(row.clone());
                    }

                    let result_rows: Vec<Vec<Value>> = groups.into_values().flatten().collect();
                    state.result_iter = Some(result_rows.into_iter());
                }

                if let Some(iter) = &mut state.result_iter {
                    let result_rows: Vec<Vec<Value>> = iter.take(1024).collect();
                    if result_rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::from_rows(result_rows)))
                    }
                } else {
                    Ok(None)
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
                let state = state.as_mut().unwrap();
                if state.result_iter.is_none() {
                    let mut col_names: Vec<String> = vec![];
                    while let Some(chunk) = input.advance()? {
                        base.ensure_not_cancelled()?;
                        if col_names.is_empty() {
                            col_names = chunk.col_names();
                        }
                        for row in chunk.rows {
                            if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                if let Some(sm) = base.spill_manager() {
                                    spill_buffer(
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
                        if let Some(sm) = base.spill_manager() {
                            let spilled = drain_spill_files(&mut state.spill_files, &sm)?;
                            let mut combined = spilled;
                            combined.append(&mut state.all_rows);
                            state.all_rows = combined;
                        }
                    }

                    if state.all_rows.is_empty() {
                        state.result_iter = Some(vec![].into_iter());
                    } else {
                        let mut partitions: BTreeMap<Vec<Value>, Vec<(usize, Vec<Value>)>> =
                            BTreeMap::new();

                        for (idx, row) in state.all_rows.iter().enumerate() {
                            let mut partition_key = Vec::new();
                            if partition_by_exprs.is_empty() {
                                partition_key.push(Value::Null(NullType::Null));
                            } else {
                                for expr in partition_by_exprs.iter() {
                                    let mut ctx =
                                        ValueRowContext::from_names(row.clone(), col_names.clone());
                                    match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                        Ok(val) => partition_key.push(val),
                                        Err(_) => partition_key.push(Value::Null(NullType::Null)),
                                    }
                                }
                            }
                            partitions
                                .entry(partition_key)
                                .or_default()
                                .push((idx, row.clone()));
                        }

                        let mut result_rows = Vec::new();
                        for (_key, mut partition_rows) in partitions {
                            if !order_by_exprs.is_empty() {
                                partition_rows.sort_by(|a, b| {
                                    for (idx, expr) in order_by_exprs.iter().enumerate() {
                                        let direction = order_by_directions
                                            .get(idx)
                                            .copied()
                                            .unwrap_or(SortDirection::Ascending);
                                        let mut ctx_a = ValueRowContext::from_names(
                                            a.1.clone(),
                                            col_names.clone(),
                                        );
                                        let mut ctx_b = ValueRowContext::from_names(
                                            b.1.clone(),
                                            col_names.clone(),
                                        );
                                        let val_a = ExpressionEvaluator::evaluate(expr, &mut ctx_a)
                                            .unwrap_or(Value::Null(NullType::Null));
                                        let val_b = ExpressionEvaluator::evaluate(expr, &mut ctx_b)
                                            .unwrap_or(Value::Null(NullType::Null));
                                        let cmp = compare_values(&val_a, &val_b);
                                        let final_cmp = match direction {
                                            SortDirection::Ascending => cmp,
                                            SortDirection::Descending => cmp.reverse(),
                                        };
                                        if final_cmp != std::cmp::Ordering::Equal {
                                            return final_cmp;
                                        }
                                    }
                                    std::cmp::Ordering::Equal
                                });
                            }

                            for (row_idx, row) in partition_rows.iter() {
                                let mut result_row = row.clone();
                                for window_expr in window_exprs.iter() {
                                    if let Expression::WindowFunction { name, args, .. } =
                                        window_expr
                                    {
                                        let func_args: Vec<Value> = args
                                            .iter()
                                            .map(|arg| {
                                                let mut ctx = ValueRowContext::from_names(
                                                    row.clone(),
                                                    col_names.clone(),
                                                );
                                                ExpressionEvaluator::evaluate(arg, &mut ctx)
                                                    .unwrap_or(Value::Null(NullType::Null))
                                            })
                                            .collect();

                                        let window_result = compute_window_function(
                                            name,
                                            &func_args,
                                            &partition_rows,
                                            partition_rows
                                                .iter()
                                                .position(|(i, _)| i == row_idx)
                                                .unwrap_or(0),
                                        );
                                        result_row.push(window_result);
                                    }
                                }
                                result_rows.push(result_row);
                            }
                        }

                        state.result_iter = Some(result_rows.into_iter());
                    }
                }

                if let Some(iter) = &mut state.result_iter {
                    let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                    if chunk_rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::from_rows(chunk_rows)))
                    }
                } else {
                    Ok(None)
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
                let state = state.as_mut().unwrap();
                if state.result_iter.is_none() {
                    let mut col_names: Vec<String> = vec![];
                    while let Some(chunk) = input.advance()? {
                        base.ensure_not_cancelled()?;
                        if col_names.is_empty() {
                            col_names = chunk.col_names();
                        }
                        for row in chunk.rows {
                            if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                if let Some(sm) = base.spill_manager() {
                                    spill_buffer(
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
                        if let Some(sm) = base.spill_manager() {
                            let spilled = drain_spill_files(&mut state.spill_files, &sm)?;
                            let mut combined = spilled;
                            combined.append(&mut state.all_rows);
                            state.all_rows = combined;
                        }
                    }

                    if state.all_rows.is_empty() {
                        state.result_iter = Some(vec![].into_iter());
                    } else {
                        let mut partitions: BTreeMap<Vec<Value>, Vec<(usize, Vec<Value>)>> =
                            BTreeMap::new();

                        for (idx, row) in state.all_rows.iter().enumerate() {
                            let mut partition_key = Vec::new();
                            if partition_by_exprs.is_empty() {
                                partition_key.push(Value::Null(NullType::Null));
                            } else {
                                for expr in partition_by_exprs.iter() {
                                    let mut ctx =
                                        ValueRowContext::from_names(row.clone(), col_names.clone());
                                    match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                        Ok(val) => partition_key.push(val),
                                        Err(_) => partition_key.push(Value::Null(NullType::Null)),
                                    }
                                }
                            }
                            partitions
                                .entry(partition_key)
                                .or_default()
                                .push((idx, row.clone()));
                        }

                        let mut result_rows = Vec::new();
                        for (_key, mut partition_rows) in partitions {
                            if !order_by_exprs.is_empty() {
                                partition_rows.sort_by(|a, b| {
                                    for (idx, expr) in order_by_exprs.iter().enumerate() {
                                        let direction = order_by_directions
                                            .get(idx)
                                            .copied()
                                            .unwrap_or(SortDirection::Ascending);
                                        let mut ctx_a = ValueRowContext::from_names(
                                            a.1.clone(),
                                            col_names.clone(),
                                        );
                                        let mut ctx_b = ValueRowContext::from_names(
                                            b.1.clone(),
                                            col_names.clone(),
                                        );
                                        let val_a = ExpressionEvaluator::evaluate(expr, &mut ctx_a)
                                            .unwrap_or(Value::Null(NullType::Null));
                                        let val_b = ExpressionEvaluator::evaluate(expr, &mut ctx_b)
                                            .unwrap_or(Value::Null(NullType::Null));
                                        let cmp = compare_values(&val_a, &val_b);
                                        let final_cmp = match direction {
                                            SortDirection::Ascending => cmp,
                                            SortDirection::Descending => cmp.reverse(),
                                        };
                                        if final_cmp != std::cmp::Ordering::Equal {
                                            return final_cmp;
                                        }
                                    }
                                    std::cmp::Ordering::Equal
                                });
                            }

                            for (row_idx, row) in partition_rows.iter() {
                                let mut result_row = row.clone();
                                for window_expr in window_exprs.iter() {
                                    if let Expression::WindowFunction { name, args, .. } =
                                        window_expr
                                    {
                                        let func_args: Vec<Value> = args
                                            .iter()
                                            .map(|arg| {
                                                let mut ctx = ValueRowContext::from_names(
                                                    row.clone(),
                                                    col_names.clone(),
                                                );
                                                ExpressionEvaluator::evaluate(arg, &mut ctx)
                                                    .unwrap_or(Value::Null(NullType::Null))
                                            })
                                            .collect();

                                        let window_result = compute_window_function(
                                            name,
                                            &func_args,
                                            &partition_rows,
                                            partition_rows
                                                .iter()
                                                .position(|(i, _)| i == row_idx)
                                                .unwrap_or(0),
                                        );
                                        result_row.push(window_result);
                                    }
                                }
                                result_rows.push(result_row);
                            }
                        }

                        state.result_iter = Some(result_rows.into_iter());
                    }
                }

                if let Some(iter) = &mut state.result_iter {
                    let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                    if chunk_rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::from_rows(chunk_rows)))
                    }
                } else {
                    Ok(None)
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

                    while let Some(chunk) = input.advance()? {
                        base.ensure_not_cancelled()?;
                        if state.col_names.is_empty() {
                            state.col_names = chunk.col_names();
                        }
                        for row in chunk.rows {
                            memory_tracker.try_reserve_row(&row)?;
                            if state.all_rows.len() < limit {
                                state.all_rows.push(row);
                            } else {
                                if state.all_rows.len() == limit {
                                    state.all_rows.sort_by(|a, b| {
                                        compare_rows_for_topn(
                                            a,
                                            b,
                                            &state.col_names,
                                            sort_expressions,
                                            sort_directions,
                                        )
                                    });
                                }
                                let cmp_last = compare_rows_for_topn(
                                    &row,
                                    state.all_rows.last().unwrap(),
                                    &state.col_names,
                                    sort_expressions,
                                    sort_directions,
                                );
                                if cmp_last == std::cmp::Ordering::Less {
                                    state.all_rows.pop();
                                    let pos = state.all_rows.binary_search_by(|existing| {
                                        compare_rows_for_topn(
                                            existing,
                                            &row,
                                            &state.col_names,
                                            sort_expressions,
                                            sort_directions,
                                        )
                                    });
                                    let pos = match pos {
                                        Ok(p) | Err(p) => p,
                                    };
                                    state.all_rows.insert(pos, row);
                                }
                            }
                        }
                    }

                    if state.all_rows.len() > 1 {
                        state.all_rows.sort_by(|a, b| {
                            compare_rows_for_topn(
                                a,
                                b,
                                &state.col_names,
                                sort_expressions,
                                sort_directions,
                            )
                        });
                    }

                    state.all_rows.truncate(limit);
                    state.result_iter = Some(std::mem::take(&mut state.all_rows).into_iter());
                }

                if let Some(iter) = &mut state.result_iter {
                    if let Some(row) = iter.next() {
                        let result_layout = Arc::new(SlotLayout::from_names(&state.col_names));
                        Ok(Some(DataChunk::new_with_layout(vec![row], result_layout)))
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
                let mut result_rows = Vec::new();
                while let Some(chunk) = input.advance()? {
                    base.ensure_not_cancelled()?;
                    if state.col_names.is_empty() {
                        state.col_names = chunk.col_names();
                    }
                    for row in chunk.rows {
                        if !state.seen_rows.contains(&row) {
                            memory_tracker.try_reserve_row(&row)?;
                            state.seen_rows.insert(row.clone());
                            result_rows.push(row);
                            if result_rows.len() == 1024 {
                                let result_layout =
                                    Arc::new(SlotLayout::from_names(&state.col_names));
                                return Ok(Some(DataChunk::new_with_layout(
                                    result_rows,
                                    result_layout,
                                )));
                            }
                        }
                    }
                }

                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    let result_layout = Arc::new(SlotLayout::from_names(&state.col_names));
                    Ok(Some(DataChunk::new_with_layout(result_rows, result_layout)))
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
                    while let Some(chunk) = input.advance()? {
                        base.ensure_not_cancelled()?;
                        for row in chunk.rows {
                            if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                if let Some(sm) = base.spill_manager() {
                                    spill_buffer(
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
                        if let Some(sm) = base.spill_manager() {
                            let spilled = drain_spill_files(&mut state.spill_files, &sm)?;
                            let mut combined = spilled;
                            combined.append(&mut state.materialized_rows);
                            state.materialized_rows = combined;
                        }
                    }

                    state.materialized = true;
                    state.result_iter =
                        Some(std::mem::take(&mut state.materialized_rows).into_iter());
                }

                if let Some(iter) = &mut state.result_iter {
                    if let Some(row) = iter.next() {
                        Ok(Some(DataChunk::from_rows(vec![row])))
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

                while let Some(chunk) = input.advance()? {
                    base.ensure_not_cancelled()?;
                    for row in chunk.rows {
                        if let Err(e) = memory_tracker.try_reserve_row(&row) {
                            if let Some(sm) = base.spill_manager() {
                                spill_buffer(
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
                    if let Some(sm) = base.spill_manager() {
                        let spilled = drain_spill_files(&mut state.spill_files, &sm)?;
                        let mut combined = spilled;
                        combined.append(&mut state.all_rows);
                        state.all_rows = combined;
                    }
                }

                if !state.all_rows.is_empty() {
                    state.emitted = true;
                    let rows = std::mem::take(&mut state.all_rows);
                    return Ok(Some(DataChunk::from_rows(rows)));
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
                    while let Some(chunk) = input.advance()? {
                        base.ensure_not_cancelled()?;
                        if col_names.is_empty() {
                            col_names = chunk.col_names();
                        }
                        for row in chunk.rows {
                            if let Err(e) = memory_tracker.try_reserve_row(&row) {
                                if let Some(sm) = base.spill_manager() {
                                    spill_buffer(
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
                        if let Some(sm) = base.spill_manager() {
                            let spilled = drain_spill_files(&mut state.spill_files, &sm)?;
                            let mut combined = spilled;
                            combined.append(&mut state.all_rows);
                            state.all_rows = combined;
                        }
                    }

                    state.result_iter = Some(std::mem::take(&mut state.all_rows).into_iter());
                }

                if let Some(iter) = &mut state.result_iter {
                    if let Some(row) = iter.next() {
                        return Ok(Some(DataChunk::from_rows(vec![row])));
                    }
                }

                Ok(None)
            }

            Self::PartialAggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names,
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
                            let mut group_key = Vec::new();
                            if group_by_expressions.is_empty() {
                                group_key.push(Value::Null(NullType::Null));
                            } else {
                                for expr in group_by_expressions.iter() {
                                    let mut ctx =
                                        ValueRowContext::from_names(row.clone(), col_names.clone());
                                    match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                        Ok(value) => group_key.push(value),
                                        Err(_) => group_key.push(Value::Null(NullType::Null)),
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
                                    let value = extract_field_value(&row, &col_names, func);
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
                        let result_layout = Arc::new(SlotLayout::from_names(output_col_names));
                        Ok(Some(DataChunk::new_with_layout(chunk_rows, result_layout)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::FinalAggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names,
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
                        let result_layout = Arc::new(SlotLayout::from_names(output_col_names));
                        Ok(Some(DataChunk::new_with_layout(chunk_rows, result_layout)))
                    }
                } else {
                    Ok(None)
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
            Self::Sort { .. }
            | Self::Aggregate { .. }
            | Self::GroupBy { .. }
            | Self::WindowFunction { .. }
            | Self::Window { .. }
            | Self::TopN { .. }
            | Self::Distinct { .. }
            | Self::Materialize { .. }
            | Self::DataCollect { .. }
            | Self::RollUpApply { .. }
            | Self::PartialAggregate { .. }
            | Self::FinalAggregate { .. } => input.stop(),
        }
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
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
                    input.close()?;
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
                    *state = None;
                    input.close()?;
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
                    *state = None;
                    input.close()?;
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
                    *state = None;
                    input.close()?;
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
                    *state = None;
                    input.close()?;
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
                    input.close()?;
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
                    input.close()?;
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
                    *state = None;
                    input.close()?;
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
                    input.close()?;
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
                    input.close()?;
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
                    input.close()?;
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
                    input.close()?;
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
                    spill_buffer(&mut s.all_rows, sm, &mut s.spill_files, memory_tracker)?;
                }
                Ok(())
            }
            Self::Aggregate {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    spill_buffer(&mut s.all_rows, sm, &mut s.spill_files, memory_tracker)?;
                }
                Ok(())
            }
            Self::GroupBy {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    spill_buffer(&mut s.all_rows, sm, &mut s.spill_files, memory_tracker)?;
                }
                Ok(())
            }
            Self::WindowFunction {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    spill_buffer(&mut s.all_rows, sm, &mut s.spill_files, memory_tracker)?;
                }
                Ok(())
            }
            Self::Window {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    spill_buffer(&mut s.all_rows, sm, &mut s.spill_files, memory_tracker)?;
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
                        let rows: Vec<Vec<Value>> = s.seen_rows.iter().cloned().collect();
                        let mut writer = sm.create_writer()?;
                        writer.write_rows(&rows)?;
                        let file = writer.finalize()?;
                        s.seen_rows.clear();
                        memory_tracker.reset();
                        s.spill_files.push(file);
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
                    spill_buffer(
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
                    spill_buffer(&mut s.all_rows, sm, &mut s.spill_files, memory_tracker)?;
                }
                Ok(())
            }
            Self::PartialAggregate {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    if !s.group_map.is_empty() {
                        let mut writer = sm.create_writer()?;
                        for (key, accs) in &s.group_map {
                            let mut row = key.clone();
                            for acc in accs {
                                row.push(accumulator_to_value(acc));
                            }
                            writer.write_row(&row)?;
                        }
                        let file = writer.finalize()?;
                        s.group_map.clear();
                        memory_tracker.reset();
                        s.spill_files.push(file);
                    }
                }
                Ok(())
            }
            Self::RollUpApply {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    spill_buffer(&mut s.all_rows, sm, &mut s.spill_files, memory_tracker)?;
                }
                Ok(())
            }
            Self::FinalAggregate {
                state,
                memory_tracker,
                ..
            } => {
                if let Some(ref mut s) = state {
                    if !s.group_map.is_empty() {
                        let mut writer = sm.create_writer()?;
                        for (key, accs) in &s.group_map {
                            let mut row = key.clone();
                            for acc in accs {
                                row.push(accumulator_to_value(acc));
                            }
                            writer.write_row(&row)?;
                        }
                        let file = writer.finalize()?;
                        s.group_map.clear();
                        memory_tracker.reset();
                        s.spill_files.push(file);
                    }
                }
                Ok(())
            }
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
            Self::Aggregate { state, .. } => sum_spill!(state),
            Self::GroupBy { state, .. } => sum_spill!(state),
            Self::WindowFunction { state, .. } => sum_spill!(state),
            Self::Window { state, .. } => sum_spill!(state),
            Self::TopN { .. } => 0,
            Self::Distinct { state, .. } => sum_spill!(state),
            Self::Materialize { state, .. } => sum_spill!(state),
            Self::DataCollect { state, .. } => sum_spill!(state),
            Self::RollUpApply { state, .. } => sum_spill!(state),
            Self::PartialAggregate { state, .. } => sum_spill!(state),
            Self::FinalAggregate { state, .. } => sum_spill!(state),
        }
    }
}

#[cfg(test)]
#[path = "test.rs"]
mod tests;
