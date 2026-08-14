use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::MemoryTracker;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::context::SplitRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::spec::BuildSide;
use crate::query::executor::streaming::runtime::ExecutionRuntime;
use crate::query::executor::streaming::slot::SlotLayout;

use super::{build_combined_names, evaluate_join_key, HashJoinBuildSide};

/// Drain the build input into the columnar build side.
///
/// `build_input` is the physical child selected by [`BuildSide`]: the right
/// child for the default right-build form, the left child for the
/// left-build form.
fn build_side_loop(
    hash_keys: &mut [Expression],
    build_side: &mut HashJoinBuildSide,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    build_input: &mut StreamingExecutor,
    runtime: &Option<Arc<ExecutionRuntime>>,
    build_done: &mut bool,
) -> Result<(), QueryError> {
    while let Some(mut chunk) = build_input.advance()? {
        if let Some(rt) = runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        // The build side materializes any propagated selection — it must
        // hash every (visible) build row once, so there is no benefit in
        // carrying a selection into the build store.
        chunk.materialize_selection_by("HashJoin");
        let col_names = chunk.col_names();
        if right_col_names.is_empty() {
            *right_col_names = col_names.clone();
        }
        for row in chunk.rows.iter() {
            memory_tracker.try_reserve_row(row)?;
        }
        build_side.insert_chunk(&mut chunk, &col_names, hash_keys)?;
    }
    *build_done = true;
    Ok(())
}

pub(super) fn next_hash_join(
    join_condition: &mut Option<Expression>,
    hash_keys: &mut [Expression],
    probe_keys: &mut [Expression],
    build_side: &mut HashJoinBuildSide,
    build_done: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    side: BuildSide,
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
        )?;
    }

    while let Some(mut probe_chunk) = probe_input.advance()? {
        let probe_col_names = probe_chunk.col_names();
        let mut result_rows = Vec::new();

        let combined_layout = if join_condition.is_some() {
            let fallback_width = right_col_names.len();
            let names = build_combined_names(&probe_col_names, right_col_names, fallback_width);
            Some(Arc::new(SlotLayout::from_names(&names)))
        } else {
            None
        };
        probe_chunk.materialize_columns();
        let probe_cols = probe_chunk.columns.as_deref();
        // The probe side consumes the child's selection vector — only
        // visible rows are probed, while the materialized columnar cache
        // stays valid across the Filter boundary (no re-transpose).
        for row_idx in probe_chunk.visible_indices() {
            let probe_row = &probe_chunk.rows[row_idx];
            let probe_key = evaluate_join_key(
                probe_row,
                &probe_col_names,
                probe_keys,
                probe_cols.map(|c| (c, row_idx)),
            )?;

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
                        let right_row = build_side.row_at(right_idx);
                        let mut joined_row = probe_row.clone();
                        joined_row.extend(right_row);
                        result_rows.push(joined_row);
                    }
                }
            }
        }

        if !result_rows.is_empty() {
            return Ok(Some(DataChunk::new_with_layout(
                result_rows,
                Arc::clone(output_layout),
            )));
        }
    }

    Ok(None)
}

pub(super) fn next_hash_left_join(
    join_condition: &mut Option<Expression>,
    hash_keys: &mut [Expression],
    probe_keys: &mut [Expression],
    build_side: &mut HashJoinBuildSide,
    build_done: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    side: BuildSide,
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
        )?;
    }

    while let Some(mut probe_chunk) = probe_input.advance()? {
        let probe_col_names = probe_chunk.col_names();
        let mut result_rows = Vec::new();

        let combined_layout = if join_condition.is_some() {
            let fallback_width = right_col_names.len();
            let names = build_combined_names(&probe_col_names, right_col_names, fallback_width);
            Some(Arc::new(SlotLayout::from_names(&names)))
        } else {
            None
        };
        probe_chunk.materialize_columns();
        let probe_cols = probe_chunk.columns.as_deref();
        // Consume the child's selection vector (see next_hash_join).
        for row_idx in probe_chunk.visible_indices() {
            let probe_row = &probe_chunk.rows[row_idx];
            let probe_key = evaluate_join_key(
                probe_row,
                &probe_col_names,
                probe_keys,
                probe_cols.map(|c| (c, row_idx)),
            )?;

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
                        let right_row = build_side.row_at(right_idx);
                        let mut joined_row = probe_row.clone();
                        joined_row.extend(right_row);
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
                    unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                }
                result_rows.push(unmatched_row);
            }
        }

        if !result_rows.is_empty() {
            return Ok(Some(DataChunk::new_with_layout(
                result_rows,
                Arc::clone(output_layout),
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
