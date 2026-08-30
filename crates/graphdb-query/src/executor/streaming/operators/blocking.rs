use std::sync::Arc;

use crate::executor::base::MemoryTracker;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::error::QueryError;

pub mod aggregate;
pub mod aggregate_operator;
pub mod helpers;
pub mod materialize;
pub mod materialize_operator;
pub mod sort;
pub mod sort_operator;
pub mod window;
pub mod window_operator;

pub use aggregate::{AggregateState, FinalAggregateState, GroupByState, PartialAggregateState};
pub use materialize::{DataCollectState, DistinctState, MaterializeState, RollUpApplyState};
pub use sort::{MergeState, RunBuffer, SortState, TopNState};
pub use window::{WindowFunctionState, WindowState};

pub use crate::executor::streaming::chunk::ColumnarBatch;
pub use crate::executor::streaming::executor::SortDirection;
pub use graphdb_core::types::expr::Expression;
pub use graphdb_core::Value;

use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;

#[derive(Debug)]
pub enum BlockingOperatorKind {
    Sort {
        sort_expressions: Vec<graphdb_core::types::expr::Expression>,
        sort_directions: Vec<crate::executor::streaming::executor::SortDirection>,
        memory_tracker: MemoryTracker,
        state: Option<SortState>,
    },
    Aggregate {
        group_by_expressions: Vec<graphdb_core::types::expr::Expression>,
        aggregate_functions: Vec<(
            graphdb_core::types::operators::AggregateFunction,
            Vec<graphdb_core::types::expr::Expression>,
        )>,
        output_col_names: Vec<String>,
        memory_tracker: MemoryTracker,
        state: Option<AggregateState>,
    },
    GroupBy {
        group_by_expressions: Vec<graphdb_core::types::expr::Expression>,
        memory_tracker: MemoryTracker,
        state: Option<GroupByState>,
    },
    WindowFunction {
        window_exprs: Vec<graphdb_core::types::expr::Expression>,
        partition_by_exprs: Vec<graphdb_core::types::expr::Expression>,
        order_by_exprs: Vec<graphdb_core::types::expr::Expression>,
        order_by_directions: Vec<crate::executor::streaming::executor::SortDirection>,
        memory_tracker: MemoryTracker,
        state: Option<WindowFunctionState>,
    },
    Window {
        window_exprs: Vec<graphdb_core::types::expr::Expression>,
        partition_by_exprs: Vec<graphdb_core::types::expr::Expression>,
        order_by_exprs: Vec<graphdb_core::types::expr::Expression>,
        order_by_directions: Vec<crate::executor::streaming::executor::SortDirection>,
        memory_tracker: MemoryTracker,
        state: Option<WindowState>,
    },
    TopN {
        n: u32,
        sort_expressions: Vec<graphdb_core::types::expr::Expression>,
        sort_directions: Vec<crate::executor::streaming::executor::SortDirection>,
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
        rollup_expressions: Vec<graphdb_core::types::expr::Expression>,
        memory_tracker: MemoryTracker,
        state: Option<RollUpApplyState>,
    },
    PartialAggregate {
        group_by_expressions: Vec<graphdb_core::types::expr::Expression>,
        aggregate_functions: Vec<(
            graphdb_core::types::operators::AggregateFunction,
            Vec<graphdb_core::types::expr::Expression>,
        )>,
        output_col_names: Vec<String>,
        memory_tracker: MemoryTracker,
        state: Option<PartialAggregateState>,
    },
    FinalAggregate {
        group_by_expressions: Vec<graphdb_core::types::expr::Expression>,
        aggregate_functions: Vec<(
            graphdb_core::types::operators::AggregateFunction,
            Vec<graphdb_core::types::expr::Expression>,
        )>,
        output_col_names: Vec<String>,
        memory_tracker: MemoryTracker,
        state: Option<FinalAggregateState>,
    },
}

