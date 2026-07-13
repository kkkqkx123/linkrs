use std::collections::{BTreeMap, HashMap};

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
use crate::query::executor::streaming::operator_base::OperatorBase;

// ——— state structs ———

#[derive(Debug)]
pub struct SortState {
    pub all_rows: Vec<Vec<Value>>,
    pub row_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub col_names: Vec<String>,
}

#[derive(Debug)]
pub struct AggregateState {
    pub all_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
}

#[derive(Debug)]
pub struct GroupByState {
    pub all_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
}

#[derive(Debug)]
pub struct WindowFunctionState {
    pub all_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
}

#[derive(Debug)]
pub struct WindowState {
    pub all_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
}

#[derive(Debug)]
pub struct TopNState {
    pub all_rows: Vec<Vec<Value>>,
    pub col_names: Vec<String>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
}

#[derive(Debug)]
pub struct DistinctState {
    pub seen_rows: std::collections::HashSet<Vec<Value>>,
    pub col_names: Vec<String>,
}

#[derive(Debug)]
pub struct MaterializeState {
    pub materialized_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub materialized: bool,
}

#[derive(Debug)]
pub struct DataCollectState {
    pub all_rows: Vec<Vec<Value>>,
    pub emitted: bool,
}

#[derive(Debug)]
pub struct RollUpApplyState {
    pub all_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
}

/// State for the local partial aggregate operator.
/// Per-partition rows are grouped and accumulator states are emitted.
#[derive(Debug)]
pub struct PartialAggregateState {
    /// group key → per-function accumulators
    pub group_map: HashMap<Vec<Value>, Vec<AggregateAccumulator>>,
    pub col_names: Vec<String>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
}

/// State for the global final aggregate operator.
/// Partial accumulator values are merged and final results produced.
#[derive(Debug)]
pub struct FinalAggregateState {
    /// group key → per-function merged accumulators
    pub group_map: HashMap<Vec<Value>, Vec<AggregateAccumulator>>,
    pub col_names: Vec<String>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
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
    /// Local partial aggregate that runs per partition.
    /// Produces group key + encoded accumulator state rows.
    PartialAggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<AggregateFunction>,
        output_col_names: Vec<String>,
        memory_tracker: MemoryTracker,
        state: Option<PartialAggregateState>,
    },
    /// Global final aggregate that merges partial results.
    /// Reads encoded accumulator rows and produces final values.
    FinalAggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<AggregateFunction>,
        output_col_names: Vec<String>,
        memory_tracker: MemoryTracker,
        state: Option<FinalAggregateState>,
    },
}

