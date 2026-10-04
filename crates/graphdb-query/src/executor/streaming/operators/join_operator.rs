use std::collections::HashSet;
use std::sync::Arc;

use crate::executor::base::MemoryTracker;
use crate::executor::streaming::chunk::{use_columnar_path, DataChunk};
use crate::executor::streaming::executor::FullOuterJoinPhase;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::Value;

mod cross_join;
pub mod grace_join;
mod hash_join;
mod merge_join;
mod nested_loop_join;
mod semi_join;

pub use hash_join::{HashJoinBuildSide, JoinKeyValue};

fn build_combined_names(
    left_col_names: &[String],
    right_col_names: &[String],
    fallback_right_width: usize,
) -> Vec<String> {
    let mut names = left_col_names.to_vec();
    if !right_col_names.is_empty() {
        names.extend_from_slice(right_col_names);
    } else {
        for i in 0..fallback_right_width {
            names.push(format!("right_{}", i));
        }
    }
    names
}

/// Defer the typed columnar layout on a freshly materialized join output.
///
/// Join outputs are assembled from rows and (unlike the storage scan) start
/// row-major, which would otherwise drop the typed fast path for every
/// operator downstream of a join. Instead of building eagerly, the chunk is
/// marked deferred when the shared [`ColumnarPolicy`] gate allows columnar:
/// the first typed consumer rebuilds the layout via
/// [`DataChunk::ensure_typed_columns`], so large join results that are never
/// read as typed columns pay no build cost. `pub(crate)` so the join modules
/// can reuse it.
pub(crate) fn finalize_join_output(
    mut chunk: DataChunk,
    runtime: &Option<Arc<ExecutionRuntime>>,
) -> DataChunk {
    if use_columnar_path(runtime) {
        chunk.columnar_build_deferred = true;
    }
    chunk
}

