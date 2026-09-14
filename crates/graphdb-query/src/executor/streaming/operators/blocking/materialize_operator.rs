use std::sync::Arc;

use graphdb_core::columnar::MaterializedBatch;
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::value::NullType;
use graphdb_core::Value;

use crate::executor::base::{MemoryBudget, MemoryTracker};
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::{StreamingExecutor, ValueRowContext};
use crate::executor::streaming::spill::{
    finalize_partitions_with_runtime, HashPartitionConfig, HashPartitionSpiller, RunReader,
    SpillManager, SpilledRun,
};

use super::helpers::{emit_batch_slice, BlockingContext};
use super::materialize::{
    project_key, DataCollectState, DistinctState, MaterializeState, RollUpApplyState,
};

pub(super) fn open_distinct(state: &mut Option<DistinctState>) {
    *state = Some(DistinctState {
        seen_rows: std::collections::HashSet::new(),
        key_cols: Vec::new(),
        batch: MaterializedBatch::new(0, 0),
        emitted_offset: 0,
        col_names: Vec::new(),
        input_layout: None,
        partition_spiller: None,
        spilled_runs: vec![],
        current_partition: 0,
        partition_seen: std::collections::HashSet::new(),
        has_spilled: false,
    });
}

pub(super) fn open_materialize(state: &mut Option<MaterializeState>) {
    *state = Some(MaterializeState {
        batch: MaterializedBatch::new(0, 0),
        emitted_offset: 0,
        materialized: false,
        input_layout: None,
        spilled_runs: Vec::new(),
        replay_index: 0,
        replay_reader: None,
        replay_batch: None,
        replay_offset: 0,
        accounted_bytes: 0,
    });
}

pub(super) fn open_data_collect(state: &mut Option<DataCollectState>) {
    *state = Some(DataCollectState {
        batch: MaterializedBatch::new(0, 0),
        emitted: false,
        emitted_offset: 0,
        input_layout: None,
        spilled_runs: Vec::new(),
        replay_index: 0,
        replay_reader: None,
        replay_batch: None,
        replay_offset: 0,
        accounted_bytes: 0,
    });
}

pub(super) fn open_rollup_apply(state: &mut Option<RollUpApplyState>) {
    *state = Some(RollUpApplyState {
        batch: MaterializedBatch::new(0, 0),
        emitted_offset: 0,
        accumulated: false,
        spilled_runs: Vec::new(),
        replay_index: 0,
        replay_reader: None,
        replay_batch: None,
        replay_offset: 0,
        accounted_bytes: 0,
    });
}

/// Drain a full batch to a new spill run, releasing its accounted memory.
///
/// Runs append in creation order so replay preserves input order. Empty
/// batches are skipped (the caller then fails the reserve, surfacing a
/// genuine single-row-over-budget error).
fn drain_batch_to_run(
    batch: &mut MaterializedBatch,
    accounted: &mut usize,
    runs: &mut Vec<SpilledRun>,
    sm: &SpillManager,
    memory_tracker: &mut MemoryTracker,
    ctx: Option<&BlockingContext<'_>>,
) -> Result<(), QueryError> {
    if batch.is_empty() {
        return Ok(());
    }
    let mut writer = sm.create_run_writer(batch.schema_fingerprint())?;
    writer.write_batch(batch)?;
    let run = sm.finalize_run(writer)?;
    if let Some(ctx) = ctx {
        if let Some(rt) = ctx.runtime.as_ref() {
            rt.columnar_stats()
                .record_spill(run.row_count, run.byte_size);
        }
    }
    runs.push(run);
    memory_tracker.release(*accounted);
    *accounted = 0;
    batch.clear();
    Ok(())
}

