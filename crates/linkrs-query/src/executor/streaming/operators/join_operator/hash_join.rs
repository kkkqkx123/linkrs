use std::sync::Arc;

use crate::executor::base::MemoryTracker;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::context::SplitRowContext;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::spec::BuildSide;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use linkrs_core::error::QueryError;
use linkrs_core::types::expr::Expression;
use linkrs_core::value::NullType;
use linkrs_core::Value;

use super::grace_join::GraceJoinState;
use super::{build_combined_names, finalize_join_output};

mod build_side;
mod partitioned;
#[cfg(test)]
mod tests;

pub use build_side::{HashJoinBuildSide, JoinKeyValue};
pub(in crate::executor::streaming::operators::join_operator) use build_side::evaluate_join_key;
use partitioned::{
    build_side_loop, enter_partitioned_mode, next_partitioned, sync_grace_counters,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn next_hash_join(
    join_condition: &mut Option<Expression>,
    hash_keys: &mut [Expression],
    probe_keys: &mut [Expression],
    build_side: &mut HashJoinBuildSide,
    build_done: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    side: BuildSide,
    grace: &mut GraceJoinState,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
    runtime: &Option<Arc<ExecutionRuntime>>,
    output_layout: &Arc<SlotLayout>,
) -> Result<Option<DataChunk>, QueryError> {
    debug_assert_eq!(
        probe_keys.len(),
        hash_keys.len(),
        "hash join probe/hash key widths must match"
    );
    if let Some(partitioned) = grace.partitioned.as_mut() {
        let result = next_partitioned(
            join_condition,
            hash_keys,
            probe_keys,
            partitioned,
            memory_tracker,
            false,
            runtime,
            output_layout,
        );
        sync_grace_counters(grace);
        return result;
    }
    let (build_input, probe_input): (&mut StreamingExecutor, &mut StreamingExecutor) = match side {
        BuildSide::Left => (left, right),
        BuildSide::Right => (right, left),
    };
    if !*build_done {
        build_side_loop(
            hash_keys,
            build_side,
            memory_tracker,
            right_col_names,
            build_input,
            runtime,
            build_done,
            grace,
        )?;
        if let Some(pending) = grace.pending_build.take() {
            enter_partitioned_mode(
                hash_keys,
                probe_keys,
                pending,
                probe_input,
                runtime,
                memory_tracker,
                grace,
            )?;
            let partitioned = grace.partitioned.as_mut().expect("partitioned must exist");
            let result = next_partitioned(
                join_condition,
                hash_keys,
                probe_keys,
                partitioned,
                memory_tracker,
                false,
                runtime,
                output_layout,
            );
            sync_grace_counters(grace);
            return result;
        }
    } else if grace.partitioned.is_some() {
        let partitioned = grace.partitioned.as_mut().expect("partitioned must exist");
        let result = next_partitioned(
            join_condition,
            hash_keys,
            probe_keys,
            partitioned,
            memory_tracker,
            false,
            runtime,
            output_layout,
        );
        sync_grace_counters(grace);
        return result;
    }

    while let Some(mut probe_chunk) = probe_input.advance()? {
        // Opaque consumer: expand symbolic multiplicity so each logical row
        // probes once. The selection vector is kept in place (consumed via
        // `visible_indices` below with no compaction benefit).
        let _ = probe_chunk.expand_multiplicity_in_place();
        let probe_col_names = probe_chunk.col_names();
        let mut result_rows = Vec::new();

        let combined_layout = if join_condition.is_some() {
            let fallback_width = right_col_names.len();
            let names = build_combined_names(&probe_col_names, right_col_names, fallback_width);
            Some(Arc::new(SlotLayout::from_names(&names)))
        } else {
            None
        };
        // The probe side consumes the child's selection vector — only
        // visible rows are probed. Keys evaluate per row through the scalar
        // interpreter; row storage is the only column source.
        for row_idx in probe_chunk.visible_indices() {
            let probe_row = &probe_chunk.rows[row_idx];
            let probe_key = evaluate_join_key(probe_row, &probe_col_names, probe_keys)?;

            if let Some(right_indices) = build_side.matching(&probe_key) {
                if let Some((condition, layout)) =
                    join_condition.as_ref().zip(combined_layout.as_ref())
                {
                    for &right_idx in right_indices {
                        let right_row = build_side.row_at(right_idx);
                        let mut split_ctx =
                            SplitRowContext::new(probe_row, &right_row, Arc::clone(layout));
                        if matches!(
                            ExpressionEvaluator::evaluate(condition, &mut split_ctx),
                            Ok(Value::Bool(b)) if b
                        ) {
                            let mut combined =
                                Vec::with_capacity(probe_row.len() + right_row.len());
                            combined.extend_from_slice(probe_row);
                            combined.extend_from_slice(&right_row);
                            result_rows.push(combined);
                        }
                    }
                } else {
                    for &right_idx in right_indices {
                        // Direct append into the output row: avoids the
                        // intermediate `Vec` allocated by `row_at()`.
                        let mut joined_row =
                            Vec::with_capacity(probe_row.len() + build_side.row_count().min(16));
                        joined_row.extend_from_slice(probe_row);
                        build_side.append_row_to(&mut joined_row, right_idx);
                        result_rows.push(joined_row);
                    }
                }
            }
        }

        if !result_rows.is_empty() {
            return Ok(Some(finalize_join_output(
                DataChunk::new_with_layout(result_rows, Arc::clone(output_layout)),
                runtime,
            )));
        }
    }

    Ok(None)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn next_hash_left_join(
    join_condition: &mut Option<Expression>,
    hash_keys: &mut [Expression],
    probe_keys: &mut [Expression],
    build_side: &mut HashJoinBuildSide,
    build_done: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    side: BuildSide,
    grace: &mut GraceJoinState,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
    runtime: &Option<Arc<ExecutionRuntime>>,
    output_layout: &Arc<SlotLayout>,
) -> Result<Option<DataChunk>, QueryError> {
    debug_assert_eq!(
        probe_keys.len(),
        hash_keys.len(),
        "hash left join probe/hash key widths must match"
    );
    if let Some(partitioned) = grace.partitioned.as_mut() {
        let result = next_partitioned(
            join_condition,
            hash_keys,
            probe_keys,
            partitioned,
            memory_tracker,
            true,
            runtime,
            output_layout,
        );
        sync_grace_counters(grace);
        return result;
    }
    let (build_input, probe_input): (&mut StreamingExecutor, &mut StreamingExecutor) = match side {
        BuildSide::Left => (left, right),
        BuildSide::Right => (right, left),
    };
    if !*build_done {
        build_side_loop(
            hash_keys,
            build_side,
            memory_tracker,
            right_col_names,
            build_input,
            runtime,
            build_done,
            grace,
        )?;
        if let Some(pending) = grace.pending_build.take() {
            enter_partitioned_mode(
                hash_keys,
                probe_keys,
                pending,
                probe_input,
                runtime,
                memory_tracker,
                grace,
            )?;
            let partitioned = grace.partitioned.as_mut().expect("partitioned must exist");
            let result = next_partitioned(
                join_condition,
                hash_keys,
                probe_keys,
                partitioned,
                memory_tracker,
                true,
                runtime,
                output_layout,
            );
            sync_grace_counters(grace);
            return result;
        }
    } else if grace.partitioned.is_some() {
        let partitioned = grace.partitioned.as_mut().expect("partitioned must exist");
        let result = next_partitioned(
            join_condition,
            hash_keys,
            probe_keys,
            partitioned,
            memory_tracker,
            true,
            runtime,
            output_layout,
        );
        sync_grace_counters(grace);
        return result;
    }

    while let Some(mut probe_chunk) = probe_input.advance()? {
        // Opaque consumer: expand symbolic multiplicity so each logical row
        // probes once. The selection vector is kept in place (consumed via
        // `visible_indices` below with no compaction benefit).
        let _ = probe_chunk.expand_multiplicity_in_place();
        let probe_col_names = probe_chunk.col_names();
        let mut result_rows = Vec::new();

        let combined_layout = if join_condition.is_some() {
            let fallback_width = right_col_names.len();
            let names = build_combined_names(&probe_col_names, right_col_names, fallback_width);
            Some(Arc::new(SlotLayout::from_names(&names)))
        } else {
            None
        };
        // Consume the child's selection vector (see next_hash_join). Keys
        // evaluate per row through the scalar interpreter.
        for row_idx in probe_chunk.visible_indices() {
            let probe_row = &probe_chunk.rows[row_idx];
            let probe_key = evaluate_join_key(probe_row, &probe_col_names, probe_keys)?;

            if let Some(right_indices) = build_side.matching(&probe_key) {
                if let Some((condition, layout)) =
                    join_condition.as_ref().zip(combined_layout.as_ref())
                {
                    for &right_idx in right_indices {
                        let right_row = build_side.row_at(right_idx);
                        let mut split_ctx =
                            SplitRowContext::new(probe_row, &right_row, Arc::clone(layout));
                        let satisfied =
                            match ExpressionEvaluator::evaluate(condition, &mut split_ctx) {
                                Ok(Value::Bool(b)) => b,
                                Ok(Value::Null(_)) => false,
                                Ok(_) => true,
                                Err(e) => {
                                    return Err(QueryError::execution(format!(
                                        "HashLeftJoin condition evaluation failed: {}",
                                        e
                                    )));
                                }
                            };
                        if satisfied {
                            let mut combined =
                                Vec::with_capacity(probe_row.len() + right_row.len());
                            combined.extend_from_slice(probe_row);
                            combined.extend_from_slice(&right_row);
                            result_rows.push(combined);
                        }
                    }
                } else {
                    for &right_idx in right_indices {
                        let mut joined_row =
                            Vec::with_capacity(probe_row.len() + build_side.row_count().min(16));
                        joined_row.extend_from_slice(probe_row);
                        build_side.append_row_to(&mut joined_row, right_idx);
                        result_rows.push(joined_row);
                    }
                }
            } else {
                let mut unmatched_row = probe_row.clone();
                let right_width = output_layout
                    .len()
                    .checked_sub(probe_row.len())
                    .ok_or_else(|| {
                        QueryError::execution(
                            "HashLeftJoin planned output layout is narrower than its left input"
                                .to_string(),
                        )
                    })?;
                for _ in 0..right_width {
                    unmatched_row.push(Value::Null(NullType::Null));
                }
                result_rows.push(unmatched_row);
            }
        }

        if !result_rows.is_empty() {
            return Ok(Some(finalize_join_output(
                DataChunk::new_with_layout(result_rows, Arc::clone(output_layout)),
                runtime,
            )));
        }
    }

    Ok(None)
}

pub(super) fn close(
    memory_tracker: &mut MemoryTracker,
    build_side: &mut HashJoinBuildSide,
) -> Result<(), QueryError> {
    memory_tracker.reset();
    build_side.clear();
    Ok(())
}

