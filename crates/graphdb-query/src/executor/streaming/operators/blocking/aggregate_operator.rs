use std::collections::HashMap;
use std::sync::Arc;

use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::operators::AggregateFunction;
use graphdb_core::value::NullType;
use graphdb_core::Value;

use crate::executor::base::{MemoryBudget, MemoryTracker};
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::{StreamingExecutor, ValueRowContext};
use crate::executor::streaming::helpers::accumulator_states::{
    accumulator_to_value, AggregateAccumulator,
};
use crate::executor::streaming::spill::{HashPartitionConfig, HashPartitionSpiller, SpillManager};

use super::aggregate::{
    value_to_partial_accumulator, AggregateState, FinalAggregateState, GroupByState,
    PartialAggregateState, ACCUMULATOR_OVERHEAD_BYTES,
};
use super::helpers::{aggregate_arg_field_name, BlockingContext};

type BatchEvalResult = Option<(Vec<Vec<Value>>, Vec<Vec<Value>>)>;

pub(super) fn open_aggregate(state: &mut Option<AggregateState>, num_agg_funcs: usize) {
    *state = Some(AggregateState {
        group_map: HashMap::new(),
        accumulator_overhead: num_agg_funcs * ACCUMULATOR_OVERHEAD_BYTES,
        result_iter: None,
        spill_files: vec![],
        partition_spiller: None,
        spilled_runs: vec![],
        current_partition: 0,
        has_spilled: false,
        output_complete: false,
        col_names: vec![],
    });
}