/// Serve the next chunk from ordered spill runs.
///
/// Returns `Ok(None)` when all runs are replayed; the caller then falls
/// through to its in-memory tail. Exhausted runs are unlinked eagerly.
fn replay_next_chunk(
    runs: &[SpilledRun],
    replay_index: &mut usize,
    replay_reader: &mut Option<RunReader>,
    replay_batch: &mut Option<MaterializedBatch>,
    replay_offset: &mut usize,
    take: usize,
    ctx: &BlockingContext<'_>,
) -> Result<Option<DataChunk>, QueryError> {
    loop {
        if *replay_index >= runs.len() {
            return Ok(None);
        }
        let filled = replay_batch
            .as_ref()
            .is_some_and(|b| *replay_offset < b.num_rows());
        if !filled {
            if replay_reader.is_none() {
                *replay_reader = Some(RunReader::open(&runs[*replay_index])?);
            }
            match replay_reader
                .as_mut()
                .expect("replay reader must be present")
                .read_batch(take)?
            {
                Some(b) => {
                    *replay_batch = Some(b);
                    *replay_offset = 0;
                }
                None => {
                    let _ = std::fs::remove_file(&runs[*replay_index].path);
                    *replay_reader = None;
                    *replay_batch = None;
                    *replay_offset = 0;
                    *replay_index += 1;
                    continue;
                }
            }
        }
        let (len, chunk) = {
            let batch = replay_batch.as_ref().expect("replay batch must be filled");
            let total = batch.num_rows();
            let len = take.min(total - *replay_offset);
            (
                len,
                DataChunk::slice_from_batch(
                    batch,
                    *replay_offset,
                    len,
                    Arc::clone(ctx.output_layout),
                ),
            )
        };
        *replay_offset += len;
        if let Some(rt) = ctx.runtime.as_ref() {
            rt.columnar_stats().record_batch_outlet();
        }
        return Ok(Some(chunk));
    }
}

/// Reserve memory for one row, spilling the batch on pressure.
///
/// On breach with a spill manager, the batch drains to a new run and the
/// reserve is retried once; a still-failing reserve (single row over budget)
/// and the no-manager case both surface the original budget error.
fn reserve_or_spill(
    memory_tracker: &mut MemoryTracker,
    est: usize,
    batch: &mut MaterializedBatch,
    accounted: &mut usize,
    runs: &mut Vec<SpilledRun>,
    ctx: &BlockingContext<'_>,
) -> Result<(), QueryError> {
    if let Err(e) = memory_tracker.try_reserve(est) {
        match ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager()) {
            Some(sm) => {
                if batch.is_empty() {
                    return Err(e);
                }
                drain_batch_to_run(batch, accounted, runs, &sm, memory_tracker, Some(ctx))?;
                memory_tracker.try_reserve(est).map_err(|_| e)?;
            }
            None => return Err(e),
        }
    }
    Ok(())
}

