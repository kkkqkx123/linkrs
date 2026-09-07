use std::sync::Arc;

use crate::executor::base::MemoryTracker;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::context::SplitRowContext;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::spec::BuildSide;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::value::NullType;
use graphdb_core::Value;

use super::grace_join::{
    hash_join_key_partition, spill_build_side, GraceJoinState, PartitionedJoinState,
    PendingBuildSpill, RotatingPartitionWriter,
};
use super::{build_combined_names, evaluate_join_key, HashJoinBuildSide};

/// Rows emitted per partitioned-serve batch.
const PARTITIONED_BATCH_ROWS: usize = 1024;

/// Drain the build input into the columnar build side.
///
/// `build_input` is the physical child selected by [`BuildSide`]: the right
/// child for the default right-build form, the left child for the
/// left-build form.
///
/// When the memory budget is exhausted and a spill manager is present, the
/// build falls back to Grace partitioning: buffered rows are re-partitioned
/// to disk and the remaining input streams straight into the spill writers.
/// The caller then spills the probe side and serves per-partition joins.
#[allow(clippy::too_many_arguments)]
fn build_side_loop(
    hash_keys: &mut [Expression],
    build_side: &mut HashJoinBuildSide,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    build_input: &mut StreamingExecutor,
    runtime: &Option<Arc<ExecutionRuntime>>,
    build_done: &mut bool,
    grace: &mut GraceJoinState,
) -> Result<(), QueryError> {
    let manager = runtime.as_ref().and_then(|rt| rt.get_spill_manager());
    while let Some(mut chunk) = build_input.advance()? {
        if let Some(rt) = runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        // The build side hashes every visible row once. `insert_chunk`
        // consumes the selection in place, so there is no benefit in
        // compacting the chunk beforehand; only the symbolic multiplicity
        // must be expanded (opaque consumer, see `normalize_for_opaque`).
        let _ = chunk.expand_multiplicity_in_place();
        let col_names = chunk.col_names();
        if right_col_names.is_empty() {
            *right_col_names = col_names.clone();
        }
        // Spill-routed path: an external `spill_with_manager` call (or an
        // earlier budget failure) opened the pending spiller; every further
        // build row streams to disk without touching the memory budget.
        if let Some(pending) = grace.pending_build.as_mut() {
            if manager.is_none() {
                return Err(QueryError::execution(
                    "spill run: spill manager not available".to_string(),
                ));
            }
            let num_partitions = pending.writer.num_partitions();
            for row in chunk.visible_rows() {
                let key = evaluate_join_key(row, &col_names, hash_keys, None)?;
                let partition = hash_join_key_partition(&key, num_partitions);
                pending.writer.insert(partition, row)?;
            }
            continue;
        }
        // Memory fast path: reserve first, then move the whole chunk.
        let mut budget_error: Option<QueryError> = None;
        for row in chunk.visible_rows() {
            if let Err(e) = memory_tracker.try_reserve_row(row) {
                budget_error = Some(e);
                break;
            }
        }
        if budget_error.is_none() {
            build_side.insert_chunk(&mut chunk, &col_names, hash_keys)?;
            continue;
        }
        // Budget exhausted: fall back to Grace partitioning when possible.
        let sm = match manager.clone() {
            Some(sm) => sm,
            None => return Err(budget_error.expect("budget error must exist")),
        };
        spill_build_side(sm, build_side, memory_tracker, right_col_names, grace)?;
        // The current chunk was only reserved (never inserted): route all of
        // its visible rows to the pending spiller. `spill_build_side` resets
        // the tracker, releasing both the drained build side and this
        // chunk's partial reservation.
        memory_tracker.reset();
        let pending = grace.pending_build.as_mut().expect("pending must exist");
        let num_partitions = pending.writer.num_partitions();
        for row in chunk.visible_rows() {
            let key = evaluate_join_key(row, &col_names, hash_keys, None)?;
            let partition = hash_join_key_partition(&key, num_partitions);
            pending.writer.insert(partition, row)?;
        }
    }
    *build_done = true;
    Ok(())
}