#[derive(Debug)]
pub struct BlockingOperator {
    pub kind: BlockingOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl BlockingOperator {
    pub fn from_spec(
        spec: &super::spec::BlockingSpec,
        memory_budget: &crate::executor::base::MemoryBudget,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let kind = match spec {
            super::spec::BlockingSpec::Sort {
                sort_expressions,
                sort_directions,
            } => BlockingOperatorKind::Sort {
                sort_expressions: sort_expressions.clone(),
                sort_directions: sort_directions.clone(),
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                state: None,
            },
            super::spec::BlockingSpec::Aggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names,
            } => BlockingOperatorKind::Aggregate {
                group_by_expressions: group_by_expressions.clone(),
                aggregate_functions: aggregate_functions.clone(),
                output_col_names: output_col_names.clone(),
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                state: None,
            },
            super::spec::BlockingSpec::GroupBy {
                group_by_expressions,
            } => BlockingOperatorKind::GroupBy {
                group_by_expressions: group_by_expressions.clone(),
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                state: None,
            },
            super::spec::BlockingSpec::WindowFunction {
                window_exprs,
                partition_by_exprs,
                order_by_exprs,
                order_by_directions,
            } => BlockingOperatorKind::WindowFunction {
                window_exprs: window_exprs.clone(),
                partition_by_exprs: partition_by_exprs.clone(),
                order_by_exprs: order_by_exprs.clone(),
                order_by_directions: order_by_directions.clone(),
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                state: None,
            },
            super::spec::BlockingSpec::Window {
                window_exprs,
                partition_by_exprs,
                order_by_exprs,
                order_by_directions,
            } => BlockingOperatorKind::Window {
                window_exprs: window_exprs.clone(),
                partition_by_exprs: partition_by_exprs.clone(),
                order_by_exprs: order_by_exprs.clone(),
                order_by_directions: order_by_directions.clone(),
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                state: None,
            },
            super::spec::BlockingSpec::TopN {
                n,
                sort_expressions,
                sort_directions,
            } => BlockingOperatorKind::TopN {
                n: *n,
                sort_expressions: sort_expressions.clone(),
                sort_directions: sort_directions.clone(),
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                state: None,
            },
            super::spec::BlockingSpec::Distinct => BlockingOperatorKind::Distinct {
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                state: None,
            },
            super::spec::BlockingSpec::Materialize => BlockingOperatorKind::Materialize {
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                state: None,
            },
            super::spec::BlockingSpec::DataCollect => BlockingOperatorKind::DataCollect {
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                state: None,
            },
            super::spec::BlockingSpec::RollUpApply { rollup_expressions } => {
                BlockingOperatorKind::RollUpApply {
                    rollup_expressions: rollup_expressions.clone(),
                    memory_tracker: crate::executor::base::MemoryTracker::new(
                        memory_budget.clone(),
                    ),
                    state: None,
                }
            }
            super::spec::BlockingSpec::PartialAggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names,
            } => BlockingOperatorKind::PartialAggregate {
                group_by_expressions: group_by_expressions.clone(),
                aggregate_functions: aggregate_functions.clone(),
                output_col_names: output_col_names.clone(),
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                state: None,
            },
            super::spec::BlockingSpec::FinalAggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names,
            } => BlockingOperatorKind::FinalAggregate {
                group_by_expressions: group_by_expressions.clone(),
                aggregate_functions: aggregate_functions.clone(),
                output_col_names: output_col_names.clone(),
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                state: None,
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn new(kind: BlockingOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            kind,
            runtime: None,
            output_layout,
            config: OperatorConfig::default(),
        }
    }

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

    pub fn memory_tracker(&self) -> &MemoryTracker {
        match &self.kind {
            BlockingOperatorKind::Sort { memory_tracker, .. }
            | BlockingOperatorKind::Aggregate { memory_tracker, .. }
            | BlockingOperatorKind::GroupBy { memory_tracker, .. }
            | BlockingOperatorKind::WindowFunction { memory_tracker, .. }
            | BlockingOperatorKind::Window { memory_tracker, .. }
            | BlockingOperatorKind::TopN { memory_tracker, .. }
            | BlockingOperatorKind::Distinct { memory_tracker, .. }
            | BlockingOperatorKind::Materialize { memory_tracker, .. }
            | BlockingOperatorKind::DataCollect { memory_tracker, .. }
            | BlockingOperatorKind::RollUpApply { memory_tracker, .. }
            | BlockingOperatorKind::PartialAggregate { memory_tracker, .. }
            | BlockingOperatorKind::FinalAggregate { memory_tracker, .. } => memory_tracker,
        }
    }