pub(super) fn next_distinct(
    memory_tracker: &mut MemoryTracker,
    state: &mut DistinctState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    // Output phase: serve slices from the output batch.
    if state.emitted_offset < state.batch.num_rows() {
        let total = state.batch.num_rows();
        let len = 2048.min(total - state.emitted_offset);
        let chunk = DataChunk::slice_from_batch(
            &state.batch,
            state.emitted_offset,
            len,
            Arc::clone(ctx.output_layout),
        );
        state.emitted_offset += len;
        if let Some(rt) = ctx.runtime.as_ref() {
            rt.columnar_stats().record_batch_outlet();
        }
        return Ok(Some(chunk));
    }

    // Replay phase
    if state.has_spilled && state.partition_spiller.is_none() {
        while state.current_partition < state.spilled_runs.len() {
            if let Some(rt) = ctx.runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }

            let run = match &state.spilled_runs[state.current_partition] {
                Some(r) => r,
                None => {
                    state.current_partition += 1;
                    continue;
                }
            };

            let mut reader = crate::executor::streaming::spill::RunReader::open(run)?;
            state.batch.clear();
            state.emitted_offset = 0;
            if !state.col_names.is_empty() {
                state
                    .batch
                    .set_schema_names(Arc::from(state.col_names.clone().into_boxed_slice()));
            }

            while let Some(row) = reader.read_row()? {
                let key = project_key(&row, &state.key_cols);
                if !state.partition_seen.contains(&key) {
                    memory_tracker.try_reserve_row(&row)?;
                    state.partition_seen.insert(key);
                    state.batch.append_row(row);
                }
            }

            let _ = std::fs::remove_file(&run.path);
            state.partition_seen.clear();
            memory_tracker.reset();
            state.current_partition += 1;

            if !state.batch.is_empty() {
                let len = 2048.min(state.batch.num_rows());
                let chunk = DataChunk::slice_from_batch(
                    &state.batch,
                    0,
                    len,
                    Arc::clone(ctx.output_layout),
                );
                state.emitted_offset = len;
                if let Some(rt) = ctx.runtime.as_ref() {
                    rt.columnar_stats().record_batch_outlet();
                }
                return Ok(Some(chunk));
            }
        }
        return Ok(None);
    }

    // Accumulation phase
    let mut accumulating = true;
    while accumulating {
        match input.advance()? {
            Some(mut chunk) => {
                chunk.normalize_for_opaque("Distinct");
                if let Some(rt) = ctx.runtime.as_ref() {
                    rt.ensure_not_cancelled()?;
                }
                if state.col_names.is_empty() {
                    state.col_names = chunk.col_names();
                    state.input_layout = Some(chunk.get_layout());
                }
                for row in chunk.rows {
                    let key = project_key(&row, &state.key_cols);
                    if !state.seen_rows.contains(&key) {
                        if let Err(e) = memory_tracker.try_reserve_row(&row) {
                            if let Some(sm) =
                                ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager())
                            {
                                // Spill drains full rows. Keyed spill would
                                // lose non-key columns, so only the identity
                                // projection may take this path.
                                debug_assert!(
                                    state.key_cols.is_empty(),
                                    "keyed distinct spill needs a row store"
                                );
                                let config = HashPartitionConfig::default();
                                let mut spiller = HashPartitionSpiller::new(config, &sm, 0)?;

                                for seen_row in state.seen_rows.drain() {
                                    spiller.insert_row(&seen_row, &sm)?;
                                    memory_tracker
                                        .release(MemoryBudget::estimate_row_memory(&seen_row));
                                }

                                spiller.insert_row(&row, &sm)?;
                                memory_tracker.release(MemoryBudget::estimate_row_memory(&row));

                                state.partition_spiller = Some(spiller);
                                state.has_spilled = true;
                                accumulating = false;
                                break;
                            } else {
                                return Err(e);
                            }
                        }
                        state.seen_rows.insert(key);
                    }
                }
            }
            None => {
                accumulating = false;
            }
        }
    }

    // Spill consumption phase
    if let Some(ref mut spiller) = state.partition_spiller {
        while let Some(mut chunk) = input.advance()? {
            chunk.normalize_for_opaque("Distinct");
            if let Some(rt) = ctx.runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            let sm = ctx
                .runtime
                .as_ref()
                .and_then(|rt| rt.get_spill_manager())
                .ok_or_else(|| QueryError::execution("Spill manager not available".to_string()))?;
            for row in chunk.rows {
                spiller.insert_row(&row, &sm)?;
            }
        }

        let runs = finalize_partitions_with_runtime(
            state.partition_spiller.take().unwrap(),
            ctx.runtime.as_ref(),
        )?;
        state.spilled_runs = runs;
        state.current_partition = 0;
        state.partition_seen.clear();

        return Ok(None);
    }

    // In-memory output phase: drain keys into the output batch, then emit.
    // With the identity projection the drained keys are the full rows.
    debug_assert!(
        state.key_cols.is_empty(),
        "keyed distinct output needs a row store"
    );
    state.batch.clear();
    state.emitted_offset = 0;
    if !state.col_names.is_empty() {
        state
            .batch
            .set_schema_names(Arc::from(state.col_names.clone().into_boxed_slice()));
    }
    for key_row in state.seen_rows.drain() {
        state.batch.append_row(key_row);
    }

    if state.batch.is_empty() {
        return Ok(None);
    }
    let len = 2048.min(state.batch.num_rows());
    let chunk = DataChunk::slice_from_batch(&state.batch, 0, len, Arc::clone(ctx.output_layout));
    state.emitted_offset = len;
    if let Some(rt) = ctx.runtime.as_ref() {
        rt.columnar_stats().record_batch_outlet();
    }
    Ok(Some(chunk))
}