#[derive(Debug)]
pub enum JoinOperatorKind {
    HashJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side: HashJoinBuildSide,
        build_done: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
        build_side_select: super::spec::BuildSide,
        grace: grace_join::GraceJoinState,
    },
    HashLeftJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side: HashJoinBuildSide,
        build_done: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
        build_side_select: super::spec::BuildSide,
        grace: grace_join::GraceJoinState,
    },
    NestedLoopJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        build_done: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    InnerJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        build_done: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    LeftJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        build_done: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    RightJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    FullOuterJoin {
        join_condition: Option<Expression>,
        left_rows: Vec<Vec<Value>>,
        right_rows: Vec<Vec<Value>>,
        matched_right_indices: HashSet<usize>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        phase: FullOuterJoinPhase,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    CrossJoin {
        all_left_rows: Vec<Vec<Value>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
        output_done: bool,
    },
    SemiJoin {
        join_condition: Option<Expression>,
        // NOT EXISTS semantics: keep left rows with NO matching right row.
        anti: bool,
        right_rows: Vec<Vec<Value>>,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
}

/// Join operator.
///
/// Wraps [`JoinOperatorKind`] with the runtime context injected at `open()`.
/// Lifecycle state is owned exclusively by the executor; operators never
/// write it.
#[derive(Debug)]
pub struct JoinOperator {
    pub kind: JoinOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl JoinOperator {
    pub fn new(kind: JoinOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            kind,
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

    pub fn from_spec(
        spec: &super::spec::JoinSpec,
        memory_budget: &crate::executor::base::MemoryBudget,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let kind = match spec {
            super::spec::JoinSpec::HashJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
            } => JoinOperatorKind::HashJoin {
                join_condition: join_condition.clone(),
                hash_keys: hash_keys.clone(),
                probe_keys: probe_keys.clone(),
                build_side: HashJoinBuildSide::new(),
                build_done: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
                build_side_select: *build_side,
                grace: grace_join::GraceJoinState::default(),
            },
            super::spec::JoinSpec::HashLeftJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
            } => JoinOperatorKind::HashLeftJoin {
                join_condition: join_condition.clone(),
                hash_keys: hash_keys.clone(),
                probe_keys: probe_keys.clone(),
                build_side: HashJoinBuildSide::new(),
                build_done: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
                build_side_select: *build_side,
                grace: grace_join::GraceJoinState::default(),
            },
            super::spec::JoinSpec::NestedLoopJoin { join_condition } => {
                JoinOperatorKind::NestedLoopJoin {
                    join_condition: join_condition.clone(),
                    build_side_tuples: Vec::new(),
                    build_done: false,
                    memory_tracker: crate::executor::base::MemoryTracker::new(
                        memory_budget.clone(),
                    ),
                    right_col_names: Vec::new(),
                }
            }
            super::spec::JoinSpec::InnerJoin { join_condition } => JoinOperatorKind::InnerJoin {
                join_condition: join_condition.clone(),
                build_side_tuples: Vec::new(),
                build_done: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
            },
            super::spec::JoinSpec::LeftJoin { join_condition } => JoinOperatorKind::LeftJoin {
                join_condition: join_condition.clone(),
                build_side_tuples: Vec::new(),
                build_done: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
            },
            super::spec::JoinSpec::RightJoin { join_condition } => JoinOperatorKind::RightJoin {
                join_condition: join_condition.clone(),
                build_side_tuples: Vec::new(),
                right_consumed: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
            },
            super::spec::JoinSpec::FullOuterJoin { join_condition } => {
                JoinOperatorKind::FullOuterJoin {
                    join_condition: join_condition.clone(),
                    left_rows: Vec::new(),
                    right_rows: Vec::new(),
                    matched_right_indices: std::collections::HashSet::new(),
                    result_iter: None,
                    phase: FullOuterJoinPhase::BuildingRight,
                    memory_tracker: crate::executor::base::MemoryTracker::new(
                        memory_budget.clone(),
                    ),
                    right_col_names: Vec::new(),
                }
            }
            super::spec::JoinSpec::CrossJoin => JoinOperatorKind::CrossJoin {
                all_left_rows: Vec::new(),
                all_right_rows: Vec::new(),
                left_consumed: false,
                right_consumed: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
                output_done: false,
            },
            super::spec::JoinSpec::SemiJoin {
                join_condition,
                anti,
            } => JoinOperatorKind::SemiJoin {
                join_condition: join_condition.clone(),
                anti: *anti,
                right_rows: Vec::new(),
                right_consumed: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn memory_tracker(&self) -> &MemoryTracker {
        match &self.kind {
            JoinOperatorKind::HashJoin { memory_tracker, .. }
            | JoinOperatorKind::HashLeftJoin { memory_tracker, .. }
            | JoinOperatorKind::NestedLoopJoin { memory_tracker, .. }
            | JoinOperatorKind::InnerJoin { memory_tracker, .. }
            | JoinOperatorKind::LeftJoin { memory_tracker, .. }
            | JoinOperatorKind::RightJoin { memory_tracker, .. }
            | JoinOperatorKind::FullOuterJoin { memory_tracker, .. }
            | JoinOperatorKind::CrossJoin { memory_tracker, .. }
            | JoinOperatorKind::SemiJoin { memory_tracker, .. } => memory_tracker,
        }
    }

    pub fn open(
        &mut self,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        left.open()?;
        right.open()?;
        Ok(())
    }

    pub fn next(
        &mut self,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        let runtime = &self.runtime;
        let output_layout = &self.output_layout;
        match &mut self.kind {
            JoinOperatorKind::HashJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                build_side_select,
                grace,
            } => hash_join::next_hash_join(
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                *build_side_select,
                grace,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::HashLeftJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                build_side_select,
                grace,
            } => hash_join::next_hash_left_join(
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                *build_side_select,
                grace,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::NestedLoopJoin {
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
            } => nested_loop_join::next_nested_loop_join(
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::InnerJoin {
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
            } => merge_join::next_inner_join(
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::LeftJoin {
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
            } => merge_join::next_left_join(
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::RightJoin {
                join_condition,
                build_side_tuples,
                right_consumed,
                memory_tracker,
                right_col_names,
            } => merge_join::next_right_join(
                join_condition,
                build_side_tuples,
                right_consumed,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::FullOuterJoin {
                join_condition,
                left_rows,
                right_rows,
                matched_right_indices,
                result_iter,
                phase,
                memory_tracker,
                right_col_names,
            } => merge_join::next_full_outer_join(
                join_condition,
                left_rows,
                right_rows,
                matched_right_indices,
                result_iter,
                phase,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::CrossJoin {
                all_left_rows,
                all_right_rows,
                left_consumed,
                right_consumed,
                memory_tracker,
                right_col_names,
                output_done,
            } => cross_join::next_cross_join(
                all_left_rows,
                all_right_rows,
                left_consumed,
                right_consumed,
                output_done,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::SemiJoin {
                join_condition,
                anti,
                right_rows,
                right_consumed,
                memory_tracker,
                right_col_names,
            } => semi_join::next_semi_join(
                join_condition,
                *anti,
                right_rows,
                right_consumed,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    /// Reset per-run join state (hash tables, buffered sides, phase flags)
    /// and rewind both inputs so the join re-produces the same result set.
    pub fn reset(
        &mut self,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<bool, QueryError> {
        match &mut self.kind {
            JoinOperatorKind::HashJoin {
                build_side,
                build_done,
                right_col_names,
                grace,
                memory_tracker,
                ..
            }
            | JoinOperatorKind::HashLeftJoin {
                build_side,
                build_done,
                right_col_names,
                grace,
                memory_tracker,
                ..
            } => {
                grace.cleanup_files();
                *grace = grace_join::GraceJoinState::default();
                *build_side = HashJoinBuildSide::new();
                *build_done = false;
                right_col_names.clear();
                memory_tracker.reset();
            }
            JoinOperatorKind::NestedLoopJoin {
                build_side_tuples,
                build_done,
                right_col_names,
                ..
            }
            | JoinOperatorKind::InnerJoin {
                build_side_tuples,
                build_done,
                right_col_names,
                ..
            }
            | JoinOperatorKind::LeftJoin {
                build_side_tuples,
                build_done,
                right_col_names,
                ..
            } => {
                build_side_tuples.clear();
                *build_done = false;
                right_col_names.clear();
            }
            JoinOperatorKind::RightJoin {
                build_side_tuples,
                right_consumed,
                right_col_names,
                ..
            } => {
                build_side_tuples.clear();
                *right_consumed = false;
                right_col_names.clear();
            }
            JoinOperatorKind::FullOuterJoin {
                left_rows,
                right_rows,
                matched_right_indices,
                result_iter,
                phase,
                right_col_names,
                ..
            } => {
                left_rows.clear();
                right_rows.clear();
                matched_right_indices.clear();
                *result_iter = None;
                *phase = FullOuterJoinPhase::BuildingRight;
                right_col_names.clear();
            }
            JoinOperatorKind::CrossJoin {
                all_left_rows,
                all_right_rows,
                left_consumed,
                right_consumed,
                right_col_names,
                output_done,
                ..
            } => {
                all_left_rows.clear();
                all_right_rows.clear();
                *left_consumed = false;
                *right_consumed = false;
                right_col_names.clear();
                *output_done = false;
            }
            JoinOperatorKind::SemiJoin {
                right_rows,
                right_consumed,
                right_col_names,
                ..
            } => {
                right_rows.clear();
                *right_consumed = false;
                right_col_names.clear();
            }
        }
        left.reset()?;
        right.reset()?;
        Ok(false)
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        match &mut self.kind {
            JoinOperatorKind::HashJoin {
                build_side,
                memory_tracker,
                grace,
                ..
            } => {
                grace.cleanup_files();
                hash_join::close(memory_tracker, build_side)
            }
            JoinOperatorKind::HashLeftJoin {
                build_side,
                memory_tracker,
                grace,
                ..
            } => {
                grace.cleanup_files();
                hash_join::close(memory_tracker, build_side)
            }
            JoinOperatorKind::NestedLoopJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(memory_tracker, build_side_tuples),
            JoinOperatorKind::InnerJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(memory_tracker, build_side_tuples),
            JoinOperatorKind::LeftJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(memory_tracker, build_side_tuples),
            JoinOperatorKind::RightJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(memory_tracker, build_side_tuples),
            JoinOperatorKind::FullOuterJoin {
                left_rows,
                right_rows,
                memory_tracker,
                ..
            } => merge_join::close_full_outer(memory_tracker, left_rows, right_rows),
            JoinOperatorKind::CrossJoin {
                all_left_rows,
                all_right_rows,
                memory_tracker,
                ..
            } => cross_join::close_cross(memory_tracker, all_left_rows, all_right_rows),
            JoinOperatorKind::SemiJoin {
                right_rows,
                memory_tracker,
                ..
            } => semi_join::close_semi(memory_tracker, right_rows),
        }
    }

    /// Spill the hash join build side to partitioned runs.
    ///
    /// Only effective during the build phase (before `build_done`): buffered
    /// build rows are re-partitioned to disk and the remaining build input is
    /// routed straight to the spill writers by the build loop. Once the probe
    /// phase has started the in-memory table must stay resident and this is a
    /// no-op; other join kinds keep their budget-failure semantics.
    pub fn spill_with_manager(
        &mut self,
        sm: &Arc<crate::executor::streaming::spill::SpillManager>,
    ) -> Result<(), graphdb_core::error::QueryError> {
        let (build_side, build_done, memory_tracker, right_col_names, grace) = match &mut self.kind
        {
            JoinOperatorKind::HashJoin {
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                grace,
                ..
            }
            | JoinOperatorKind::HashLeftJoin {
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                grace,
                ..
            } => (
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                grace,
            ),
            _ => return Ok(()),
        };
        if *build_done || grace.partitioned.is_some() {
            return Ok(());
        }
        if build_side.row_count() == 0 && grace.pending_build.is_none() {
            return Ok(());
        }
        grace_join::spill_build_side(
            Arc::clone(sm),
            build_side,
            memory_tracker,
            right_col_names,
            grace,
        )
    }

    pub fn spilled_bytes(&self) -> u64 {
        match &self.kind {
            JoinOperatorKind::HashJoin { grace, .. }
            | JoinOperatorKind::HashLeftJoin { grace, .. } => grace.spilled_bytes,
            _ => 0,
        }
    }

    pub fn spill_count(&self) -> u64 {
        match &self.kind {
            JoinOperatorKind::HashJoin { grace, .. }
            | JoinOperatorKind::HashLeftJoin { grace, .. } => grace.spill_runs,
            _ => 0,
        }
    }

    pub fn spilled_rows(&self) -> u64 {
        match &self.kind {
            JoinOperatorKind::HashJoin { grace, .. }
            | JoinOperatorKind::HashLeftJoin { grace, .. } => grace.spilled_rows,
            _ => 0,
        }
    }
}
