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
use crate::executor::streaming::helpers::accumulator_states::{
    accumulator_to_value, AggregateAccumulator,
};

use super::shared::eval_group_key;
use super::super::aggregate::PartialAggregateState;
use super::super::helpers::{aggregate_arg_field_name, emit_batch_slice, BlockingContext};

pub(crate) fn open_partial_aggregate(state: &mut Option<PartialAggregateState>) {
    *state = Some(PartialAggregateState {
        group_map: HashMap::new(),
        col_names: vec![],
        result_batch: MaterializedBatch::new(0, 0),
        emitted_offset: 0,
    });
}

pub(crate) fn next_partial_aggregate(
    group_by_expressions: &[Expression],
    aggregate_functions: &[(AggregateFunction, Vec<Expression>)],
    memory_tracker: &mut MemoryTracker,
    state: &mut PartialAggregateState,
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
        // Opaque consumer: expand symbolic multiplicity so each logical row
        // accumulates once. The selection vector stays in place (consumed via
        // `visible_indices` below with no compaction benefit).
        let _ = chunk.expand_multiplicity_in_place();
        if col_names.is_empty() {
            col_names = chunk.col_names();
        }
        let visible = chunk.visible_indices();
        for idx in &visible {
            memory_tracker.try_reserve_row(&chunk.rows[*idx])?;
        }
        // Vectorized group-key evaluation is a fast path over the whole
        // chunk; a chunk it cannot handle falls back to the authoritative
        // per-row helper below rather than failing the query.
        let batch_keys: Option<Vec<Vec<Value>>> =
            if chunk.selection.is_none() && !chunk.rows.is_empty() {
                chunk.evaluate_expressions(group_by_expressions, None).ok()
            } else {
                None
            };
        let field_indices: Vec<Option<usize>> = aggregate_functions
            .iter()
            .map(|(func, args)| {
                let field = aggregate_arg_field_name(func, args);
                field.and_then(|f| col_names.iter().position(|c| c == &f))
            })
            .collect();
        for idx in visible {
            let row = &chunk.rows[idx];
            let mut group_key = Vec::new();
            if group_by_expressions.is_empty() {
                group_key.push(Value::Null(NullType::Null));
            } else {
                match &batch_keys {
                    Some(keys) => {
                        group_key = keys.iter().map(|c| c[idx].clone()).collect();
                    }
                    None => {
                        group_key = eval_group_key(row, &col_names, group_by_expressions);
                    }
                }
            }

            let group_accs = state.group_map.entry(group_key).or_insert_with(|| {
                aggregate_functions
                    .iter()
                    .filter_map(|(f, args)| AggregateAccumulator::for_function(f, args))
                    .collect()
            });

            for (i, (func, _args)) in aggregate_functions.iter().enumerate() {
                if let Some(acc) = group_accs.get_mut(i) {
                    let value = match &field_indices[i] {
                        Some(j) => row
                            .get(*j)
                            .cloned()
                            .unwrap_or_else(|| Value::Null(NullType::Null)),
                        None => match func {
                            AggregateFunction::Count => Value::Int(1),
                            _ => Value::Null(NullType::Null),
                        },
                    };
                    acc.accumulate(&value);
                }
            }
        }
    }

    let mut result_rows: Vec<Vec<Value>> = Vec::new();
    let num_group_keys = group_by_expressions.len();
    for (group_key, accs) in std::mem::take(&mut state.group_map) {
        let mut row = if num_group_keys == 0 {
            vec![]
        } else {
            group_key
        };
        for (i, _func) in aggregate_functions.iter().enumerate() {
            if let Some(acc) = accs.get(i) {
                row.push(accumulator_to_value(acc));
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

pub(crate) fn close_partial_aggregate(state: &mut Option<PartialAggregateState>) {
    *state = None;
}