/// Spill the full probe input into key partitions and arm per-partition
/// serving. Takes the pending build spiller left by the build phase.
#[allow(clippy::too_many_arguments)]
fn enter_partitioned_mode(
    hash_keys: &[Expression],
    probe_keys: &[Expression],
    pending: PendingBuildSpill,
    probe_input: &mut StreamingExecutor,
    runtime: &Option<Arc<ExecutionRuntime>>,
    memory_tracker: &mut MemoryTracker,
    grace: &mut GraceJoinState,
) -> Result<(), QueryError> {
    let manager = runtime
        .as_ref()
        .and_then(|rt| rt.get_spill_manager())
        .ok_or_else(|| {
            QueryError::execution("spill run: spill manager not available".to_string())
        })?;
    let num_partitions = pending.writer.num_partitions();
    let mut probe_writer: Option<RotatingPartitionWriter> = None;
    let mut probe_names: Vec<String> = Vec::new();
    while let Some(mut chunk) = probe_input.advance()? {
        if let Some(rt) = runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        let _ = chunk.expand_multiplicity_in_place();
        let col_names = chunk.col_names();
        if probe_names.is_empty() {
            probe_names = col_names.clone();
        }
        let writer = match probe_writer.as_mut() {
            Some(writer) => writer,
            None => {
                probe_writer = Some(RotatingPartitionWriter::new(
                    manager.clone(),
                    &col_names,
                    num_partitions,
                )?);
                probe_writer.as_mut().expect("writer must exist")
            }
        };
        for row in chunk.visible_rows() {
            let key = evaluate_join_key(row, &col_names, probe_keys, None)?;
            let partition = hash_join_key_partition(&key, num_partitions);
            writer.insert(partition, row)?;
        }
    }
    let build_col_names = pending.build_col_names.clone();
    let build_runs = pending.writer.finish()?;
    let probe_runs = match probe_writer {
        Some(writer) => writer.finish()?,
        None => vec![Vec::new(); num_partitions as usize],
    };
    let mut bytes = 0u64;
    let mut runs = 0u64;
    let mut rows = 0u64;
    for run in build_runs
        .iter()
        .flatten()
        .chain(probe_runs.iter().flatten())
    {
        bytes += run.byte_size;
        runs += 1;
        rows += run.row_count;
    }
    grace.spilled_bytes += bytes;
    grace.spill_runs += runs;
    grace.spilled_rows += rows;
    if let Some(rt) = runtime.as_ref() {
        let stats = rt.columnar_stats();
        for run in build_runs
            .iter()
            .flatten()
            .chain(probe_runs.iter().flatten())
        {
            stats.record_spill(run.row_count, run.byte_size);
        }
    }
    grace.probe_col_names = probe_names.clone();
    let spill_manager = runtime.as_ref().and_then(|rt| rt.get_spill_manager());
    let mut partitioned = PartitionedJoinState::new(
        build_runs,
        probe_runs,
        build_col_names,
        probe_names,
        spill_manager,
        hash_keys.to_vec(),
        probe_keys.to_vec(),
    );
    memory_tracker.reset();
    let _ = partitioned.load_first(hash_keys, memory_tracker, runtime.as_ref())?;
    // load_first may have split oversized partitions; reconcile counters.
    grace.spilled_bytes = partitioned.byte_count();
    grace.spill_runs = partitioned.run_count();
    grace.spilled_rows = partitioned.row_count();
    grace.partitioned = Some(partitioned);
    Ok(())
}

