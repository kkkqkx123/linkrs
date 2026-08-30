use std::collections::BTreeMap;
use std::sync::Arc;

use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::value::NullType;
use graphdb_core::Value;

use crate::executor::base::{MemoryBudget, MemoryTracker};
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::{SortDirection, StreamingExecutor, ValueRowContext};
use crate::executor::streaming::spill::{HashPartitionConfig, HashPartitionSpiller, SpillManager};

use super::helpers::BlockingContext;
use super::window::{
    compute_window_partition_result, sort_partition_rows, WindowFunctionState, WindowState,
};

pub(super) fn open_window_function(state: &mut Option<WindowFunctionState>) {
    *state = Some(WindowFunctionState {
        all_rows: vec![],
        col_names: vec![],
        result_iter: None,
        spill_files: vec![],
        partition_spiller: None,
        spilled_runs: vec![],
        current_partition: 0,
        has_spilled: false,
        output_complete: false,
    });
}

pub(super) fn open_window(state: &mut Option<WindowState>) {
    *state = Some(WindowState {
        all_rows: vec![],
        col_names: vec![],
        result_iter: None,
        spill_files: vec![],
        partition_spiller: None,
        spilled_runs: vec![],
        current_partition: 0,
        has_spilled: false,
        output_complete: false,
    });
}