pub(super) fn next_materialize(
    memory_tracker: &mut MemoryTracker,
    state: &mut MaterializeState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !state.materialized {
        while let Some(mut chunk) = input.advance()? {
            chunk.normalize_for_opaque("Materialize");
            if let Some(rt) = ctx.runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            if state.input_layout.is_none() {
                state.input_layout = Some(chunk.get_layout());
                state
                    .batch
                    .set_schema_names(Arc::from(chunk.col_names().into_boxed_slice()));
            }
            for row in chunk.rows {
                let est = MemoryBudget::estimate_row_memory(&row);
                reserve_or_spill(
                    memory_tracker,
                    est,
                    &mut state.batch,
                    &mut state.accounted_bytes,
                    &mut state.spilled_runs,
                    ctx,
                )?;
                state.accounted_bytes += est;
                state.batch.append_row(row);
            }
        }

        state.materialized = true;
        state.emitted_offset = 0;
        state.replay_index = 0;
    }

    // Spilled runs replay first (creation order = input order).
    if let Some(chunk) = replay_next_chunk(
        &state.spilled_runs,
        &mut state.replay_index,
        &mut state.replay_reader,
        &mut state.replay_batch,
        &mut state.replay_offset,
        ctx.config.chunk_size,
        ctx,
    )? {
        return Ok(Some(chunk));
    }

    // In-memory tail.
    if state.emitted_offset < state.batch.num_rows() {
        return Ok(Some(emit_batch_slice(
            &state.batch,
            &mut state.emitted_offset,
            ctx,
        )));
    }
    Ok(None)
}

pub(super) fn next_data_collect(
    memory_tracker: &mut MemoryTracker,
    state: &mut DataCollectState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if state.emitted {
        return Ok(None);
    }

    while let Some(mut chunk) = input.advance()? {
        chunk.normalize_for_opaque("DataCollect");
        if let Some(rt) = ctx.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        if state.input_layout.is_none() {
            state.input_layout = Some(chunk.get_layout());
            state
                .batch
                .set_schema_names(Arc::from(chunk.col_names().into_boxed_slice()));
        }
        for row in chunk.rows {
            let est = MemoryBudget::estimate_row_memory(&row);
            reserve_or_spill(
                memory_tracker,
                est,
                &mut state.batch,
                &mut state.accounted_bytes,
                &mut state.spilled_runs,
                ctx,
            )?;
            state.accounted_bytes += est;
            state.batch.append_row(row);
        }
    }

    // Fast path (no spill): single chunk, the established contract.
    if state.spilled_runs.is_empty() {
        if !state.batch.is_empty() {
            state.emitted = true;
            if let Some(rt) = ctx.runtime.as_ref() {
                rt.columnar_stats().record_batch_outlet();
            }
            return Ok(Some(DataChunk::from_batch(
                &state.batch,
                Arc::clone(ctx.output_layout),
            )));
        }
        return Ok(None);
    }

    // Spill path: stream runs first, then the in-memory tail.
    if let Some(chunk) = replay_next_chunk(
        &state.spilled_runs,
        &mut state.replay_index,
        &mut state.replay_reader,
        &mut state.replay_batch,
        &mut state.replay_offset,
        ctx.config.chunk_size,
        ctx,
    )? {
        return Ok(Some(chunk));
    }
    if state.emitted_offset < state.batch.num_rows() {
        return Ok(Some(emit_batch_slice(
            &state.batch,
            &mut state.emitted_offset,
            ctx,
        )));
    }
    state.emitted = true;
    Ok(None)
}

