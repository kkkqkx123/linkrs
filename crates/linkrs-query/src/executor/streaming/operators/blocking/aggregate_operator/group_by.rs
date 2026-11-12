use std::collections::HashMap;

use linkrs_core::columnar::MaterializedBatch;
use linkrs_core::error::QueryError;
use linkrs_core::types::expr::Expression;
use linkrs_core::value::NullType;
use linkrs_core::Value;

use crate::executor::base::{MemoryBudget, MemoryTracker};
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::{StreamingExecutor, ValueRowContext};
use crate::executor::streaming::spill::{
    finalize_partitions_with_runtime, HashPartitionConfig, HashPartitionSpiller, SpillManager,
};

use super::shared::eval_group_key;
use super::super::aggregate::GroupByState;
use super::super::helpers::{emit_batch_slice, BlockingContext};

pub(crate) fn open_groupby(state: &mut Option<GroupByState>) {
    *state = Some(GroupByState {
        batch: MaterializedBatch::new(0, 0),
        col_names: vec![],
        result_batch: MaterializedBatch::new(0, 0),
        emitted_offset: 0,
        partition_spiller: None,
        spilled_runs: vec![],
        current_partition: 0,
        has_spilled: false,
        output_complete: false,
    });
}

pub(crate) fn next_groupby(
    group_by_expressions: &[Expression],
    memory_tracker: &mut MemoryTracker,
    state: &mut GroupByState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    use crate::executor::streaming::spill::RunReader;

    let group_rows = |rows: Vec<Vec<Value>>, col_names: &[String]| -> Vec<Vec<Value>> {
        let mut groups: HashMap<String, Vec<Vec<Value>>> = HashMap::new();
        for row in rows {
            let key_parts: Vec<String> = eval_group_key(&row, col_names, group_by_expressions)
                .iter()
                .map(|v| format!("{:?}", v))
                .collect();
            let key = key_parts.join("|");
            groups.entry(key).or_default().push(row);
        }
        groups.into_values().flatten().collect()
    };

    loop {
        if state.output_complete {
            return Ok(None);
        }

        // Output phase: serve slices from the result batch.
        if state.emitted_offset < state.result_batch.num_rows() {
            return Ok(Some(emit_batch_slice(
                &state.result_batch,
                &mut state.emitted_offset,
                ctx,
            )));
        }
        if state.emitted_offset > 0 {
            state.emitted_offset = 0;
            state.result_batch.clear();
            if !state.has_spilled || state.current_partition >= state.spilled_runs.len() {
                state.output_complete = true;
                return Ok(None);
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
                let mut partition_rows = Vec::new();
                while let Some(row) = reader.read_row()? {
                    memory_tracker.try_reserve_row(&row)?;
                    partition_rows.push(row);
                }

                let col_names = if state.col_names.is_empty() {
                    (0..partition_rows.first().map_or(0, |r| r.len()))
                        .map(|i| format!("col_{}", i))
                        .collect()
                } else {
                    state.col_names.clone()
                };
                let result_rows = group_rows(partition_rows, &col_names);

                let _ = std::fs::remove_file(&run.path);
                state.current_partition += 1;
                memory_tracker.reset();

                if !result_rows.is_empty() {
                    state.result_batch.clear();
                    state.emitted_offset = 0;
                    for result_row in result_rows {
                        state.result_batch.append_row(result_row);
                    }
                    return Ok(Some(emit_batch_slice(
                        &state.result_batch,
                        &mut state.emitted_offset,
                        ctx,
                    )));
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
                    if let Some(rt) = ctx.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    // Opaque consumer: expand symbolic multiplicity and compact the
                    // selection so every logical row enters the batch exactly once.
                    chunk.normalize_for_opaque("GroupBy");
                    if state.col_names.is_empty() {
                        state.col_names = match chunk.col_names() {
                            names if !names.is_empty() => names,
                            _ => (0..chunk.rows.first().map_or(0, |r| r.len()))
                                .map(|i| format!("col_{}", i))
                                .collect(),
                        };
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
                            let group_key =
                                eval_group_key(&row, &state.col_names, group_by_expressions);
                            let p = crate::executor::streaming::spill::hash_row_partition(
                                &group_key,
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

                                for pending in state.batch.into_rows() {
                                    let group_key = eval_group_key(
                                        &pending,
                                        &state.col_names,
                                        group_by_expressions,
                                    );
                                    let p = crate::executor::streaming::spill::hash_row_partition(
                                        &group_key,
                                        num_partitions,
                                    ) as usize;
                                    spiller.insert_row_to_partition(&pending, p, &sm)?;
                                    memory_tracker
                                        .release(MemoryBudget::estimate_row_memory(&pending));
                                }

                                let group_key =
                                    eval_group_key(&row, &state.col_names, group_by_expressions);
                                let p = crate::executor::streaming::spill::hash_row_partition(
                                    &group_key,
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
                        state.batch.append_row(row);
                    }
                }
                None => {
                    accumulating = false;
                }
            }
        }

        // Finalize spilled runs
        if state.partition_spiller.is_some() {
            let runs = finalize_partitions_with_runtime(
                state.partition_spiller.take().unwrap(),
                ctx.runtime.as_ref(),
            )?;
            state.spilled_runs = runs;
            state.current_partition = 0;
            continue;
        }

        // In-memory output
        if state.batch.is_empty() {
            state.output_complete = true;
            return Ok(None);
        }
        let col_names = if state.col_names.is_empty() {
            (0..state.batch.num_columns())
                .map(|i| format!("col_{}", i))
                .collect()
        } else {
            state.col_names.clone()
        };
        let result_rows = group_rows(state.batch.into_rows(), &col_names);
        state.result_batch.clear();
        state.emitted_offset = 0;
        for result_row in result_rows {
            state.result_batch.append_row(result_row);
        }
        if state.result_batch.is_empty() {
            state.output_complete = true;
            return Ok(None);
        }
        return Ok(Some(emit_batch_slice(
            &state.result_batch,
            &mut state.emitted_offset,
            ctx,
        )));
    }
}

pub(crate) fn close_groupby(state: &mut Option<GroupByState>) {
    if let Some(ref s) = state {
        for r in s.spilled_runs.iter().flatten() {
            let _ = std::fs::remove_file(&r.path);
        }
    }
    *state = None;
}

pub(crate) fn spill_groupby(
    state: &mut GroupByState,
    memory_tracker: &mut MemoryTracker,
    group_by_expressions: &[Expression],
    sm: &SpillManager,
) -> Result<(), QueryError> {
    if state.partition_spiller.is_none() && !state.batch.is_empty() {
        let config = HashPartitionConfig::default();
        let num_partitions = config.num_partitions;
        let mut spiller = HashPartitionSpiller::new(config, sm, 0)?;
        for row in state.batch.into_rows() {
            let mut group_key = Vec::new();
            for expr in group_by_expressions.iter() {
                let mut ctx = ValueRowContext::from_names(row.clone(), state.col_names.clone());
                group_key.push(
                    ExpressionEvaluator::evaluate(expr, &mut ctx)
                        .unwrap_or(Value::Null(NullType::Null)),
                );
            }
            let p =
                crate::executor::streaming::spill::hash_row_partition(&group_key, num_partitions)
                    as usize;
            spiller.insert_row_to_partition(&row, p, sm)?;
            memory_tracker.release(MemoryBudget::estimate_row_memory(&row));
        }
        state.partition_spiller = Some(spiller);
        state.has_spilled = true;
    }
    Ok(())
}
