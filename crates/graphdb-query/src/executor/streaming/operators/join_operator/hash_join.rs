use std::collections::HashMap;
use std::sync::Arc;

use crate::executor::base::MemoryTracker;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::{gather_typed_column, DataChunk};
use crate::executor::streaming::context::{BorrowedRowContext, SplitRowContext};
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
use super::{build_combined_names, finalize_join_output};

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
                let key = evaluate_join_key(row, &col_names, hash_keys)?;
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
            let key = evaluate_join_key(row, &col_names, hash_keys)?;
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
            let key = evaluate_join_key(row, &col_names, probe_keys)?;
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
                Ok(Some(finalize_join_output(
                    DataChunk::new_with_layout(out, Arc::clone(output_layout)),
                    runtime,
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
                    Ok(Some(finalize_join_output(
                        DataChunk::new_with_layout(out, Arc::clone(output_layout)),
                        runtime,
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
            let probe_key = evaluate_join_key(&probe_row, &probe_names, probe_keys)?;
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
            return Ok(Some(finalize_join_output(
                DataChunk::new_with_layout(out, Arc::clone(output_layout)),
                runtime,
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

/// Specialized hash join key that avoids `Vec<Value>` allocation for
/// single-column i64/string keys.
#[derive(Debug, Clone, Hash, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum JoinKeyValue {
    I32(i32),
    I64(i64),
    String(String),
    Multi(Vec<Value>),
}

impl JoinKeyValue {
    /// Whether this key is NULL (or all-NULL composite). NULL keys are pinned
    /// to Grace partition 0 so both sides agree on placement.
    pub fn is_nullish(&self) -> bool {
        match self {
            JoinKeyValue::Multi(values) => {
                !values.is_empty() && values.iter().all(|v| matches!(v, Value::Null(_)))
            }
            _ => false,
        }
    }
}

impl From<Value> for JoinKeyValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Int(i) => JoinKeyValue::I32(i),
            Value::BigInt(i) => JoinKeyValue::I64(i),
            Value::String(s) => JoinKeyValue::String(s.to_string()),
            other => JoinKeyValue::Multi(vec![other]),
        }
    }
}

fn eval_join_expr(expr: &Expression, ctx: &mut BorrowedRowContext) -> Result<Value, QueryError> {
    ExpressionEvaluator::evaluate(expr, ctx)
        .map_err(|e| QueryError::execution(format!("HashJoin key evaluation failed: {}", e)))
}

pub(super) fn evaluate_join_key(
    row: &[Value],
    col_names: &[String],
    key_expressions: &[Expression],
) -> Result<JoinKeyValue, QueryError> {
    if key_expressions.is_empty() {
        return Ok(JoinKeyValue::Multi(Vec::new()));
    }

    let layout = Arc::new(SlotLayout::from_names(col_names));
    let mut ctx = BorrowedRowContext::new(row, Arc::clone(&layout));

    if key_expressions.len() == 1 {
        let value = eval_join_expr(&key_expressions[0], &mut ctx)?;
        return Ok(JoinKeyValue::from(value));
    }

    let mut key = Vec::with_capacity(key_expressions.len());
    for expr in key_expressions {
        let value = eval_join_expr(expr, &mut ctx)?;
        key.push(value);
    }
    Ok(JoinKeyValue::Multi(key))
}

/// Columnar build side for hash joins.
///
/// Build rows are accumulated column-major (one `Vec<Value>` per input
/// column) and the hash index maps each key to its row indices. Build costs
/// a single columnar copy per value — no per-row clones — and probe reads
/// rows back by index.
#[derive(Debug)]
pub struct HashJoinBuildSide {
    /// Build-side column store (authoritative row-indexed storage).
    /// Unrelated to the removed `DataChunk.columns` lazy shim.
    build_columns: Vec<Vec<Value>>,
    index: HashMap<JoinKeyValue, Vec<u32>>,
}

impl Default for HashJoinBuildSide {
    fn default() -> Self {
        Self::new()
    }
}

impl HashJoinBuildSide {
    pub fn new() -> Self {
        Self {
            build_columns: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Append one input chunk: join keys are evaluated per visible row,
    /// visible rows move into the column store, and each visible row is
    /// indexed by its key.
    ///
    /// This is the single build-side entry point: an attached selection is
    /// consumed in place (only visible rows move), so callers must
    /// not `materialize_selection` beforehand. Values move from the typed
    /// column layout when present (gathered per visible row) and fall back
    /// to row-major clones otherwise; keys evaluate per row through the
    /// scalar interpreter.
    pub fn insert_chunk(
        &mut self,
        chunk: &mut DataChunk,
        col_names: &[String],
        key_expressions: &[Expression],
    ) -> Result<(), QueryError> {
        let visible = chunk.visible_indices();
        let num_cols = chunk.num_columns();
        debug_assert!(
            chunk.rows.iter().all(|row| row.len() == num_cols),
            "row/column count mismatch: chunk has rows without columnar data"
        );
        let base = self.row_count();
        for (pos, row_idx) in visible.iter().enumerate() {
            let row = &chunk.rows[*row_idx];
            let key = evaluate_join_key(row, col_names, key_expressions)?;
            self.index.entry(key).or_default().push((base + pos) as u32);
        }
        // Prefer the typed layout: gather visible rows per column in one pass
        // instead of cloning value-by-value out of `rows`. The typed layout is
        // consumed here, so dropping it afterwards is not a wasted build.
        let typed = chunk.typed_columns.take();
        let typed_usable = typed.as_ref().is_some_and(|cols| cols.len() == num_cols);
        if typed_usable {
            if !self.build_columns.is_empty() && num_cols != self.build_columns.len() {
                chunk.typed_columns = typed;
                return Err(QueryError::execution(format!(
                    "HashJoinBuildSide: chunk column count {} differs from build side column count {}",
                    num_cols,
                    self.build_columns.len()
                )));
            }
            let cols = typed.as_ref().expect("typed layout checked usable");
            if self.build_columns.is_empty() {
                // First chunk defines the build width; only visible rows move.
                self.build_columns = cols
                    .iter()
                    .map(|col| gather_typed_column(col, &visible).to_values())
                    .collect();
            } else {
                for (target, col) in self.build_columns.iter_mut().zip(cols.iter()) {
                    target.extend(gather_typed_column(col, &visible).to_values());
                }
            }
        } else {
            if self.build_columns.is_empty() {
                // First chunk defines the build width; only visible rows move.
                self.build_columns = (0..num_cols)
                    .map(|j| visible.iter().map(|&i| chunk.rows[i][j].clone()).collect())
                    .collect();
            } else {
                if num_cols != self.build_columns.len() {
                    chunk.typed_columns = typed;
                    return Err(QueryError::execution(format!(
                        "HashJoinBuildSide: chunk column count {} differs from build side column count {}",
                        num_cols,
                        self.build_columns.len()
                    )));
                }
                for (j, target) in self.build_columns.iter_mut().enumerate() {
                    target.extend(visible.iter().map(|&i| chunk.rows[i][j].clone()));
                }
            }
        }
        // The build chunk is fully consumed; drop rows/selection without a
        // second compaction pass. A typed layout dropped without being
        // consumed counts as a wasted build so a high build cost with no
        // reader becomes visible.
        chunk.take_selection();
        chunk.rows.clear();
        if !typed_usable && typed.is_some() {
            if let Some(stats) = &chunk.columnar_stats {
                stats.record_wasted_build();
            }
        }
        Ok(())
    }

    /// Row indices matching a probe key.
    pub fn matching(&self, key: &JoinKeyValue) -> Option<&[u32]> {
        self.index.get(key).map(|v| v.as_slice())
    }

    /// Number of rows held in the columnar build store.
    pub fn row_count(&self) -> usize {
        self.build_columns.first().map_or(0, Vec::len)
    }

    /// Append the build row at `row_idx` directly into `target`.
    ///
    /// Avoids the intermediate `Vec` allocation of [`Self::row_at`] on the
    /// hot equi-join probe path: the caller pre-reserves capacity once and
    /// extends column values in place.
    pub fn append_row_to(&self, target: &mut Vec<Value>, row_idx: u32) {
        let idx = row_idx as usize;
        target.extend(self.build_columns.iter().map(|col| col[idx].clone()));
    }

    /// Insert one fully materialized row under a precomputed key.
    ///
    /// Used when rebuilding a Grace partition from spilled rows: keys come
    /// from re-evaluation, values move straight into the column store.
    pub fn insert_keyed_row(&mut self, key: JoinKeyValue, row: &[Value]) -> Result<(), QueryError> {
        let width = self.build_columns.len();
        if width == 0 {
            self.build_columns = row.iter().map(|v| vec![v.clone()]).collect();
        } else {
            if row.len() != width {
                return Err(QueryError::execution(format!(
                    "HashJoinBuildSide: spilled row width {} differs from build width {}",
                    row.len(),
                    width,
                )));
            }
            for (target, value) in self.build_columns.iter_mut().zip(row.iter()) {
                target.push(value.clone());
            }
        }
        let idx = (self.row_count() - 1) as u32;
        self.index.entry(key).or_default().push(idx);
        Ok(())
    }

    /// Drain all indexed rows with their join keys, freeing build memory.
    ///
    /// Grace spill entry point: existing in-memory rows are re-partitioned to
    /// disk without re-evaluating key expressions.
    pub fn take_indexed_rows(&mut self) -> Vec<(JoinKeyValue, Vec<Value>)> {
        let index = std::mem::take(&mut self.index);
        let columns = std::mem::take(&mut self.build_columns);
        let row_count = columns.first().map_or(0, Vec::len);
        if row_count == 0 {
            return Vec::new();
        }
        let mut keys: Vec<Option<JoinKeyValue>> = (0..row_count).map(|_| None).collect();
        for (key, indices) in index {
            for idx in indices {
                let slot = keys
                    .get_mut(idx as usize)
                    .expect("build index out of range");
                debug_assert!(slot.is_none(), "duplicate build row index");
                *slot = Some(key.clone());
            }
        }
        let mut out = Vec::with_capacity(row_count);
        for (idx, key) in keys.into_iter().enumerate() {
            let key = key.expect("build row without a join key");
            let mut row = Vec::with_capacity(columns.len());
            for col in &columns {
                row.push(col[idx].clone());
            }
            out.push((key, row));
        }
        out
    }

    /// Materialize the row at the given index by cloning column values.
    pub fn row_at(&self, row_idx: u32) -> Vec<Value> {
        let mut out = Vec::with_capacity(self.build_columns.len());
        self.append_row_to(&mut out, row_idx);
        out
    }

    pub fn clear(&mut self) {
        self.build_columns.clear();
        self.index.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_from_columns(cols: Vec<Vec<Value>>) -> DataChunk {
        let names: Vec<String> = (0..cols.len()).map(|i| format!("c{i}")).collect();
        DataChunk::from_columns(cols, Arc::new(SlotLayout::from_names(&names)))
    }

    #[test]
    fn insert_chunk_accumulates_across_chunks() {
        let mut side = HashJoinBuildSide::new();
        let mut c1 = chunk_from_columns(vec![
            vec![Value::Int(1), Value::Int(2)],
            vec![Value::string("a"), Value::string("b")],
        ]);
        side.insert_chunk(&mut c1, &[], &[]).unwrap();
        let mut c2 = chunk_from_columns(vec![vec![Value::Int(3)], vec![Value::string("c")]]);
        side.insert_chunk(&mut c2, &[], &[]).unwrap();
        assert_eq!(side.build_columns.len(), 2);
        assert_eq!(
            side.build_columns[0],
            vec![Value::Int(1), Value::Int(2), Value::Int(3)]
        );
        assert_eq!(side.row_at(2), vec![Value::Int(3), Value::string("c")]);
        let indexed_rows: usize = side.index.values().map(|v| v.len()).sum();
        assert_eq!(indexed_rows, 3);
    }

    #[test]
    fn insert_chunk_column_count_mismatch_is_error() {
        let mut side = HashJoinBuildSide::new();
        let mut c1 = chunk_from_columns(vec![vec![Value::Int(1)], vec![Value::Int(2)]]);
        side.insert_chunk(&mut c1, &[], &[]).unwrap();
        let mut c2 = chunk_from_columns(vec![
            vec![Value::Int(3)],
            vec![Value::Int(4)],
            vec![Value::Int(5)],
        ]);
        let err = side.insert_chunk(&mut c2, &[], &[]).unwrap_err();
        assert!(err.to_string().contains("column count"));
        assert_eq!(side.build_columns.len(), 2);
        assert_eq!(side.build_columns[0], vec![Value::Int(1)]);
    }

    #[test]
    fn insert_chunk_consumes_selection_in_place() {
        // Single entry point: an attached selection is consumed without a
        // prior materialization, and only visible rows land in the store.
        let mut side = HashJoinBuildSide::new();
        let mut chunk = chunk_from_columns(vec![
            vec![Value::Int(1), Value::Int(2), Value::Int(3)],
            vec![Value::string("a"), Value::string("b"), Value::string("c")],
        ])
        .with_selection(vec![0, 2]);
        side.insert_chunk(&mut chunk, &[], &[]).unwrap();
        assert_eq!(side.row_count(), 2);
        assert_eq!(side.build_columns[0], vec![Value::Int(1), Value::Int(3)]);
        assert!(chunk.selection().is_none());
        assert!(chunk.rows.is_empty());
        let indexed_rows: usize = side.index.values().map(|v| v.len()).sum();
        assert_eq!(indexed_rows, 2);
    }

    #[test]
    #[should_panic(expected = "row/column count mismatch")]
    fn insert_chunk_rejects_schema_less_chunk() {
        // Rows carrying values that no schema column can address would be
        // silently dropped from the build side; the invariant guard must fire.
        let mut side = HashJoinBuildSide::new();
        let mut chunk = DataChunk::new_with_layout(
            vec![vec![Value::Int(1)]],
            Arc::new(SlotLayout::from_names(&[])),
        );
        let _ = side.insert_chunk(&mut chunk, &[], &[]);
    }

    #[test]
    fn insert_chunk_typed_layout_matches_row_path() {
        // Same logical chunk built twice: once with the typed layout, once
        // without. Both build paths must land byte-identical values,
        // including hidden rows skipped via selection and NULL cells.
        use graphdb_core::value::NullType;
        let cols = || {
            vec![
                vec![Value::Int(1), Value::Int(2), Value::Int(3)],
                vec![
                    Value::string("a"),
                    Value::Null(NullType::Null),
                    Value::string("c"),
                ],
            ]
        };
        let mut typed_chunk = chunk_from_columns(cols());
        typed_chunk.build_typed_columns(true);
        assert!(typed_chunk.typed_columns.is_some());
        let mut typed_chunk = typed_chunk.with_selection(vec![0, 2]);
        let mut rows_chunk = chunk_from_columns(cols()).with_selection(vec![0, 2]);

        let mut typed_side = HashJoinBuildSide::new();
        typed_side
            .insert_chunk(&mut typed_chunk, &[], &[])
            .expect("typed insert");
        let mut rows_side = HashJoinBuildSide::new();
        rows_side
            .insert_chunk(&mut rows_chunk, &[], &[])
            .expect("rows insert");

        assert_eq!(typed_side.row_count(), 2);
        assert_eq!(typed_side.row_count(), rows_side.row_count());
        for idx in 0..typed_side.row_count() as u32 {
            assert_eq!(typed_side.row_at(idx), rows_side.row_at(idx));
        }
        assert_eq!(
            typed_side.row_at(1),
            vec![Value::Int(3), Value::string("c")]
        );
        // The typed layout is consumed by the build, not dropped.
        assert!(typed_chunk.typed_columns.is_none());
    }

    #[test]
    fn insert_chunk_typed_consume_skips_wasted_build_count() {
        use std::sync::atomic::Ordering;
        // Typed layout consumed by the build: no wasted-build record.
        let stats = Arc::new(crate::executor::streaming::runtime::ColumnarStats::new());
        let mut chunk = chunk_from_columns(vec![
            vec![Value::Int(1), Value::Int(2)],
            vec![Value::string("a"), Value::string("b")],
        ])
        .with_columnar_stats(Arc::clone(&stats));
        chunk.build_typed_columns(true);
        let mut side = HashJoinBuildSide::new();
        side.insert_chunk(&mut chunk, &[], &[])
            .expect("typed insert");
        assert_eq!(stats.columnar_wasted_builds.load(Ordering::Relaxed), 0);

        // Typed layout present but unusable (width mismatch): the build falls
        // back to rows and the dropped layout still counts as wasted.
        let stats = Arc::new(crate::executor::streaming::runtime::ColumnarStats::new());
        let mut chunk = chunk_from_columns(vec![vec![Value::Int(1)], vec![Value::string("a")]])
            .with_columnar_stats(Arc::clone(&stats));
        chunk.build_typed_columns(true);
        chunk.typed_columns.as_mut().expect("typed layout").pop();
        let mut side = HashJoinBuildSide::new();
        side.insert_chunk(&mut chunk, &[], &[])
            .expect("fallback insert");
        assert_eq!(side.row_count(), 1);
        assert_eq!(stats.columnar_wasted_builds.load(Ordering::Relaxed), 1);

        // No typed layout at all: nothing to waste.
        let stats = Arc::new(crate::executor::streaming::runtime::ColumnarStats::new());
        let mut chunk =
            chunk_from_columns(vec![vec![Value::Int(1)]]).with_columnar_stats(Arc::clone(&stats));
        let mut side = HashJoinBuildSide::new();
        side.insert_chunk(&mut chunk, &[], &[])
            .expect("plain insert");
        assert_eq!(stats.columnar_wasted_builds.load(Ordering::Relaxed), 0);
    }
}
