use std::sync::Arc;

use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::value::NullType;
use graphdb_core::Value;

use crate::executor::base::{MemoryBudget, MemoryTracker};
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::{StreamingExecutor, ValueRowContext};
use crate::executor::streaming::spill::{HashPartitionConfig, HashPartitionSpiller, SpillManager};

use super::helpers::{BlockingContext, reject_spill_replay, spill_not_supported};
use super::materialize::{DataCollectState, DistinctState, MaterializeState, RollUpApplyState};

pub(super) fn open_distinct(state: &mut Option<DistinctState>) {
    *state = Some(DistinctState {
        seen_rows: std::collections::HashSet::new(),
        col_names: Vec::new(),
        input_layout: None,
        spill_files: vec![],
        partition_spiller: None,
        spilled_runs: vec![],
        current_partition: 0,
        partition_seen: std::collections::HashSet::new(),
        has_spilled: false,
        output_iter: None,
    });
}

pub(super) fn open_materialize(state: &mut Option<MaterializeState>) {
    *state = Some(MaterializeState {
        materialized_rows: vec![],
        result_iter: None,
        materialized: false,
        spill_files: vec![],
        input_layout: None,
    });
}

pub(super) fn open_data_collect(state: &mut Option<DataCollectState>) {
    *state = Some(DataCollectState {
        all_rows: vec![],
        emitted: false,
        spill_files: vec![],
        input_layout: None,
    });
}

pub(super) fn open_rollup_apply(state: &mut Option<RollUpApplyState>) {
    *state = Some(RollUpApplyState {
        all_rows: vec![],
        result_iter: None,
        spill_files: vec![],
    });
}

