use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::MemoryTracker;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::FullOuterJoinPhase;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::executor::ValueRowContext;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::base::OperatorLifecycle;
use crate::query::executor::streaming::slot::SlotLayout;

mod hash_join;
mod nested_loop_join;
mod merge_join;
mod cross_semi_join;

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

fn evaluate_join_key(
    row: &[Value],
    col_names: &[String],
    key_expressions: &[Expression],
) -> Result<Vec<Value>, QueryError> {
    if key_expressions.is_empty() {
        return Ok(Vec::new());
    }

    let layout = Arc::new(SlotLayout::from_names(col_names));
    let mut key = Vec::with_capacity(key_expressions.len());
    for expr in key_expressions {
        let mut context = ValueRowContext::new(row.to_vec(), layout.clone());
        let value = ExpressionEvaluator::evaluate(expr, &mut context)
            .map_err(|e| QueryError::execution(format!("HashJoin key evaluation failed: {}", e)))?;
        key.push(value);
    }
    Ok(key)
}

fn close_common(
    lifecycle: &mut OperatorLifecycle,
    memory_tracker: &mut MemoryTracker,
    clear: impl FnOnce(),
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
) -> Result<(), QueryError> {
    if lifecycle.can_close() {
        memory_tracker.reset();
        clear();
        let left_err = left.close().err();
        let right_err = right.close().err();
        lifecycle.mark_closed();
        match (left_err, right_err) {
            (Some(e), _) => Err(e),
            (_, Some(e)) => Err(e),
            _ => Ok(()),
        }
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub enum JoinOperator {
    HashJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side_hash: HashMap<Vec<Value>, Vec<Vec<Value>>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    HashLeftJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side_hash: HashMap<Vec<Value>, Vec<Vec<Value>>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    NestedLoopJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    InnerJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    LeftJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
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
    },
    SemiJoin {
        join_condition: Option<Expression>,
        right_rows: Vec<Vec<Value>>,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
}

impl JoinOperator {
    pub fn from_spec(
        spec: &super::spec::JoinSpec,
        memory_budget: &crate::query::executor::base::MemoryBudget,
    ) -> Self {
        match spec {
            super::spec::JoinSpec::HashJoin {
                join_condition,
                hash_keys,
                probe_keys,
            } => Self::HashJoin {
                join_condition: join_condition.clone(),
                hash_keys: hash_keys.clone(),
                probe_keys: probe_keys.clone(),
                build_side_hash: std::collections::HashMap::new(),
                all_right_rows: Vec::new(),
                left_consumed: false,
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                right_col_names: Vec::new(),
            },
            super::spec::JoinSpec::HashLeftJoin {
                join_condition,
                hash_keys,
                probe_keys,
            } => Self::HashLeftJoin {
                join_condition: join_condition.clone(),
                hash_keys: hash_keys.clone(),
                probe_keys: probe_keys.clone(),
                build_side_hash: std::collections::HashMap::new(),
                all_right_rows: Vec::new(),
                left_consumed: false,
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                right_col_names: Vec::new(),
            },
            super::spec::JoinSpec::NestedLoopJoin { join_condition } => {
                Self::NestedLoopJoin {
                    join_condition: join_condition.clone(),
                    build_side_tuples: Vec::new(),
                    left_consumed: false,
                    memory_tracker: crate::query::executor::base::MemoryTracker::new(
                        memory_budget.clone(),
                    ),
                    right_col_names: Vec::new(),
                }
            }
            super::spec::JoinSpec::InnerJoin { join_condition } => {
                Self::InnerJoin {
                    join_condition: join_condition.clone(),
                    build_side_tuples: Vec::new(),
                    left_consumed: false,
                    memory_tracker: crate::query::executor::base::MemoryTracker::new(
                        memory_budget.clone(),
                    ),
                    right_col_names: Vec::new(),
                }
            }
            super::spec::JoinSpec::LeftJoin { join_condition } => Self::LeftJoin {
                join_condition: join_condition.clone(),
                build_side_tuples: Vec::new(),
                left_consumed: false,
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                right_col_names: Vec::new(),
            },
            super::spec::JoinSpec::RightJoin { join_condition } => {
                Self::RightJoin {
                    join_condition: join_condition.clone(),
                    build_side_tuples: Vec::new(),
                    right_consumed: false,
                    memory_tracker: crate::query::executor::base::MemoryTracker::new(
                        memory_budget.clone(),
                    ),
                    right_col_names: Vec::new(),
                }
            }
            super::spec::JoinSpec::FullOuterJoin { join_condition } => {
                Self::FullOuterJoin {
                    join_condition: join_condition.clone(),
                    left_rows: Vec::new(),
                    right_rows: Vec::new(),
                    matched_right_indices: std::collections::HashSet::new(),
                    result_iter: None,
                    phase: FullOuterJoinPhase::BuildingRight,
                    memory_tracker: crate::query::executor::base::MemoryTracker::new(
                        memory_budget.clone(),
                    ),
                    right_col_names: Vec::new(),
                }
            }
            super::spec::JoinSpec::CrossJoin => Self::CrossJoin {
                all_left_rows: Vec::new(),
                all_right_rows: Vec::new(),
                left_consumed: false,
                right_consumed: false,
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                right_col_names: Vec::new(),
            },
            super::spec::JoinSpec::SemiJoin { join_condition } => Self::SemiJoin {
                join_condition: join_condition.clone(),
                right_rows: Vec::new(),
                right_consumed: false,
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    memory_budget.clone(),
                ),
                right_col_names: Vec::new(),
            },
        }
    }

    pub fn memory_tracker(&self) -> &MemoryTracker {
        match self {
            Self::HashJoin { memory_tracker, .. }
            | Self::HashLeftJoin { memory_tracker, .. }
            | Self::NestedLoopJoin { memory_tracker, .. }
            | Self::InnerJoin { memory_tracker, .. }
            | Self::LeftJoin { memory_tracker, .. }
            | Self::RightJoin { memory_tracker, .. }
            | Self::FullOuterJoin { memory_tracker, .. }
            | Self::CrossJoin { memory_tracker, .. }
            | Self::SemiJoin { memory_tracker, .. } => memory_tracker,
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::HashJoin { .. }
            | Self::HashLeftJoin { .. }
            | Self::NestedLoopJoin { .. }
            | Self::InnerJoin { .. }
            | Self::LeftJoin { .. }
            | Self::RightJoin { .. }
            | Self::FullOuterJoin { .. }
            | Self::CrossJoin { .. }
            | Self::SemiJoin { .. } => {
                left.open()?;
                right.open()?;
                base.lifecycle.mark_opened();
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::HashJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side_hash,
                all_right_rows,
                left_consumed,
                memory_tracker,
                right_col_names,
            } => hash_join::next_hash_join(
                join_condition,
                hash_keys,
                probe_keys,
                build_side_hash,
                all_right_rows,
                left_consumed,
                memory_tracker,
                right_col_names,
                base,
                left,
                right,
            ),
            Self::HashLeftJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side_hash,
                all_right_rows,
                left_consumed,
                memory_tracker,
                right_col_names,
            } => hash_join::next_hash_left_join(
                join_condition,
                hash_keys,
                probe_keys,
                build_side_hash,
                all_right_rows,
                left_consumed,
                memory_tracker,
                right_col_names,
                base,
                left,
                right,
            ),
            Self::NestedLoopJoin {
                join_condition,
                build_side_tuples,
                left_consumed,
                memory_tracker,
                right_col_names,
            } => nested_loop_join::next_nested_loop_join(
                join_condition,
                build_side_tuples,
                left_consumed,
                memory_tracker,
                right_col_names,
                base,
                left,
                right,
            ),
            Self::InnerJoin {
                join_condition,
                build_side_tuples,
                left_consumed,
                memory_tracker,
                right_col_names,
            } => merge_join::next_inner_join(
                join_condition,
                build_side_tuples,
                left_consumed,
                memory_tracker,
                right_col_names,
                base,
                left,
                right,
            ),
            Self::LeftJoin {
                join_condition,
                build_side_tuples,
                left_consumed,
                memory_tracker,
                right_col_names,
            } => merge_join::next_left_join(
                join_condition,
                build_side_tuples,
                left_consumed,
                memory_tracker,
                right_col_names,
                base,
                left,
                right,
            ),
            Self::RightJoin {
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
                base,
                left,
                right,
            ),
            Self::FullOuterJoin {
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
                base,
                left,
                right,
            ),
            Self::CrossJoin {
                all_left_rows,
                all_right_rows,
                left_consumed,
                right_consumed,
                memory_tracker,
                right_col_names,
            } => cross_semi_join::next_cross_join(
                all_left_rows,
                all_right_rows,
                left_consumed,
                right_consumed,
                memory_tracker,
                right_col_names,
                base,
                left,
                right,
            ),
            Self::SemiJoin {
                join_condition,
                right_rows,
                right_consumed,
                memory_tracker,
                ..
            } => cross_semi_join::next_semi_join(
                join_condition,
                right_rows,
                right_consumed,
                memory_tracker,
                base,
                left,
                right,
            ),
        }
    }

    pub fn stop(
        &mut self,
        _base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::HashJoin { .. }
            | Self::HashLeftJoin { .. }
            | Self::NestedLoopJoin { .. }
            | Self::InnerJoin { .. }
            | Self::LeftJoin { .. }
            | Self::RightJoin { .. }
            | Self::FullOuterJoin { .. }
            | Self::CrossJoin { .. }
            | Self::SemiJoin { .. } => {
                left.stop()?;
                right.stop()
            }
        }
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::HashJoin {
                build_side_hash,
                all_right_rows,
                memory_tracker,
                ..
            } => hash_join::close(
                &mut base.lifecycle,
                memory_tracker,
                build_side_hash,
                all_right_rows,
                left,
                right,
            ),
            Self::HashLeftJoin {
                build_side_hash,
                all_right_rows,
                memory_tracker,
                ..
            } => hash_join::close(
                &mut base.lifecycle,
                memory_tracker,
                build_side_hash,
                all_right_rows,
                left,
                right,
            ),
            Self::NestedLoopJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(
                &mut base.lifecycle,
                memory_tracker,
                build_side_tuples,
                left,
                right,
            ),
            Self::InnerJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(
                &mut base.lifecycle,
                memory_tracker,
                build_side_tuples,
                left,
                right,
            ),
            Self::LeftJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(
                &mut base.lifecycle,
                memory_tracker,
                build_side_tuples,
                left,
                right,
            ),
            Self::RightJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(
                &mut base.lifecycle,
                memory_tracker,
                build_side_tuples,
                left,
                right,
            ),
            Self::FullOuterJoin {
                left_rows,
                right_rows,
                memory_tracker,
                ..
            } => merge_join::close_full_outer(
                &mut base.lifecycle,
                memory_tracker,
                left_rows,
                right_rows,
                left,
                right,
            ),
            Self::CrossJoin {
                all_left_rows,
                all_right_rows,
                memory_tracker,
                ..
            } => cross_semi_join::close_cross(
                &mut base.lifecycle,
                memory_tracker,
                all_left_rows,
                all_right_rows,
                left,
                right,
            ),
            Self::SemiJoin {
                right_rows,
                memory_tracker,
                ..
            } => cross_semi_join::close_semi(
                &mut base.lifecycle,
                memory_tracker,
                right_rows,
                left,
                right,
            ),
        }
    }

    pub fn spill_with_manager(&mut self, _sm: &crate::query::executor::streaming::spill::SpillManager) -> Result<(), crate::core::error::QueryError> {
        Ok(())
    }

    pub fn spilled_bytes(&self) -> u64 {
        0
    }
}