pub(super) fn next_window_function(
    window_exprs: &[Expression],
    partition_by_exprs: &[Expression],
    order_by_exprs: &[Expression],
    order_by_directions: &[SortDirection],
    memory_tracker: &mut MemoryTracker,
    state: &mut WindowFunctionState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    use crate::executor::streaming::spill::RunReader;

    let eval_partition_key = |row: &[Value], col_names: &[String]| -> Vec<Value> {
        if partition_by_exprs.is_empty() {
            return vec![Value::Null(NullType::Null)];
        }
        let mut key = Vec::with_capacity(partition_by_exprs.len());
        for expr in partition_by_exprs.iter() {
            let mut ctx = ValueRowContext::from_names(row.to_vec(), col_names.to_vec());
            key.push(
                ExpressionEvaluator::evaluate(expr, &mut ctx)
                    .unwrap_or(Value::Null(NullType::Null)),
            );
        }
        key
    };

    loop {
        if state.output_complete {
            return Ok(None);
        }

        // Output phase
        if let Some(ref mut iter) = state.result_iter {
            let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(2048).collect();
            if chunk_rows.is_empty() {
                state.result_iter = None;
                if !state.has_spilled || state.current_partition >= state.spilled_runs.len() {
                    state.output_complete = true;
                    return Ok(None);
                }
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

                let mut reader = RunReader::open(run)?;
                let mut partitions: BTreeMap<Vec<Value>, Vec<(usize, Vec<Value>)>> =
                    BTreeMap::new();
                let mut run_row_count: u64 = 0;
                while let Some(row) = reader.read_row()? {
                    memory_tracker.try_reserve_row(&row)?;
                    let partition_key = eval_partition_key(&row, &state.col_names);
                    partitions
                        .entry(partition_key)
                        .or_default()
                        .push((run_row_count as usize, row));
                    run_row_count += 1;
                }

                let mut result_rows = Vec::with_capacity(run_row_count as usize);
                for (_key, mut partition_rows) in partitions {
                    sort_partition_rows(
                        &mut partition_rows,
                        &state.col_names,
                        order_by_exprs,
                        order_by_directions,
                    );
                    result_rows.extend(compute_window_partition_result(
                        &partition_rows,
                        &state.col_names,
                        window_exprs,
                    ));
                }

                let _ = std::fs::remove_file(&run.path);
                state.current_partition += 1;
                memory_tracker.reset();

                if !result_rows.is_empty() {
                    state.result_iter = Some(result_rows.into_iter());
                    let chunk_rows: Vec<Vec<Value>> = state
                        .result_iter
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
                    state.result_iter = None;
                }
            }
            state.output_complete = true;
            return Ok(None);
        }

        // Accumulation phase
        let mut accumulating = true;
        while accumulating {
            match input.advance()? {
                Some(mut chunk) => {
                    chunk.materialize_selection_by("WindowFunction");
                    if let Some(rt) = ctx.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if state.col_names.is_empty() {
                        state.col_names = chunk.col_names();
                    }
                    for row in chunk.rows {
                        if let Some(ref mut spiller) = state.partition_spiller {
                            let manager = ctx
                                .runtime
                                .as_ref()
                                .and_then(|rt| rt.get_spill_manager())
                                .ok_or_else(|| {
                                    QueryError::execution("Spill manager not available".to_string())
                                })?;
                            let partition_key = eval_partition_key(&row, &state.col_names);
                            let p = crate::executor::streaming::spill::hash_row_partition(
                                &partition_key,
                                spiller.num_partitions(),
                            ) as usize;
                            spiller.insert_row_to_partition(&row, p, &manager)?;
                            continue;
                        }
                        if let Err(e) = memory_tracker.try_reserve_row(&row) {
                            if let Some(sm) =
                                ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager())
                            {
                                let config = HashPartitionConfig::default();
                                let num_partitions = config.num_partitions;
                                let mut spiller = HashPartitionSpiller::new(config, &sm, 0)?;

                                for pending in std::mem::take(&mut state.all_rows) {
                                    let partition_key =
                                        eval_partition_key(&pending, &state.col_names);
                                    let p = crate::executor::streaming::spill::hash_row_partition(
                                        &partition_key,
                                        num_partitions,
                                    ) as usize;
                                    spiller.insert_row_to_partition(&pending, p, &sm)?;
                                    memory_tracker
                                        .release(MemoryBudget::estimate_row_memory(&pending));
                                }

                                let partition_key = eval_partition_key(&row, &state.col_names);
                                let p = crate::executor::streaming::spill::hash_row_partition(
                                    &partition_key,
                                    num_partitions,
                                ) as usize;
                                spiller.insert_row_to_partition(&row, p, &sm)?;

                                state.partition_spiller = Some(spiller);
                                state.has_spilled = true;
                                continue;
                            } else {
                                return Err(e);
                            }
                        }
                        state.all_rows.push(row);
                    }
                }
                None => {
                    accumulating = false;
                }
            }
        }

        // Finalize spilled runs
        if state.partition_spiller.is_some() {
            let runs = state.partition_spiller.take().unwrap().finalize()?;
            state.spilled_runs = runs;
            state.current_partition = 0;
            continue;
        }

        // In-memory output
        if state.all_rows.is_empty() {
            state.output_complete = true;
            return Ok(None);
        }
        let mut partitions: BTreeMap<Vec<Value>, Vec<(usize, Vec<Value>)>> = BTreeMap::new();
        for (idx, row) in std::mem::take(&mut state.all_rows).into_iter().enumerate() {
            let partition_key = eval_partition_key(&row, &state.col_names);
            partitions
                .entry(partition_key)
                .or_default()
                .push((idx, row));
        }
        let mut result_rows = Vec::new();
        for (_key, mut partition_rows) in partitions {
            sort_partition_rows(
                &mut partition_rows,
                &state.col_names,
                order_by_exprs,
                order_by_directions,
            );
            result_rows.extend(compute_window_partition_result(
                &partition_rows,
                &state.col_names,
                window_exprs,
            ));
        }

        let mut result_iter = result_rows.into_iter();
        let chunk_rows: Vec<Vec<Value>> = result_iter.by_ref().take(2048).collect();
        state.result_iter = Some(result_iter);
        if chunk_rows.is_empty() {
            state.output_complete = true;
            return Ok(None);
        }
        return Ok(Some(DataChunk::new_with_layout(
            chunk_rows,
            Arc::clone(ctx.output_layout),
        )));
    }
}