pub(super) fn next_rollup_apply(
    rollup_expressions: &[Expression],
    memory_tracker: &mut MemoryTracker,
    state: &mut RollUpApplyState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    loop {
        // Spilled runs replay first (creation order = input order).
        if let Some(chunk) = replay_next_chunk(
            &state.spilled_runs,
            &mut state.replay_index,
            &mut state.replay_reader,
            &mut state.replay_batch,
            &mut state.replay_offset,
            ctx.config.chunk_size,
            ctx,
        )? {
            return Ok(Some(chunk));
        }
        // In-memory tail.
        if state.emitted_offset < state.batch.num_rows() {
            return Ok(Some(emit_batch_slice(
                &state.batch,
                &mut state.emitted_offset,
                ctx,
            )));
        }
        if state.accumulated {
            return Ok(None);
        }

        let mut col_names: Vec<String> = Vec::new();
        let mut schema_names_set = false;
        while let Some(mut chunk) = input.advance()? {
            chunk.normalize_for_opaque("RollUpApply");
            if let Some(rt) = ctx.runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            if col_names.is_empty() {
                col_names = chunk.col_names();
            }
            if !schema_names_set {
                state
                    .batch
                    .set_schema_names(Arc::from(chunk.col_names().into_boxed_slice()));
                schema_names_set = true;
            }
            for row in chunk.rows {
                let mut ctx_eval = ValueRowContext::from_names(row.clone(), col_names.clone());
                let mut aggregated = row.clone();
                for expr in rollup_expressions.iter() {
                    match ExpressionEvaluator::evaluate(expr, &mut ctx_eval) {
                        Ok(val) => aggregated.push(val),
                        Err(_) => aggregated.push(Value::Null(NullType::Null)),
                    }
                }
                let est = MemoryBudget::estimate_row_memory(&aggregated);
                reserve_or_spill(
                    memory_tracker,
                    est,
                    &mut state.batch,
                    &mut state.accounted_bytes,
                    &mut state.spilled_runs,
                    ctx,
                )?;
                state.accounted_bytes += est;
                state.batch.append_row(aggregated);
            }
        }
        state.accumulated = true;
    }
}

pub(super) fn close_distinct(state: &mut Option<DistinctState>) {
    if let Some(ref s) = state {
        for r in s.spilled_runs.iter().flatten() {
            let _ = std::fs::remove_file(&r.path);
        }
    }
    *state = None;
}

pub(super) fn close_materialize(state: &mut Option<MaterializeState>) {
    if let Some(ref s) = state {
        for r in &s.spilled_runs {
            let _ = std::fs::remove_file(&r.path);
        }
    }
    *state = None;
}

pub(super) fn close_data_collect(state: &mut Option<DataCollectState>) {
    if let Some(ref s) = state {
        for r in &s.spilled_runs {
            let _ = std::fs::remove_file(&r.path);
        }
    }
    *state = None;
}

pub(super) fn close_rollup_apply(state: &mut Option<RollUpApplyState>) {
    if let Some(ref s) = state {
        for r in &s.spilled_runs {
            let _ = std::fs::remove_file(&r.path);
        }
    }
    *state = None;
}

pub(super) fn spill_distinct(
    state: &mut DistinctState,
    memory_tracker: &mut MemoryTracker,
    sm: &SpillManager,
) -> Result<(), QueryError> {
    if !state.seen_rows.is_empty() {
        let config = HashPartitionConfig::default();
        let mut spiller = HashPartitionSpiller::new(config, sm, 0)?;
        for row in state.seen_rows.drain() {
            spiller.insert_row(&row, sm)?;
            memory_tracker.release(MemoryBudget::estimate_row_memory(&row));
        }
        state.partition_spiller = Some(spiller);
        state.has_spilled = true;
    }
    Ok(())
}

pub(super) fn spill_materialize(
    state: &mut MaterializeState,
    sm: &SpillManager,
    memory_tracker: &mut MemoryTracker,
) -> Result<(), QueryError> {
    drain_batch_to_run(
        &mut state.batch,
        &mut state.accounted_bytes,
        &mut state.spilled_runs,
        sm,
        memory_tracker,
        None,
    )
}