pub(super) fn open_groupby(state: &mut Option<GroupByState>) {
    *state = Some(GroupByState {
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

pub(super) fn open_partial_aggregate(state: &mut Option<PartialAggregateState>) {
    *state = Some(PartialAggregateState {
        group_map: HashMap::new(),
        col_names: vec![],
        result_iter: None,
        spill_files: vec![],
    });
}

pub(super) fn open_final_aggregate(state: &mut Option<FinalAggregateState>) {
    *state = Some(FinalAggregateState {
        group_map: HashMap::new(),
        col_names: vec![],
        result_iter: None,
        spill_files: vec![],
    });
}

pub(super) fn next_aggregate(
    group_by_expressions: &[Expression],
    aggregate_functions: &[(AggregateFunction, Vec<Expression>)],
    memory_tracker: &mut MemoryTracker,
    state: &mut AggregateState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let num_group_keys = group_by_expressions.len();
    let has_group_keys = num_group_keys > 0;
    let group_overhead = state.accumulator_overhead;

    let eval_group_key = |row: &[Value], col_names: &[String]| -> Vec<Value> {
        if !has_group_keys {
            return Vec::new();
        }
        let mut key = Vec::with_capacity(num_group_keys);
        for expr in group_by_expressions.iter() {
            let mut ctx = ValueRowContext::from_names(row.to_vec(), col_names.to_vec());
            match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                Ok(value) => key.push(value),
                Err(_) => key.push(Value::Null(NullType::Null)),
            }
        }
        key
    };

    loop {
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

                let mut reader = crate::executor::streaming::spill::RunReader::open(run)?;
                let mut partition_results = Vec::new();
                let mut group_map: HashMap<Vec<Value>, Vec<AggregateAccumulator>> = HashMap::new();
                while let Some(row) = reader.read_row()? {
                    let group_key: Vec<Value> = row.iter().take(num_group_keys).cloned().collect();
                    let accs = group_map.entry(group_key).or_insert_with(|| {
                        aggregate_functions
                            .iter()
                            .map(|(f, args)| {
                                AggregateAccumulator::for_function(f, args)
                                    .expect("every aggregate function has an accumulator")
                            })
                            .collect()
                    });
                    for (i, func) in aggregate_functions.iter().enumerate() {
                        if let Some(acc) = accs.get_mut(i) {
                            let partial_value = row
                                .get(num_group_keys + i)
                                .cloned()
                                .unwrap_or(Value::Null(NullType::Null));
                            if let Some(other) =
                                value_to_partial_accumulator(&func.0, &func.1, &partial_value)
                            {
                                acc.merge(&other);
                            }
                        }
                    }
                }
                for (group_key, accs) in group_map {
                    let mut result_row = if has_group_keys {
                        group_key
                    } else {
                        Vec::new()
                    };
                    for acc in accs {
                        result_row.push(acc.finalize());
                    }
                    partition_results.push(result_row);
                }

                let _ = std::fs::remove_file(&run.path);
                state.current_partition += 1;

                if !partition_results.is_empty() {
                    state.result_iter = Some(partition_results.into_iter());
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
                    if let Some(rt) = ctx.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if state.col_names.is_empty() {
                        state.col_names = chunk.col_names();
                    }
                    let sm = ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager());
                    let batch_eval: BatchEvalResult =
                        if chunk.selection.is_none() && !chunk.rows.is_empty() {
                            match chunk.evaluate_expressions(group_by_expressions, None) {
                                Ok(keys) => {
                                    let mut args = Vec::with_capacity(aggregate_functions.len());
                                    let mut ok = true;
                                    for (_func, func_args) in aggregate_functions.iter() {
                                        if let Some(expr) = func_args.first() {
                                            match chunk.evaluate_expression(expr, None) {
                                                Ok(col) => args.push(col),
                                                Err(_) => {
                                                    ok = false;
                                                    break;
                                                }
                                            }
                                        } else {
                                            ok = false;
                                            break;
                                        }
                                    }
                                    if ok {
                                        Some((keys, args))
                                    } else {
                                        None
                                    }
                                }
                                Err(_) => None,
                            }
                        } else {
                            None
                        };

                    for idx in chunk.visible_indices() {
                        let row = &chunk.rows[idx];
                        let group_key: Vec<Value> = match &batch_eval {
                            Some((keys, _)) => keys.iter().map(|c| c[idx].clone()).collect(),
                            None => eval_group_key(row, &state.col_names),
                        };
                        let arg_values: Vec<Value> = match &batch_eval {
                            Some((_, args)) => args.iter().map(|c| c[idx].clone()).collect(),
                            None => {
                                let mut values = Vec::with_capacity(aggregate_functions.len());
                                for (_func, func_args) in aggregate_functions.iter() {
                                    let mut ctx = ValueRowContext::from_names(
                                        row.to_vec(),
                                        state.col_names.clone(),
                                    );
                                    let expr = func_args
                                        .first()
                                        .cloned()
                                        .unwrap_or_else(|| Expression::Literal(Value::Int(1)));
                                    values.push(
                                        match ExpressionEvaluator::evaluate(&expr, &mut ctx) {
                                            Ok(v) => v,
                                            Err(_) => Value::Null(NullType::Null),
                                        },
                                    );
                                }
                                values
                            }
                        };
                        let partial_row_of =
                            |group_key: &[Value], arg_values: &[Value]| -> Vec<Value> {
                                let mut partial_row = group_key.to_vec();
                                for (i, (func, args)) in aggregate_functions.iter().enumerate() {
                                    let value = arg_values
                                        .get(i)
                                        .cloned()
                                        .unwrap_or_else(|| Value::Null(NullType::Null));
                                    let mut acc = AggregateAccumulator::for_function(func, args)
                                        .expect("every aggregate function has an accumulator");
                                    acc.accumulate(&value);
                                    partial_row.push(accumulator_to_value(&acc));
                                }
                                partial_row
                            };
                        if let Some(ref mut spiller) = state.partition_spiller {
                            let manager = sm.clone().ok_or_else(|| {
                                QueryError::execution("Spill manager not available".to_string())
                            })?;
                            let p = crate::executor::streaming::spill::hash_row_partition(
                                &group_key,
                                spiller.num_partitions(),
                            ) as usize;
                            let partial_row = partial_row_of(&group_key, &arg_values);
                            spiller.insert_row_to_partition(&partial_row, p, &manager)?;
                            continue;
                        }
                        if !state.group_map.contains_key(&group_key) {
                            if let Err(e) = memory_tracker.try_reserve(
                                MemoryBudget::estimate_row_memory(&group_key) + group_overhead,
                            ) {
                                if let Some(sm) =
                                    ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager())
                                {
                                    let config = HashPartitionConfig::default();
                                    let num_partitions = config.num_partitions;
                                    let mut spiller = HashPartitionSpiller::new(config, &sm, 0)?;

                                    for (key, accs) in std::mem::take(&mut state.group_map) {
                                        let p =
                                            crate::executor::streaming::spill::hash_row_partition(
                                                &key,
                                                num_partitions,
                                            ) as usize;
                                        let mut partial_row = key.clone();
                                        for acc in &accs {
                                            partial_row.push(accumulator_to_value(acc));
                                        }
                                        spiller.insert_row_to_partition(&partial_row, p, &sm)?;
                                        memory_tracker.release(
                                            MemoryBudget::estimate_row_memory(&key)
                                                + group_overhead,
                                        );
                                    }

                                    let p = crate::executor::streaming::spill::hash_row_partition(
                                        &group_key,
                                        num_partitions,
                                    ) as usize;
                                    let partial_row = partial_row_of(&group_key, &arg_values);
                                    spiller.insert_row_to_partition(&partial_row, p, &sm)?;

                                    state.partition_spiller = Some(spiller);
                                    state.has_spilled = true;
                                    continue;
                                } else {
                                    return Err(e);
                                }
                            }
                        }

                        let accs = state.group_map.entry(group_key).or_insert_with(|| {
                            aggregate_functions
                                .iter()
                                .map(|(f, args)| {
                                    AggregateAccumulator::for_function(f, args)
                                        .expect("every aggregate function has an accumulator")
                                })
                                .collect()
                        });
                        for (i, (_func, _expr)) in aggregate_functions.iter().enumerate() {
                            if let Some(acc) = accs.get_mut(i) {
                                let value = arg_values
                                    .get(i)
                                    .cloned()
                                    .unwrap_or_else(|| Value::Null(NullType::Null));
                                acc.accumulate(&value);
                            }
                        }
                    }
                }
                None => {
                    accumulating = false;
                }
            }
        }

        // Finalize spilled runs and replay them within the loop.
        if state.partition_spiller.is_some() {
            let runs = state.partition_spiller.take().unwrap().finalize()?;
            state.spilled_runs = runs;
            state.current_partition = 0;
            continue;
        }

        // In-memory output: finalize accumulated groups
        let group_map = std::mem::take(&mut state.group_map);
        let mut result_rows = Vec::new();
        for (group_key, accs) in group_map {
            let mut result_row = if has_group_keys {
                group_key
            } else {
                Vec::new()
            };
            for acc in accs {
                result_row.push(acc.finalize());
            }
            result_rows.push(result_row);
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

pub(super) fn next_groupby(
    group_by_expressions: &[Expression],
    memory_tracker: &mut MemoryTracker,
    state: &mut GroupByState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    use crate::executor::streaming::spill::RunReader;

    let eval_group_key = |row: &[Value], col_names: &[String]| -> Vec<Value> {
        let mut key = Vec::with_capacity(group_by_expressions.len());
        for expr in group_by_expressions.iter() {
            let mut ctx = ValueRowContext::from_names(row.to_vec(), col_names.to_vec());
            key.push(
                ExpressionEvaluator::evaluate(expr, &mut ctx)
                    .unwrap_or(Value::Null(NullType::Null)),
            );
        }
        key
    };

    let group_rows = |rows: Vec<Vec<Value>>, col_names: &[String]| -> Vec<Vec<Value>> {
        let mut groups: HashMap<String, Vec<Vec<Value>>> = HashMap::new();
        for row in rows {
            let key_parts: Vec<String> = eval_group_key(&row, col_names)
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
                Some(chunk) => {
                    if let Some(rt) = ctx.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
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
                            let group_key = eval_group_key(&row, &state.col_names);
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

                                for pending in std::mem::take(&mut state.all_rows) {
                                    let group_key = eval_group_key(&pending, &state.col_names);
                                    let p = crate::executor::streaming::spill::hash_row_partition(
                                        &group_key,
                                        num_partitions,
                                    ) as usize;
                                    spiller.insert_row_to_partition(&pending, p, &sm)?;
                                    memory_tracker
                                        .release(MemoryBudget::estimate_row_memory(&pending));
                                }

                                let group_key = eval_group_key(&row, &state.col_names);
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
        let col_names = if state.col_names.is_empty() {
            (0..state.all_rows[0].len())
                .map(|i| format!("col_{}", i))
                .collect()
        } else {
            state.col_names.clone()
        };
        let result_rows = group_rows(std::mem::take(&mut state.all_rows), &col_names);
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

pub(super) fn next_partial_aggregate(
    group_by_expressions: &[Expression],
    aggregate_functions: &[(AggregateFunction, Vec<Expression>)],
    memory_tracker: &mut MemoryTracker,
    state: &mut PartialAggregateState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if state.result_iter.is_some() {
        if let Some(ref mut iter) = state.result_iter {
            let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(2048).collect();
            if chunk_rows.is_empty() {
                return Ok(None);
            }
            return Ok(Some(DataChunk::new_with_layout(
                chunk_rows,
                Arc::clone(ctx.output_layout),
            )));
        }
        return Ok(None);
    }

    let mut col_names: Vec<String> = vec![];
    while let Some(mut chunk) = input.advance()? {
        if let Some(rt) = ctx.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        if col_names.is_empty() {
            col_names = chunk.col_names();
        }
        let visible = chunk.visible_indices();
        for idx in &visible {
            memory_tracker.try_reserve_row(&chunk.rows[*idx])?;
        }
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
                        for expr in group_by_expressions.iter() {
                            let mut ctx =
                                ValueRowContext::from_names(row.clone(), col_names.clone());
                            match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                Ok(value) => group_key.push(value),
                                Err(_) => group_key.push(Value::Null(NullType::Null)),
                            }
                        }
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

    state.result_iter = Some(result_rows.into_iter());

    if let Some(ref mut iter) = state.result_iter {
        let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(2048).collect();
        if chunk_rows.is_empty() {
            Ok(None)
        } else {
            Ok(Some(DataChunk::new_with_layout(
                chunk_rows,
                Arc::clone(ctx.output_layout),
            )))
        }
    } else {
        Ok(None)
    }
}

pub(super) fn next_final_aggregate(
    group_by_expressions: &[Expression],
    aggregate_functions: &[(AggregateFunction, Vec<Expression>)],
    memory_tracker: &mut MemoryTracker,
    state: &mut FinalAggregateState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if state.result_iter.is_some() {
        if let Some(ref mut iter) = state.result_iter {
            let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(2048).collect();
            if chunk_rows.is_empty() {
                return Ok(None);
            }
            return Ok(Some(DataChunk::new_with_layout(
                chunk_rows,
                Arc::clone(ctx.output_layout),
            )));
        }
        return Ok(None);
    }

    let mut col_names: Vec<String> = vec![];
    while let Some(chunk) = input.advance()? {
        if let Some(rt) = ctx.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
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

    state.result_iter = Some(result_rows.into_iter());

    if let Some(ref mut iter) = state.result_iter {
        let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(2048).collect();
        if chunk_rows.is_empty() {
            Ok(None)
        } else {
            Ok(Some(DataChunk::new_with_layout(
                chunk_rows,
                Arc::clone(ctx.output_layout),
            )))
        }
    } else {
        Ok(None)
    }
}

pub(super) fn close_aggregate(state: &mut Option<AggregateState>) {
    if let Some(ref s) = state {
        for r in s.spilled_runs.iter().flatten() {
            let _ = std::fs::remove_file(&r.path);
        }
    }
    *state = None;
}

pub(super) fn close_groupby(state: &mut Option<GroupByState>) {
    if let Some(ref s) = state {
        for r in s.spilled_runs.iter().flatten() {
            let _ = std::fs::remove_file(&r.path);
        }
    }
    *state = None;
}

pub(super) fn close_partial_aggregate(state: &mut Option<PartialAggregateState>) {
    *state = None;
}

pub(super) fn close_final_aggregate(state: &mut Option<FinalAggregateState>) {
    *state = None;
}

pub(super) fn spill_aggregate(
    state: &mut AggregateState,
    memory_tracker: &mut MemoryTracker,
    sm: &SpillManager,
) -> Result<(), QueryError> {
    if state.partition_spiller.is_none() && !state.group_map.is_empty() {
        let config = HashPartitionConfig::default();
        let num_partitions = config.num_partitions;
        let mut spiller = HashPartitionSpiller::new(config, sm, 0)?;
        for (key, accs) in std::mem::take(&mut state.group_map) {
            let p = crate::executor::streaming::spill::hash_row_partition(&key, num_partitions)
                as usize;
            let mut partial_row = key.clone();
            for acc in &accs {
                partial_row.push(accumulator_to_value(acc));
            }
            spiller.insert_row_to_partition(&partial_row, p, sm)?;
            memory_tracker
                .release(MemoryBudget::estimate_row_memory(&key) + state.accumulator_overhead);
        }
        state.partition_spiller = Some(spiller);
        state.has_spilled = true;
    }
    Ok(())
}

pub(super) fn spill_groupby(
    state: &mut GroupByState,
    memory_tracker: &mut MemoryTracker,
    group_by_expressions: &[Expression],
    sm: &SpillManager,
) -> Result<(), QueryError> {
    if state.partition_spiller.is_none() && !state.all_rows.is_empty() {
        let config = HashPartitionConfig::default();
        let num_partitions = config.num_partitions;
        let mut spiller = HashPartitionSpiller::new(config, sm, 0)?;
        for row in std::mem::take(&mut state.all_rows) {
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

pub(super) fn spill_partial_aggregate(
    state: &Option<PartialAggregateState>,
) -> Result<(), QueryError> {
    if state.as_ref().is_some_and(|s| !s.group_map.is_empty()) {
        return Err(QueryError::execution(
            "Partial aggregate spill is not implemented; query memory budget exceeded".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn spill_final_aggregate(state: &Option<FinalAggregateState>) -> Result<(), QueryError> {
    if state.as_ref().is_some_and(|s| !s.group_map.is_empty()) {
        return Err(QueryError::execution(
            "Final aggregate spill is not implemented; query memory budget exceeded".to_string(),
        ));
    }
    Ok(())
}