pub(super) fn next_window(
    window_exprs: &[Expression],
    partition_by_exprs: &[Expression],
    order_by_exprs: &[Expression],
    order_by_directions: &[SortDirection],
    memory_tracker: &mut MemoryTracker,
    state: &mut WindowState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    use crate::executor::streaming::spill::RunReader;

    let eval_partition_key = |row: &[Value], col_names: &[String]| -> Vec<Value> {
        if partition_by_exprs.is_empty() {
            return vec![Value::Null(NullType::Null)];
        }
        let mut key = Vec::with_capacity(partition_by_exprs.len());
        for expr in partition_by_exprs.iter() {
            let mut ctx = ValueRowContext::from_names(row.to_vec(), col_names.to_vec());
            key.push(
                ExpressionEvaluator::evaluate(expr, &mut ctx)
                    .unwrap_or(Value::Null(NullType::Null)),
            );
        }
        key
    };

    loop {
        if state.output_complete {
            return Ok(None);
        }

        // Output phase
        if let Some(ref mut iter) = state.result_iter {
            let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(2048).collect();
            if chunk_rows.is_empty() {
                state.result_iter = None;
                if !state.has_spilled || state.current_partition >= state.spilled_runs.len() {
                    state.output_complete = true;
                    return Ok(None);
                }
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

                let mut reader = RunReader::open(run)?;
                let mut partitions: BTreeMap<Vec<Value>, Vec<(usize, Vec<Value>)>> =
                    BTreeMap::new();
                let mut run_row_count: u64 = 0;
                while let Some(row) = reader.read_row()? {
                    memory_tracker.try_reserve_row(&row)?;
                    let partition_key = eval_partition_key(&row, &state.col_names);
                    partitions
                        .entry(partition_key)
                        .or_default()
                        .push((run_row_count as usize, row));
                    run_row_count += 1;
                }

                let mut result_rows = Vec::with_capacity(run_row_count as usize);
                for (_key, mut partition_rows) in partitions {
                    sort_partition_rows(
                        &mut partition_rows,
                        &state.col_names,
                        order_by_exprs,
                        order_by_directions,
                    );
                    result_rows.extend(compute_window_partition_result(
                        &partition_rows,
                        &state.col_names,
                        window_exprs,
                    ));
                }

                let _ = std::fs::remove_file(&run.path);
                state.current_partition += 1;
                memory_tracker.reset();

                if !result_rows.is_empty() {
                    state.result_iter = Some(result_rows.into_iter());
                    let chunk_rows: Vec<Vec<Value>> = state
                        .result_iter
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
                    state.result_iter = None;
                }
            }
            state.output_complete = true;
            return Ok(None);
        }

        // Accumulation phase
        let mut accumulating = true;
        while accumulating {
            match input.advance()? {
                Some(mut chunk) => {
                    chunk.materialize_selection_by("Window");
                    if let Some(rt) = ctx.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if state.col_names.is_empty() {
                        state.col_names = chunk.col_names();
                    }
                    for row in chunk.rows {
                        if let Some(ref mut spiller) = state.partition_spiller {
                            let manager = ctx
                                .runtime
                                .as_ref()
                                .and_then(|rt| rt.get_spill_manager())
                                .ok_or_else(|| {
                                    QueryError::execution("Spill manager not available".to_string())
                                })?;
                            let partition_key = eval_partition_key(&row, &state.col_names);
                            let p = crate::executor::streaming::spill::hash_row_partition(
                                &partition_key,
                                spiller.num_partitions(),
                            ) as usize;
                            spiller.insert_row_to_partition(&row, p, &manager)?;
                            continue;
                        }
                        if let Err(e) = memory_tracker.try_reserve_row(&row) {
                            if let Some(sm) =
                                ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager())
                            {
                                let config = HashPartitionConfig::default();
                                let num_partitions = config.num_partitions;
                                let mut spiller = HashPartitionSpiller::new(config, &sm, 0)?;

                                for pending in std::mem::take(&mut state.all_rows) {
                                    let partition_key =
                                        eval_partition_key(&pending, &state.col_names);
                                    let p = crate::executor::streaming::spill::hash_row_partition(
                                        &partition_key,
                                        num_partitions,
                                    ) as usize;
                                    spiller.insert_row_to_partition(&pending, p, &sm)?;
                                    memory_tracker
                                        .release(MemoryBudget::estimate_row_memory(&pending));
                                }

                                let partition_key = eval_partition_key(&row, &state.col_names);
                                let p = crate::executor::streaming::spill::hash_row_partition(
                                    &partition_key,
                                    num_partitions,
                                ) as usize;
                                spiller.insert_row_to_partition(&row, p, &sm)?;

                                state.partition_spiller = Some(spiller);
                                state.has_spilled = true;
                                continue;
                            } else {
                                return Err(e);
                            }
                        }
                        state.all_rows.push(row);
                    }
                }
                None => {
                    accumulating = false;
                }
            }
        }

        // Finalize spilled runs
        if state.partition_spiller.is_some() {
            let runs = state.partition_spiller.take().unwrap().finalize()?;
            state.spilled_runs = runs;
            state.current_partition = 0;
            continue;
        }

        // In-memory output
        if state.all_rows.is_empty() {
            state.output_complete = true;
            return Ok(None);
        }
        let mut partitions: BTreeMap<Vec<Value>, Vec<(usize, Vec<Value>)>> = BTreeMap::new();
        for (idx, row) in std::mem::take(&mut state.all_rows).into_iter().enumerate() {
            let partition_key = eval_partition_key(&row, &state.col_names);
            partitions
                .entry(partition_key)
                .or_default()
                .push((idx, row));
        }
        let mut result_rows = Vec::new();
        for (_key, mut partition_rows) in partitions {
            sort_partition_rows(
                &mut partition_rows,
                &state.col_names,
                order_by_exprs,
                order_by_directions,
            );
            result_rows.extend(compute_window_partition_result(
                &partition_rows,
                &state.col_names,
                window_exprs,
            ));
        }

        let mut result_iter = result_rows.into_iter();
        let chunk_rows: Vec<Vec<Value>> = result_iter.by_ref().take(2048).collect();
        state.result_iter = Some(result_iter);
        if chunk_rows.is_empty() {
            state.output_complete = true;
            return Ok(None);
        }
        return Ok(Some(DataChunk::new_with_layout(
            chunk_rows,
            Arc::clone(ctx.output_layout),
        )));
    }
}