pub(super) fn spill_data_collect(
    state: &mut DataCollectState,
    sm: &SpillManager,
    memory_tracker: &mut MemoryTracker,
) -> Result<(), QueryError> {
    drain_batch_to_run(
        &mut state.batch,
        &mut state.accounted_bytes,
        &mut state.spilled_runs,
        sm,
        memory_tracker,
        None,
    )
}

pub(super) fn spill_rollup_apply(
    state: &mut RollUpApplyState,
    sm: &SpillManager,
    memory_tracker: &mut MemoryTracker,
) -> Result<(), QueryError> {
    drain_batch_to_run(
        &mut state.batch,
        &mut state.accounted_bytes,
        &mut state.spilled_runs,
        sm,
        memory_tracker,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::base::MemoryBudget;
    use crate::executor::streaming::slot::SlotLayout;
    use crate::executor::streaming::spill::SpillConfig;

    fn test_ctx<'a>(
        output_layout: &'a Arc<SlotLayout>,
        config: &'a crate::executor::streaming::operators::source_operator::OperatorConfig,
    ) -> BlockingContext<'a> {
        BlockingContext {
            runtime: &None,
            output_layout,
            config,
        }
    }

    #[test]
    fn spill_replay_preserves_input_order() {
        let sm = SpillManager::new(SpillConfig::default(), 9101).expect("spill manager");
        let mut tracker = MemoryTracker::new(MemoryBudget::default_budget());
        let layout = Arc::new(SlotLayout::from_names(&["v".to_string()]));
        let config =
            crate::executor::streaming::operators::source_operator::OperatorConfig::default();
        let ctx = test_ctx(&layout, &config);

        let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::BigInt(i)]).collect();
        // Two ordered runs: [0..6) then [6..8); tail batch holds [8..10).
        let mut first = MaterializedBatch::from_rows(rows[0..6].to_vec());
        let mut accounted = 0usize;
        let mut runs = Vec::new();
        drain_batch_to_run(
            &mut first,
            &mut accounted,
            &mut runs,
            &sm,
            &mut tracker,
            Some(&ctx),
        )
        .expect("drain first");
        assert!(first.is_empty());
        let mut second = MaterializedBatch::from_rows(rows[6..8].to_vec());
        drain_batch_to_run(
            &mut second,
            &mut accounted,
            &mut runs,
            &sm,
            &mut tracker,
            Some(&ctx),
        )
        .expect("drain second");
        assert_eq!(runs.len(), 2);

        let tail = MaterializedBatch::from_rows(rows[8..10].to_vec());
        let mut replay_index = 0usize;
        let mut replay_reader = None;
        let mut replay_batch = None;
        let mut replay_offset = 0usize;
        let mut out = Vec::new();
        while let Some(chunk) = replay_next_chunk(
            &runs,
            &mut replay_index,
            &mut replay_reader,
            &mut replay_batch,
            &mut replay_offset,
            3,
            &ctx,
        )
        .expect("replay")
        {
            out.extend(chunk.rows);
        }
        out.extend(tail.to_rows());
        assert_eq!(out, rows);
        // Exhausted runs are unlinked eagerly.
        assert!(!runs[0].path.exists());
        assert!(!runs[1].path.exists());
    }

    #[test]
    fn reserve_or_spill_errors_without_manager() {
        let mut tracker = MemoryTracker::new(MemoryBudget::new(1));
        let layout = Arc::new(SlotLayout::from_names(&["v".to_string()]));
        let config =
            crate::executor::streaming::operators::source_operator::OperatorConfig::default();
        let ctx = test_ctx(&layout, &config);
        let mut batch = MaterializedBatch::new(0, 0);
        let mut accounted = 0usize;
        let mut runs = Vec::new();
        let row = vec![Value::string("way too large for a 1-byte budget")];
        let est = MemoryBudget::estimate_row_memory(&row);
        assert!(reserve_or_spill(
            &mut tracker,
            est,
            &mut batch,
            &mut accounted,
            &mut runs,
            &ctx
        )
        .is_err());
        assert!(runs.is_empty());
    }
}
