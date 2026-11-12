use std::collections::HashMap;

use linkrs_core::columnar::MaterializedBatch;
use linkrs_core::error::QueryError;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::value::NullType;
use linkrs_core::Value;

use crate::executor::base::MemoryTracker;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::helpers::accumulator_states::AggregateAccumulator;

use super::super::aggregate::{value_to_partial_accumulator, FinalAggregateState};
use super::super::helpers::{emit_batch_slice, BlockingContext};

pub(crate) fn open_final_aggregate(state: &mut Option<FinalAggregateState>) {
    *state = Some(FinalAggregateState {
        group_map: HashMap::new(),
        col_names: vec![],
        result_batch: MaterializedBatch::new(0, 0),
        emitted_offset: 0,
    });
}

pub(crate) fn next_final_aggregate(
    group_by_expressions: &[Expression],
    aggregate_functions: &[(AggregateFunction, Vec<Expression>)],
    memory_tracker: &mut MemoryTracker,
    state: &mut FinalAggregateState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if state.emitted_offset < state.result_batch.num_rows() {
        return Ok(Some(emit_batch_slice(
            &state.result_batch,
            &mut state.emitted_offset,
            ctx,
        )));
    }
    if state.emitted_offset > 0 {
        return Ok(None);
    }

    let mut col_names: Vec<String> = vec![];
    while let Some(mut chunk) = input.advance()? {
        if let Some(rt) = ctx.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        // Opaque consumer: expand symbolic multiplicity and compact the
        // selection so every logical row merges exactly once.
        chunk.normalize_for_opaque("Aggregate");
        if col_names.is_empty() {
            col_names = chunk.col_names();
        }
        for row in &chunk.rows {
            memory_tracker.try_reserve_row(row)?;
        }
        for row in chunk.rows {
            let num_group_keys = group_by_expressions.len();
            let num_agg_funcs = aggregate_functions.len();
            let group_key: Vec<Value> = if num_group_keys == 0 {
                vec![Value::Null(NullType::Null)]
            } else {
                row[0..num_group_keys].to_vec()
            };

            let group_accs = state.group_map.entry(group_key).or_insert_with(|| {
                aggregate_functions
                    .iter()
                    .filter_map(|(f, args)| AggregateAccumulator::for_function(f, args))
                    .collect()
            });

            for (i, func) in aggregate_functions.iter().enumerate().take(num_agg_funcs) {
                if let Some(acc) = group_accs.get_mut(i) {
                    let acc_col_idx = num_group_keys + i;
                    let partial_value = row.get(acc_col_idx);
                    if let Some(val) = partial_value {
                        let partial_acc = value_to_partial_accumulator(&func.0, &func.1, val);
                        if let Some(other) = partial_acc {
                            acc.merge(&other);
                        }
                    }
                }
            }
        }
    }

    let mut result_rows: Vec<Vec<Value>> = Vec::new();
    for (group_key, accs) in std::mem::take(&mut state.group_map) {
        let mut row = if group_by_expressions.is_empty() {
            vec![]
        } else {
            group_key
        };
        for (_i, _func) in aggregate_functions.iter().enumerate() {
            if let Some(acc) = accs.get(_i) {
                row.push(acc.finalize());
            } else {
                row.push(Value::Null(NullType::Null));
            }
        }
        result_rows.push(row);
    }

    state.result_batch.clear();
    state.emitted_offset = 0;
    for row in result_rows {
        state.result_batch.append_row(row);
    }

    if state.result_batch.is_empty() {
        Ok(None)
    } else {
        Ok(Some(emit_batch_slice(
            &state.result_batch,
            &mut state.emitted_offset,
            ctx,
        )))
    }
}

pub(crate) fn close_final_aggregate(state: &mut Option<FinalAggregateState>) {
    *state = None;
}