pub(super) fn next_distinct(
    memory_tracker: &mut MemoryTracker,
    state: &mut DistinctState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    // Output phase
    if let Some(ref mut iter) = state.output_iter {
        let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(2048).collect();
        if chunk_rows.is_empty() {
            state.output_iter = None;
        } else {
            return Ok(Some(DataChunk::new_with_layout(
                chunk_rows,
                Arc::clone(ctx.output_layout),
            )));
        }
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
            let mut partition_rows = Vec::new();

            while let Some(row) = reader.read_row()? {
                if !state.partition_seen.contains(&row) {
                    memory_tracker.try_reserve_row(&row)?;
                    state.partition_seen.insert(row.clone());
                    partition_rows.push(row);
                }
            }

            let _ = std::fs::remove_file(&run.path);
            state.partition_seen.clear();
            memory_tracker.reset();
            state.current_partition += 1;

            if !partition_rows.is_empty() {
                state.output_iter = Some(partition_rows.into_iter());
                let chunk_rows: Vec<Vec<Value>> = state
                    .output_iter
                    .as_mut()
                    .unwrap()
                    .by_ref()
                    .take(2048)
                    .collect();
                if !chunk_rows.is_empty() {
                    return Ok(Some(DataChunk::new_with_layout(
                        chunk_rows,
                        Arc::clone(ctx.output_layout),
                    )));
                }
                state.output_iter = None;
            }
        }
        return Ok(None);
    }

    // Accumulation phase
    let mut accumulating = true;
    while accumulating {
        match input.advance()? {
            Some(mut chunk) => {
                chunk.materialize_selection_by("Distinct");
                if let Some(rt) = ctx.runtime.as_ref() {
                    rt.ensure_not_cancelled()?;
                }
                if state.col_names.is_empty() {
                    state.col_names = chunk.col_names();
                    state.input_layout = Some(chunk.get_layout());
                }
                for row in chunk.rows {
                    if !state.seen_rows.contains(&row) {
                        if let Err(e) = memory_tracker.try_reserve_row(&row) {
                            if let Some(sm) = ctx
                                .runtime
                                .as_ref()
                                .and_then(|rt| rt.get_spill_manager())
                            {
                                let config = HashPartitionConfig::default();
                                let mut spiller = HashPartitionSpiller::new(config, &sm, 0)?;

                                for seen_row in state.seen_rows.drain() {
                                    spiller.insert_row(&seen_row, &sm)?;
                                    memory_tracker.release(
                                        MemoryBudget::estimate_row_memory(&seen_row),
                                    );
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
                        state.seen_rows.insert(row.clone());
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
            chunk.materialize_selection_by("Distinct");
            if let Some(rt) = ctx.runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            let sm = ctx
                .runtime
                .as_ref()
                .and_then(|rt| rt.get_spill_manager())
                .ok_or_else(|| {
                    QueryError::execution("Spill manager not available".to_string())
                })?;
            for row in chunk.rows {
                spiller.insert_row(&row, &sm)?;
            }
        }

        let runs = state.partition_spiller.take().unwrap().finalize()?;
        state.spilled_runs = runs;
        state.current_partition = 0;
        state.partition_seen.clear();

        return Ok(None);
    }

    // In-memory output phase
    let unique_rows: Vec<Vec<Value>> = state.seen_rows.drain().collect();
    state.output_iter = Some(unique_rows.into_iter());

    let chunk_rows: Vec<Vec<Value>> = state
        .output_iter
        .as_mut()
        .unwrap()
        .by_ref()
        .take(2048)
        .collect();
    if chunk_rows.is_empty() {
        Ok(None)
    } else {
        Ok(Some(DataChunk::new_with_layout(
            chunk_rows,
            Arc::clone(ctx.output_layout),
        )))
    }
}

pub(super) fn next_materialize(
    memory_tracker: &mut MemoryTracker,
    state: &mut MaterializeState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !state.materialized {
        while let Some(mut chunk) = input.advance()? {
            chunk.materialize_selection_by("Materialize");
            if let Some(rt) = ctx.runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            if state.input_layout.is_none() {
                state.input_layout = Some(chunk.get_layout());
            }
            for row in chunk.rows {
                if let Err(e) = memory_tracker.try_reserve_row(&row) {
                    if let Some(sm) = ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager()) {
                        spill_not_supported(
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
            return reject_spill_replay(&state.spill_files).map(|_| None);
        }

        state.materialized = true;
        state.result_iter =
            Some(std::mem::take(&mut state.materialized_rows).into_iter());
    }

    if let Some(iter) = &mut state.result_iter {
        let rows: Vec<Vec<Value>> = iter.by_ref().take(ctx.config.chunk_size).collect();
        if !rows.is_empty() {
            Ok(Some(DataChunk::new_with_layout(
                rows,
                Arc::clone(ctx.output_layout),
            )))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
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
        chunk.materialize_selection_by("DataCollect");
        if let Some(rt) = ctx.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        if state.input_layout.is_none() {
            state.input_layout = Some(chunk.get_layout());
        }
        for row in chunk.rows {
            if let Err(e) = memory_tracker.try_reserve_row(&row) {
                if let Some(sm) = ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager()) {
                    spill_not_supported(
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
        return reject_spill_replay(&state.spill_files).map(|_| None);
    }

    if !state.all_rows.is_empty() {
        state.emitted = true;
        let rows = std::mem::take(&mut state.all_rows);
        return Ok(Some(DataChunk::new_with_layout(
            rows,
            Arc::clone(ctx.output_layout),
        )));
    }

    Ok(None)
}

pub(super) fn next_rollup_apply(
    rollup_expressions: &[Expression],
    memory_tracker: &mut MemoryTracker,
    state: &mut RollUpApplyState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if state.result_iter.is_some() {
        if let Some(iter) = &mut state.result_iter {
            let rows: Vec<Vec<Value>> = iter.by_ref().take(ctx.config.chunk_size).collect();
            if !rows.is_empty() {
                return Ok(Some(DataChunk::new_with_layout(
                    rows,
                    Arc::clone(ctx.output_layout),
                )));
            }
        }
        return Ok(None);
    }

    let mut col_names: Vec<String> = Vec::new();
    while let Some(mut chunk) = input.advance()? {
        chunk.materialize_selection_by("RollUpApply");
        if let Some(rt) = ctx.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        if col_names.is_empty() {
            col_names = chunk.col_names();
        }
        for row in chunk.rows {
            if let Err(e) = memory_tracker.try_reserve_row(&row) {
                if let Some(sm) = ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager()) {
                    spill_not_supported(
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
            let mut ctx_eval = ValueRowContext::from_names(row.clone(), col_names.clone());
            let mut aggregated = row.clone();
            for expr in rollup_expressions.iter() {
                match ExpressionEvaluator::evaluate(expr, &mut ctx_eval) {
                    Ok(val) => aggregated.push(val),
                    Err(_) => aggregated.push(Value::Null(NullType::Null)),
                }
            }
            state.all_rows.push(aggregated);
        }
    }

    if !state.spill_files.is_empty() {
        return reject_spill_replay(&state.spill_files).map(|_| None);
    }

    state.result_iter = Some(std::mem::take(&mut state.all_rows).into_iter());

    if let Some(iter) = &mut state.result_iter {
        let rows: Vec<Vec<Value>> = iter.by_ref().take(ctx.config.chunk_size).collect();
        if !rows.is_empty() {
            return Ok(Some(DataChunk::new_with_layout(
                rows,
                Arc::clone(ctx.output_layout),
            )));
        }
    }

    Ok(None)
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
    *state = None;
}

pub(super) fn close_data_collect(state: &mut Option<DataCollectState>) {
    *state = None;
}

pub(super) fn close_rollup_apply(state: &mut Option<RollUpApplyState>) {
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
    spill_not_supported(
        &mut state.materialized_rows,
        sm,
        &mut state.spill_files,
        memory_tracker,
    )
}

pub(super) fn spill_data_collect(
    state: &mut DataCollectState,
    sm: &SpillManager,
    memory_tracker: &mut MemoryTracker,
) -> Result<(), QueryError> {
    spill_not_supported(&mut state.all_rows, sm, &mut state.spill_files, memory_tracker)
}

pub(super) fn spill_rollup_apply(
    state: &mut RollUpApplyState,
    sm: &SpillManager,
    memory_tracker: &mut MemoryTracker,
) -> Result<(), QueryError> {
    spill_not_supported(&mut state.all_rows, sm, &mut state.spill_files, memory_tracker)
}