/// Reconcile per-operator spill counters with the current partition files.
///
/// Repartition splits replace one partition with several sub-partitions;
/// syncing keeps `spilled_bytes`/`spill_runs`/`spilled_rows` exact instead
/// of stale.
fn sync_grace_counters(grace: &mut GraceJoinState) {
    if let Some(partitioned) = grace.partitioned.as_ref() {
        grace.spilled_bytes = partitioned.byte_count();
        grace.spill_runs = partitioned.run_count();
        grace.spilled_rows = partitioned.row_count();
    }
}

/// Serve one output batch from the partitioned join state.
#[allow(clippy::too_many_arguments)]
fn next_partitioned(
    join_condition: &Option<Expression>,
    hash_keys: &[Expression],
    probe_keys: &[Expression],
    state: &mut PartitionedJoinState,
    memory_tracker: &mut MemoryTracker,
    left_join: bool,
    runtime: &Option<Arc<ExecutionRuntime>>,
    output_layout: &Arc<SlotLayout>,
) -> Result<Option<DataChunk>, QueryError> {
    let mut out: Vec<Vec<Value>> = Vec::new();
    loop {
        if state.is_exhausted() {
            return if out.is_empty() {
                Ok(None)
            } else {
                Ok(Some(DataChunk::new_with_layout(
                    out,
                    Arc::clone(output_layout),
                )))
            };
        }
        while state.probe_pos() >= state.probe_rows().len() {
            if let Some(rt) = runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            if !state.advance(hash_keys, memory_tracker, runtime.as_ref())? {
                return if out.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::new_with_layout(
                        out,
                        Arc::clone(output_layout),
                    )))
                };
            }
        }
        let build_names = state.build_col_names().to_vec();
        let probe_names = state.probe_col_names().to_vec();
        let combined_layout = join_condition.as_ref().map(|_| {
            let names = build_combined_names(&probe_names, &build_names, build_names.len());
            Arc::new(SlotLayout::from_names(&names))
        });
        while state.probe_pos() < state.probe_rows().len() && out.len() < PARTITIONED_BATCH_ROWS {
            let pos = state.probe_pos();
            let probe_row = state.probe_rows()[pos].clone();
            let probe_key = evaluate_join_key(&probe_row, &probe_names, probe_keys, None)?;
            if let Some(right_indices) = state.build_side().matching(&probe_key) {
                let right_indices: Vec<u32> = right_indices.to_vec();
                if let Some((condition, layout)) =
                    join_condition.as_ref().zip(combined_layout.as_ref())
                {
                    for right_idx in right_indices {
                        let right_row = state.build_side().row_at(right_idx);
                        let mut split_ctx =
                            SplitRowContext::new(&probe_row, &right_row, Arc::clone(layout));
                        let satisfied = if left_join {
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
                            }
                        } else if let Ok(Value::Bool(b)) =
                            ExpressionEvaluator::evaluate(condition, &mut split_ctx)
                        {
                            b
                        } else {
                            false
                        };
                        if satisfied {
                            let mut combined =
                                Vec::with_capacity(probe_row.len() + right_row.len());
                            combined.extend_from_slice(&probe_row);
                            combined.extend_from_slice(&right_row);
                            out.push(combined);
                        }
                    }
                } else {
                    for right_idx in right_indices {
                        // Direct append into the output row: avoids the
                        // intermediate `Vec` allocated by `row_at()`.
                        let mut joined_row = Vec::with_capacity(
                            probe_row.len() + state.build_side().row_count().min(16),
                        );
                        joined_row.extend_from_slice(&probe_row);
                        state.build_side().append_row_to(&mut joined_row, right_idx);
                        out.push(joined_row);
                    }
                }
            } else if left_join {
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
                out.push(unmatched_row);
            }
            state.set_probe_pos(pos + 1);
        }
        if !out.is_empty() {
            return Ok(Some(DataChunk::new_with_layout(
                out,
                Arc::clone(output_layout),
            )));
        }
    }
}

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
            return Ok(Some(DataChunk::new_with_layout(
                result_rows,
                Arc::clone(output_layout),
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