    pub fn open(&mut self, input: &mut StreamingExecutor) -> Result<(), QueryError> {
        match &mut self.kind {
            BlockingOperatorKind::Sort { state, .. } => {
                sort_operator::open_sort(state);
            }
            BlockingOperatorKind::Aggregate {
                state,
                aggregate_functions,
                ..
            } => {
                aggregate_operator::open_aggregate(state, aggregate_functions.len());
            }
            BlockingOperatorKind::GroupBy { state, .. } => {
                aggregate_operator::open_groupby(state);
            }
            BlockingOperatorKind::WindowFunction { state, .. } => {
                window_operator::open_window_function(state);
            }
            BlockingOperatorKind::Window { state, .. } => {
                window_operator::open_window(state);
            }
            BlockingOperatorKind::TopN { state, .. } => {
                sort_operator::open_topn(state);
            }
            BlockingOperatorKind::Distinct { state, .. } => {
                materialize_operator::open_distinct(state);
            }
            BlockingOperatorKind::Materialize { state, .. } => {
                materialize_operator::open_materialize(state);
            }
            BlockingOperatorKind::DataCollect { state, .. } => {
                materialize_operator::open_data_collect(state);
            }
            BlockingOperatorKind::RollUpApply { state, .. } => {
                materialize_operator::open_rollup_apply(state);
            }
            BlockingOperatorKind::PartialAggregate { state, .. } => {
                aggregate_operator::open_partial_aggregate(state);
            }
            BlockingOperatorKind::FinalAggregate { state, .. } => {
                aggregate_operator::open_final_aggregate(state);
            }
        }
        input.open()?;
        Ok(())
    }