pub(super) fn close_window_function(state: &mut Option<WindowFunctionState>) {
    if let Some(ref s) = state {
        for r in s.spilled_runs.iter().flatten() {
            let _ = std::fs::remove_file(&r.path);
        }
    }
    *state = None;
}

pub(super) fn close_window(state: &mut Option<WindowState>) {
    if let Some(ref s) = state {
        for r in s.spilled_runs.iter().flatten() {
            let _ = std::fs::remove_file(&r.path);
        }
    }
    *state = None;
}

pub(super) fn spill_window_function(
    state: &mut WindowFunctionState,
    memory_tracker: &mut MemoryTracker,
    partition_by_exprs: &[Expression],
    sm: &SpillManager,
) -> Result<(), QueryError> {
    if state.partition_spiller.is_none() && !state.all_rows.is_empty() {
        let config = HashPartitionConfig::default();
        let num_partitions = config.num_partitions;
        let mut spiller = HashPartitionSpiller::new(config, sm, 0)?;
        for row in std::mem::take(&mut state.all_rows) {
            let mut partition_key = Vec::new();
            if partition_by_exprs.is_empty() {
                partition_key.push(Value::Null(NullType::Null));
            } else {
                for expr in partition_by_exprs.iter() {
                    let mut ctx = ValueRowContext::from_names(row.clone(), state.col_names.clone());
                    partition_key.push(
                        ExpressionEvaluator::evaluate(expr, &mut ctx)
                            .unwrap_or(Value::Null(NullType::Null)),
                    );
                }
            }
            let p = crate::executor::streaming::spill::hash_row_partition(
                &partition_key,
                num_partitions,
            ) as usize;
            spiller.insert_row_to_partition(&row, p, sm)?;
            memory_tracker.release(MemoryBudget::estimate_row_memory(&row));
        }
        state.partition_spiller = Some(spiller);
        state.has_spilled = true;
    }
    Ok(())
}

pub(super) fn spill_window(
    state: &mut WindowState,
    memory_tracker: &mut MemoryTracker,
    partition_by_exprs: &[Expression],
    sm: &SpillManager,
) -> Result<(), QueryError> {
    if state.partition_spiller.is_none() && !state.all_rows.is_empty() {
        let config = HashPartitionConfig::default();
        let num_partitions = config.num_partitions;
        let mut spiller = HashPartitionSpiller::new(config, sm, 0)?;
        for row in std::mem::take(&mut state.all_rows) {
            let mut partition_key = Vec::new();
            if partition_by_exprs.is_empty() {
                partition_key.push(Value::Null(NullType::Null));
            } else {
                for expr in partition_by_exprs.iter() {
                    let mut ctx = ValueRowContext::from_names(row.clone(), state.col_names.clone());
                    partition_key.push(
                        ExpressionEvaluator::evaluate(expr, &mut ctx)
                            .unwrap_or(Value::Null(NullType::Null)),
                    );
                }
            }
            let p = crate::executor::streaming::spill::hash_row_partition(
                &partition_key,
                num_partitions,
            ) as usize;
            spiller.insert_row_to_partition(&row, p, sm)?;
            memory_tracker.release(MemoryBudget::estimate_row_memory(&row));
        }
        state.partition_spiller = Some(spiller);
        state.has_spilled = true;
    }
    Ok(())
}