impl BlockingOperator {
    /// Create a BlockingOperator with fresh mutable state from an immutable spec.
    pub fn from_spec(
        spec: &super::super::operator_spec::BlockingSpec,
        memory_budget: &crate::query::executor::base::MemoryBudget,
    ) -> Self {
        match spec {
            super::super::operator_spec::BlockingSpec::Sort {
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
            super::super::operator_spec::BlockingSpec::Aggregate {
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
            super::super::operator_spec::BlockingSpec::GroupBy {
                group_by_expressions,
            } => Self::GroupBy {
                group_by_expressions: group_by_expressions.clone(),
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::super::operator_spec::BlockingSpec::WindowFunction {
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
            super::super::operator_spec::BlockingSpec::Window {
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
            super::super::operator_spec::BlockingSpec::TopN {
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
            super::super::operator_spec::BlockingSpec::Distinct => Self::Distinct {
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::super::operator_spec::BlockingSpec::Materialize => Self::Materialize {
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::super::operator_spec::BlockingSpec::DataCollect => Self::DataCollect {
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::super::operator_spec::BlockingSpec::RollUpApply {
                rollup_expressions,
            } => Self::RollUpApply {
                rollup_expressions: rollup_expressions.clone(),
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                state: None,
            },
            super::super::operator_spec::BlockingSpec::PartialAggregate {
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
            super::super::operator_spec::BlockingSpec::FinalAggregate {
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
                    all_rows: vec![],
                    row_iter: None,
                    col_names: vec![],
                });
            }
            Self::Aggregate { state, .. } => {
                *state = Some(AggregateState {
                    all_rows: vec![],
                    result_iter: None,
                });
            }
            Self::GroupBy { state, .. } => {
                *state = Some(GroupByState {
                    all_rows: vec![],
                    result_iter: None,
                });
            }
            Self::WindowFunction { state, .. } => {
                *state = Some(WindowFunctionState {
                    all_rows: vec![],
                    result_iter: None,
                });
            }
            Self::Window { state, .. } => {
                *state = Some(WindowState {
                    all_rows: vec![],
                    result_iter: None,
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
                });
            }
            Self::Materialize { state, .. } => {
                *state = Some(MaterializeState {
                    materialized_rows: vec![],
                    result_iter: None,
                    materialized: false,
                });
            }
            Self::DataCollect { state, .. } => {
                *state = Some(DataCollectState {
                    all_rows: vec![],
                    emitted: false,
                });
            }
            Self::RollUpApply { state, .. } => {
                *state = Some(RollUpApplyState {
                    all_rows: vec![],
                    result_iter: None,
                });
            }
            Self::PartialAggregate { state, .. } => {
                *state = Some(PartialAggregateState {
                    group_map: HashMap::new(),
                    col_names: vec![],
                    result_iter: None,
                });
            }
            Self::FinalAggregate { state, .. } => {
                *state = Some(FinalAggregateState {
                    group_map: HashMap::new(),
                    col_names: vec![],
                    result_iter: None,
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
                let state = state.as_mut().unwrap();
                if state.row_iter.is_none() {
                    while let Some(chunk) = input.advance()? {
                        base.ensure_not_cancelled()?;
                        if state.col_names.is_empty() {
                            state.col_names = chunk.col_names();
                        }
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        state.all_rows.extend(chunk.rows);
                    }

                    if !sort_expressions.is_empty() {
                        state.all_rows.sort_by(|a, b| {
                            for (idx, expr) in sort_expressions.iter().enumerate() {
                                let direction = sort_directions
                                    .get(idx)
                                    .copied()
                                    .unwrap_or(SortDirection::Ascending);

                                let mut ctx_a =
                                    ValueRowContext::new(a.clone(), state.col_names.clone());
                                let mut ctx_b =
                                    ValueRowContext::new(b.clone(), state.col_names.clone());

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

                    let all_rows_copy = std::mem::take(&mut state.all_rows);
                    state.row_iter = Some(all_rows_copy.into_iter());
                }

                if let Some(iter) = &mut state.row_iter {
                    let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                    if chunk_rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::from_rows_with_col_names(
                            chunk_rows,
                            Some(state.col_names.clone()),
                        )))
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
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        state.all_rows.extend(chunk.rows);
                    }

                    let mut group_map: HashMap<Vec<Value>, Vec<Vec<Value>>> = HashMap::new();

                    for row in state.all_rows.iter().cloned() {
                        let mut group_key = Vec::new();
                        if group_by_expressions.is_empty() {
                            group_key.push(Value::Null(NullType::Null));
                        } else {
                            for expr in group_by_expressions.iter() {
                                let mut context =
                                    ValueRowContext::new(row.clone(), col_names.clone());
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
                                let mut context =
                                    ValueRowContext::new(first_row.clone(), col_names.clone());
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
                        Ok(Some(DataChunk::from_rows_with_col_names(
                            chunk_rows,
                            Some(output_col_names.clone()),
                        )))
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
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        state.all_rows.extend(chunk.rows);
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
                            let mut context = ValueRowContext::new(row.clone(), col_names.clone());
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
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        state.all_rows.extend(chunk.rows);
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
                                        ValueRowContext::new(row.clone(), col_names.clone());
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
                                        let mut ctx_a =
                                            ValueRowContext::new(a.1.clone(), col_names.clone());
                                        let mut ctx_b =
                                            ValueRowContext::new(b.1.clone(), col_names.clone());
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
                                                let mut ctx = ValueRowContext::new(
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
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        state.all_rows.extend(chunk.rows);
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
                                        ValueRowContext::new(row.clone(), col_names.clone());
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
                                        let mut ctx_a =
                                            ValueRowContext::new(a.1.clone(), col_names.clone());
                                        let mut ctx_b =
                                            ValueRowContext::new(b.1.clone(), col_names.clone());
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
                                                let mut ctx = ValueRowContext::new(
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
                        Ok(Some(DataChunk::from_rows_with_col_names(
                            vec![row],
                            Some(state.col_names.clone()),
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
                                return Ok(Some(DataChunk::from_rows_with_col_names(
                                    result_rows,
                                    Some(state.col_names.clone()),
                                )));
                            }
                        }
                    }
                }

                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows_with_col_names(
                        result_rows,
                        Some(state.col_names.clone()),
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
                    while let Some(chunk) = input.advance()? {
                        base.ensure_not_cancelled()?;
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        state.materialized_rows.extend(chunk.rows);
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
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    state.all_rows.extend(chunk.rows);
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
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        for row in chunk.rows {
                            let mut ctx = ValueRowContext::new(row.clone(), col_names.clone());
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
                                        ValueRowContext::new(row.clone(), col_names.clone());
                                    match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                        Ok(value) => group_key.push(value),
                                        Err(_) => group_key.push(Value::Null(NullType::Null)),
                                    }
                                }
                            }

                            let group_accs = state.group_map.entry(group_key).or_insert_with(|| {
                                aggregate_functions
                                    .iter()
                                    .filter_map(|f| AggregateAccumulator::for_function(f))
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
                        Ok(Some(DataChunk::from_rows_with_col_names(
                            chunk_rows,
                            Some(output_col_names.clone()),
                        )))
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
                                        .filter_map(|f| AggregateAccumulator::for_function(f))
                                        .collect()
                                });

                            for i in 0..num_agg_funcs {
                                if let Some(acc) = group_accs.get_mut(i) {
                                    let acc_col_idx = num_group_keys + i;
                                    let partial_value = row.get(acc_col_idx);
                                    if let Some(val) = partial_value {
                                        let partial_acc =
                                            value_to_partial_accumulator(&aggregate_functions[i], val);
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
                        Ok(Some(DataChunk::from_rows_with_col_names(
                            chunk_rows,
                            Some(output_col_names.clone()),
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
}

fn compute_window_function(
    name: &str,
    args: &[Value],
    partition_rows: &[(usize, Vec<Value>)],
    current_pos: usize,
) -> Value {
    let value_to_i64 = |val: &Value| -> i64 {
        match val {
            Value::Int(i) => *i as i64,
            Value::BigInt(i) => *i,
            _ => 1,
        }
    };

    match name {
        "row_number" => Value::BigInt((current_pos + 1) as i64),
        "rank" => Value::BigInt((current_pos + 1) as i64),
        "dense_rank" => Value::BigInt((current_pos + 1) as i64),
        "lead" => {
            let offset = if !args.is_empty() {
                value_to_i64(&args[0]) as usize
            } else {
                1
            };
            let default_val = if args.len() > 1 {
                args[1].clone()
            } else {
                Value::Null(NullType::Null)
            };
            if current_pos + offset < partition_rows.len() {
                partition_rows[current_pos + offset]
                    .1
                    .first()
                    .cloned()
                    .unwrap_or(default_val)
            } else {
                default_val
            }
        }
        "lag" => {
            let offset = if !args.is_empty() {
                value_to_i64(&args[0]) as usize
            } else {
                1
            };
            let default_val = if args.len() > 1 {
                args[1].clone()
            } else {
                Value::Null(NullType::Null)
            };
            if current_pos >= offset {
                partition_rows[current_pos - offset]
                    .1
                    .first()
                    .cloned()
                    .unwrap_or(default_val)
            } else {
                default_val
            }
        }
        "first_value" => partition_rows
            .first()
            .and_then(|(_, r)| r.first().cloned())
            .unwrap_or(Value::Null(NullType::Null)),
        "last_value" => partition_rows
            .last()
            .and_then(|(_, r)| r.first().cloned())
            .unwrap_or(Value::Null(NullType::Null)),
        "nth_value" => {
            let n = if !args.is_empty() {
                value_to_i64(&args[0]) as usize
            } else {
                1
            };
            if n > 0 && n <= partition_rows.len() {
                partition_rows[n - 1]
                    .1
                    .first()
                    .cloned()
                    .unwrap_or(Value::Null(NullType::Null))
            } else {
                Value::Null(NullType::Null)
            }
        }
        "ntile" => {
            let n = if !args.is_empty() {
                value_to_i64(&args[0])
            } else {
                1
            };
            if n > 0 {
                let bucket_size = (partition_rows.len() as i64 + n - 1) / n;
                Value::BigInt((current_pos as i64 / bucket_size) + 1)
            } else {
                Value::Null(NullType::Null)
            }
        }
        _ => Value::Null(NullType::Null),
    }
}

fn compare_rows_for_topn(
    a: &[Value],
    b: &[Value],
    col_names: &[String],
    sort_expressions: &[Expression],
    sort_directions: &[SortDirection],
) -> std::cmp::Ordering {
    for (idx, expr) in sort_expressions.iter().enumerate() {
        let direction = sort_directions
            .get(idx)
            .copied()
            .unwrap_or(SortDirection::Ascending);

        let mut ctx_a = ValueRowContext::new(a.to_vec(), col_names.to_vec());
        let mut ctx_b = ValueRowContext::new(b.to_vec(), col_names.to_vec());

        let val_a =
            ExpressionEvaluator::evaluate(expr, &mut ctx_a).unwrap_or(Value::Null(NullType::Null));
        let val_b =
            ExpressionEvaluator::evaluate(expr, &mut ctx_b).unwrap_or(Value::Null(NullType::Null));

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
}

/// Extract the field value from a row for a given aggregate function.
fn extract_field_value(row: &[Value], col_names: &[String], func: &AggregateFunction) -> Value {
    match func {
        AggregateFunction::Count(None) => Value::Int(1),
        AggregateFunction::Count(Some(field))
        | AggregateFunction::Sum(field)
        | AggregateFunction::Avg(field)
        | AggregateFunction::Min(field)
        | AggregateFunction::Max(field) => {
            if let Some(idx) = col_names.iter().position(|c| c == field) {
                row.get(idx).cloned().unwrap_or(Value::Null(NullType::Null))
            } else {
                Value::Null(NullType::Null)
            }
        }
        _ => Value::Null(NullType::Null),
    }
}

/// Decode a Value back into a single-use AggregateAccumulator for merging.
/// The value was produced by `accumulator_to_value`.
fn value_to_partial_accumulator(func: &AggregateFunction, value: &Value) -> Option<AggregateAccumulator> {
    match func {
        AggregateFunction::Count(_) => {
            if let Value::BigInt(n) = value {
                Some(AggregateAccumulator::Count(*n as u64))
            } else {
                Some(AggregateAccumulator::Count(0))
            }
        }
        AggregateFunction::Sum(_) => {
            match value {
                Value::Double(n) => Some(AggregateAccumulator::Sum(*n)),
                Value::BigInt(n) => Some(AggregateAccumulator::Sum(*n as f64)),
                Value::Int(n) => Some(AggregateAccumulator::Sum(*n as f64)),
                _ => Some(AggregateAccumulator::Sum(0.0)),
            }
        }
        AggregateFunction::Min(_) => {
            if matches!(value, Value::Null(_)) {
                Some(AggregateAccumulator::Min(None))
            } else {
                Some(AggregateAccumulator::Min(Some(value.clone())))
            }
        }
        AggregateFunction::Max(_) => {
            if matches!(value, Value::Null(_)) {
                Some(AggregateAccumulator::Max(None))
            } else {
                Some(AggregateAccumulator::Max(Some(value.clone())))
            }
        }
        AggregateFunction::Avg(_) => {
            if let Value::List(list) = value {
                let sum = list.values.first().and_then(|v| match v {
                    Value::Double(n) => Some(*n),
                    Value::BigInt(n) => Some(*n as f64),
                    _ => None,
                }).unwrap_or(0.0);
                let count = list.values.get(1).and_then(|v| match v {
                    Value::BigInt(n) => Some(*n as u64),
                    _ => None,
                }).unwrap_or(0);
                Some(AggregateAccumulator::Avg { sum, count })
            } else {
                Some(AggregateAccumulator::Avg { sum: 0.0, count: 0 })
            }
        }
        _ => None,
    }
}