    pub fn next(&mut self, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        let ctx = helpers::BlockingContext {
            runtime: &self.runtime,
            output_layout: &self.output_layout,
            config: &self.config,
        };

        match &mut self.kind {
            BlockingOperatorKind::Sort {
                sort_expressions,
                sort_directions,
                memory_tracker,
                state,
                ..
            } => sort_operator::next_sort(
                sort_expressions,
                sort_directions,
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
            BlockingOperatorKind::Aggregate {
                group_by_expressions,
                aggregate_functions,
                memory_tracker,
                state,
                ..
            } => aggregate_operator::next_aggregate(
                group_by_expressions,
                aggregate_functions,
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
            BlockingOperatorKind::GroupBy {
                group_by_expressions,
                memory_tracker,
                state,
                ..
            } => aggregate_operator::next_groupby(
                group_by_expressions,
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
            BlockingOperatorKind::WindowFunction {
                window_exprs,
                partition_by_exprs,
                order_by_exprs,
                order_by_directions,
                memory_tracker,
                state,
                ..
            } => window_operator::next_window_function(
                window_exprs,
                partition_by_exprs,
                order_by_exprs,
                order_by_directions,
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
            BlockingOperatorKind::Window {
                window_exprs,
                partition_by_exprs,
                order_by_exprs,
                order_by_directions,
                memory_tracker,
                state,
                ..
            } => window_operator::next_window(
                window_exprs,
                partition_by_exprs,
                order_by_exprs,
                order_by_directions,
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
            BlockingOperatorKind::TopN {
                n,
                sort_expressions,
                sort_directions,
                memory_tracker,
                state,
                ..
            } => sort_operator::next_topn(
                *n,
                sort_expressions,
                sort_directions,
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
            BlockingOperatorKind::Distinct {
                memory_tracker,
                state,
                ..
            } => materialize_operator::next_distinct(
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
            BlockingOperatorKind::Materialize {
                memory_tracker,
                state,
                ..
            } => materialize_operator::next_materialize(
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
            BlockingOperatorKind::DataCollect {
                memory_tracker,
                state,
                ..
            } => materialize_operator::next_data_collect(
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
            BlockingOperatorKind::RollUpApply {
                rollup_expressions,
                memory_tracker,
                state,
                ..
            } => materialize_operator::next_rollup_apply(
                rollup_expressions,
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
            BlockingOperatorKind::PartialAggregate {
                group_by_expressions,
                aggregate_functions,
                memory_tracker,
                state,
                ..
            } => aggregate_operator::next_partial_aggregate(
                group_by_expressions,
                aggregate_functions,
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
            BlockingOperatorKind::FinalAggregate {
                group_by_expressions,
                aggregate_functions,
                memory_tracker,
                state,
                ..
            } => aggregate_operator::next_final_aggregate(
                group_by_expressions,
                aggregate_functions,
                memory_tracker,
                state.as_mut().unwrap(),
                &ctx,
                input,
            ),
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        match &mut self.kind {
            BlockingOperatorKind::Sort {
                state,
                memory_tracker,
                ..
            } => {
                sort_operator::close_sort(state);
                memory_tracker.reset();
                Ok(())
            }
            BlockingOperatorKind::Aggregate {
                state,
                memory_tracker,
                ..
            } => {
                aggregate_operator::close_aggregate(state);
                memory_tracker.reset();
                Ok(())
            }
            BlockingOperatorKind::GroupBy {
                state,
                memory_tracker,
                ..
            } => {
                aggregate_operator::close_groupby(state);
                memory_tracker.reset();
                Ok(())
            }
            BlockingOperatorKind::WindowFunction {
                state,
                memory_tracker,
                ..
            } => {
                window_operator::close_window_function(state);
                memory_tracker.reset();
                Ok(())
            }
            BlockingOperatorKind::Window {
                state,
                memory_tracker,
                ..
            } => {
                window_operator::close_window(state);
                memory_tracker.reset();
                Ok(())
            }
            BlockingOperatorKind::TopN {
                state,
                memory_tracker,
                ..
            } => {
                sort_operator::close_topn(state);
                memory_tracker.reset();
                Ok(())
            }
            BlockingOperatorKind::RollUpApply {
                state,
                memory_tracker,
                ..
            } => {
                materialize_operator::close_rollup_apply(state);
                memory_tracker.reset();
                Ok(())
            }
            BlockingOperatorKind::Distinct {
                state,
                memory_tracker,
                ..
            } => {
                materialize_operator::close_distinct(state);
                memory_tracker.reset();
                Ok(())
            }
            BlockingOperatorKind::PartialAggregate {
                state,
                memory_tracker,
                ..
            } => {
                aggregate_operator::close_partial_aggregate(state);
                memory_tracker.reset();
                Ok(())
            }
            BlockingOperatorKind::FinalAggregate {
                state,
                memory_tracker,
                ..
            } => {
                aggregate_operator::close_final_aggregate(state);
                memory_tracker.reset();
                Ok(())
            }
            BlockingOperatorKind::Materialize {
                state,
                memory_tracker,
                ..
            } => {
                materialize_operator::close_materialize(state);
                memory_tracker.reset();
                Ok(())
            }
            BlockingOperatorKind::DataCollect {
                state,
                memory_tracker,
                ..
            } => {
                materialize_operator::close_data_collect(state);
                memory_tracker.reset();
                Ok(())
            }
        }
    }

    pub fn spill_with_manager(
        &mut self,
        sm: &crate::executor::streaming::spill::SpillManager,
    ) -> Result<(), QueryError> {
        match &mut self.kind {
            BlockingOperatorKind::Sort {
                state,
                memory_tracker,
                sort_expressions,
                sort_directions,
                ..
            } => sort_operator::spill_sort(
                state.as_mut().unwrap(),
                memory_tracker,
                sort_expressions,
                sort_directions,
                sm,
            ),
            BlockingOperatorKind::Aggregate {
                state,
                memory_tracker,
                ..
            } => aggregate_operator::spill_aggregate(state.as_mut().unwrap(), memory_tracker, sm),
            BlockingOperatorKind::GroupBy {
                state,
                memory_tracker,
                group_by_expressions,
                ..
            } => aggregate_operator::spill_groupby(
                state.as_mut().unwrap(),
                memory_tracker,
                group_by_expressions,
                sm,
            ),
            BlockingOperatorKind::WindowFunction {
                state,
                memory_tracker,
                partition_by_exprs,
                ..
            } => window_operator::spill_window_function(
                state.as_mut().unwrap(),
                memory_tracker,
                partition_by_exprs,
                sm,
            ),
            BlockingOperatorKind::Window {
                state,
                memory_tracker,
                partition_by_exprs,
                ..
            } => window_operator::spill_window(
                state.as_mut().unwrap(),
                memory_tracker,
                partition_by_exprs,
                sm,
            ),
            BlockingOperatorKind::TopN { .. } => Ok(()),
            BlockingOperatorKind::Distinct {
                state,
                memory_tracker,
                ..
            } => materialize_operator::spill_distinct(state.as_mut().unwrap(), memory_tracker, sm),
            BlockingOperatorKind::Materialize {
                state,
                memory_tracker,
                ..
            } => {
                materialize_operator::spill_materialize(state.as_mut().unwrap(), sm, memory_tracker)
            }
            BlockingOperatorKind::DataCollect {
                state,
                memory_tracker,
                ..
            } => materialize_operator::spill_data_collect(
                state.as_mut().unwrap(),
                sm,
                memory_tracker,
            ),
            BlockingOperatorKind::PartialAggregate { state, .. } => {
                aggregate_operator::spill_partial_aggregate(state)
            }
            BlockingOperatorKind::RollUpApply {
                state,
                memory_tracker,
                ..
            } => materialize_operator::spill_rollup_apply(
                state.as_mut().unwrap(),
                sm,
                memory_tracker,
            ),
            BlockingOperatorKind::FinalAggregate { state, .. } => {
                aggregate_operator::spill_final_aggregate(state)
            }
        }
    }

    pub fn spill_count(&self) -> u64 {
        match &self.kind {
            BlockingOperatorKind::Sort { state, .. } => {
                state.as_ref().map_or(0, |s| s.runs.len() as u64)
            }
            BlockingOperatorKind::Aggregate { state, .. } => state.as_ref().map_or(0, |s| {
                s.spilled_runs.iter().filter_map(|r| r.as_ref()).count() as u64
            }),
            BlockingOperatorKind::GroupBy { state, .. } => state.as_ref().map_or(0, |s| {
                s.spilled_runs.iter().filter_map(|r| r.as_ref()).count() as u64
            }),
            BlockingOperatorKind::WindowFunction { state, .. } => state.as_ref().map_or(0, |s| {
                s.spilled_runs.iter().filter_map(|r| r.as_ref()).count() as u64
            }),
            BlockingOperatorKind::Window { state, .. } => state.as_ref().map_or(0, |s| {
                s.spilled_runs.iter().filter_map(|r| r.as_ref()).count() as u64
            }),
            BlockingOperatorKind::Distinct { state, .. } => state.as_ref().map_or(0, |s| {
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
        match &self.kind {
            BlockingOperatorKind::Sort { state, .. } => {
                let base = sum_spill!(state);
                let run_bytes: u64 = state
                    .as_ref()
                    .map_or(0, |s| s.runs.iter().map(|r| r.byte_size).sum::<u64>());
                base + run_bytes
            }
            BlockingOperatorKind::Aggregate { state, .. } => {
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
            BlockingOperatorKind::GroupBy { state, .. } => {
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
            BlockingOperatorKind::WindowFunction { state, .. } => {
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
            BlockingOperatorKind::Window { state, .. } => {
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
            BlockingOperatorKind::TopN { .. } => 0,
            BlockingOperatorKind::Distinct { state, .. } => {
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
            BlockingOperatorKind::Materialize { state, .. } => sum_spill!(state),
            BlockingOperatorKind::DataCollect { state, .. } => sum_spill!(state),
            BlockingOperatorKind::RollUpApply { state, .. } => sum_spill!(state),
            BlockingOperatorKind::PartialAggregate { state, .. } => sum_spill!(state),
            BlockingOperatorKind::FinalAggregate { state, .. } => sum_spill!(state),
        }
    }
}

#[cfg(test)]
#[path = "blocking/test.rs"]
mod tests;
