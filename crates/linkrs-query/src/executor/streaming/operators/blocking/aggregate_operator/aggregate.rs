use std::collections::{HashMap, HashSet};

use linkrs_core::columnar::{MaterializedBatch, RowKey};
use linkrs_core::error::QueryError;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::value::NullType;
use linkrs_core::Value;

use crate::executor::base::{MemoryBudget, MemoryTracker};
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::{bitmap_is_valid, DataChunk, TypedColumn};
use crate::executor::streaming::executor::{StreamingExecutor, ValueRowContext};
use crate::executor::streaming::helpers::accumulator_states::{
    accumulator_to_value, AggregateAccumulator,
};
use crate::executor::streaming::spill::{
    finalize_partitions_with_runtime, HashPartitionConfig, HashPartitionSpiller, SpillManager,
};

use super::super::aggregate::{
    value_to_partial_accumulator, AggregateState, TypedGroupCount, TypedGroupKey,
    TypedGroupKeyKind, ACCUMULATOR_OVERHEAD_BYTES,
};
use super::super::helpers::{
    count_identity_arg, count_star_arg, emit_batch_slice, BlockingContext,
};
use super::shared::eval_group_key;

type BatchEvalResult = Option<(Vec<Vec<Value>>, Vec<Vec<Value>>)>;

fn typed_key_kind(column: &TypedColumn) -> Option<TypedGroupKeyKind> {
    match column {
        TypedColumn::I64(_) => Some(TypedGroupKeyKind::I64),
        TypedColumn::I32(_) => Some(TypedGroupKeyKind::I32),
        TypedColumn::Bool(_) => Some(TypedGroupKeyKind::Bool),
        _ => None,
    }
}

fn typed_key_at(
    column: &TypedColumn,
    kind: TypedGroupKeyKind,
    idx: usize,
) -> Option<TypedGroupKey> {
    match (kind, column) {
        (TypedGroupKeyKind::I64, TypedColumn::I64(values)) => {
            values.get(idx).copied().map(TypedGroupKey::I64)
        }
        (TypedGroupKeyKind::I32, TypedColumn::I32(values)) => {
            values.get(idx).copied().map(TypedGroupKey::I32)
        }
        (TypedGroupKeyKind::Bool, TypedColumn::Bool(values)) => {
            values.get(idx).copied().map(TypedGroupKey::Bool)
        }
        _ => None,
    }
}

fn typed_group_bytes(kind: TypedGroupKeyKind, overhead: usize) -> usize {
    let sample = match kind {
        TypedGroupKeyKind::I64 => TypedGroupKey::I64(0).into_value(),
        TypedGroupKeyKind::I32 => TypedGroupKey::I32(0).into_value(),
        TypedGroupKeyKind::Bool => TypedGroupKey::Bool(false).into_value(),
    };
    MemoryBudget::estimate_row_memory(&[sample]) + overhead
}

/// NULL bitmap of a `count(variable)` argument, with its typed length.
/// `None` means every visible row contributes one to the count.
fn typed_count_validity<'a>(
    func: &AggregateFunction,
    args: &[Expression],
    chunk: &'a DataChunk,
) -> Option<(Option<&'a [u64]>, usize)> {
    if !matches!(func, AggregateFunction::Count) {
        return None;
    }
    if args.is_empty() || count_star_arg(args) {
        return Some((None, chunk.eval_len()));
    }
    let Expression::Variable(name) = args.first()? else {
        return None;
    };
    let slot = chunk.get_layout().slot_id(name)?;
    let column = chunk.typed_column(slot)?;
    if column.is_nullable() && column.validity_bitmap().is_none() {
        return None;
    }
    Some((column.validity_bitmap(), column.len()))
}

/// Accumulate one chunk through the typed group-count path. Returns false
/// without touching `state` when the chunk does not satisfy the narrow
/// shape, so the caller can process it on the generic path unchanged.
fn try_typed_group_count(
    state: &mut AggregateState,
    memory_tracker: &mut MemoryTracker,
    group_overhead: usize,
    has_filters: bool,
    group_by_expressions: &[Expression],
    aggregate_functions: &[(AggregateFunction, Vec<Expression>)],
    chunk: &mut DataChunk,
) -> bool {
    if has_filters
        || state.partition_spiller.is_some()
        || state.has_spilled
        || !state.group_map.is_empty()
        || group_by_expressions.len() != 1
        || aggregate_functions.len() != 1
    {
        return false;
    }
    let Expression::Variable(group_name) = &group_by_expressions[0] else {
        return false;
    };
    let rowless = chunk.rows.is_empty();
    if !matches!(aggregate_functions[0].0, AggregateFunction::Count)
        || chunk.selection().is_some()
        || (rowless && chunk.typed_columns.is_none())
    {
        return false;
    }
    let len = if rowless {
        chunk.typed_len().unwrap_or(0)
    } else {
        chunk.rows.len()
    };
    if len == 0 {
        return false;
    }

    chunk.ensure_typed_columns();
    let Some(group_slot) = chunk.get_layout().slot_id(group_name) else {
        return false;
    };
    let Some(key_column) = chunk.typed_column(group_slot) else {
        return false;
    };
    let Some(key_kind) = typed_key_kind(key_column) else {
        return false;
    };
    if key_column.len() != len {
        return false;
    }
    if state
        .typed_group_count
        .as_ref()
        .is_some_and(|fast| fast.key_kind != key_kind)
    {
        return false;
    }
    let Some((validity, arg_len)) =
        typed_count_validity(&aggregate_functions[0].0, &aggregate_functions[0].1, chunk)
    else {
        return false;
    };
    if arg_len != len {
        return false;
    }

    let mut new_keys = HashSet::new();
    for idx in 0..len {
        let Some(key) = typed_key_at(key_column, key_kind, idx) else {
            return false;
        };
        if state
            .typed_group_count
            .as_ref()
            .is_none_or(|fast| !fast.counts.contains_key(&key))
        {
            new_keys.insert(key);
        }
    }

    if state.typed_group_count.is_none() {
        state.typed_group_count = Some(TypedGroupCount {
            counts: HashMap::new(),
            key_kind,
            bytes_per_group: typed_group_bytes(key_kind, group_overhead),
        });
    }
    let fast = state
        .typed_group_count
        .as_mut()
        .expect("typed group count initialized above");
    if !new_keys.is_empty()
        && memory_tracker
            .try_reserve(fast.bytes_per_group * new_keys.len())
            .is_err()
    {
        return false;
    }
    for key in new_keys {
        fast.counts.insert(key, 0);
    }
    for idx in 0..len {
        let key = typed_key_at(key_column, key_kind, idx)
            .expect("key was validated in the preflight pass");
        let keep = validity.is_none_or(|bitmap| bitmap_is_valid(bitmap, idx));
        if keep {
            *fast
                .counts
                .get_mut(&key)
                .expect("key was inserted in the preflight pass") += 1;
        }
    }
    true
}

/// Move accumulated typed counts into the generic accumulator map before a
/// chunk or a spill takes the generic path. Memory stays reserved because
/// the same groups are still live.
fn deopt_typed_group_count(state: &mut AggregateState) {
    if let Some(fast) = state.typed_group_count.take() {
        for (key, count) in fast.counts {
            state.group_map.insert(
                vec![key.into_value()],
                vec![AggregateAccumulator::Count(count)],
            );
        }
    }
}

pub(crate) fn open_aggregate(state: &mut Option<AggregateState>, num_agg_funcs: usize) {
    *state = Some(AggregateState {
        group_map: HashMap::new(),
        accumulator_overhead: num_agg_funcs * ACCUMULATOR_OVERHEAD_BYTES,
        result_batch: MaterializedBatch::new(0, 0),
        emitted_offset: 0,
        partition_spiller: None,
        spilled_runs: vec![],
        current_partition: 0,
        has_spilled: false,
        output_complete: false,
        col_names: vec![],
        typed_group_count: None,
    });
}

pub(crate) fn next_aggregate(
    group_by_expressions: &[Expression],
    aggregate_functions: &[(AggregateFunction, Vec<Expression>)],
    aggregate_filters: &[Option<Expression>],
    memory_tracker: &mut MemoryTracker,
    state: &mut AggregateState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let num_group_keys = group_by_expressions.len();
    let has_group_keys = num_group_keys > 0;
    let group_overhead = state.accumulator_overhead;
    // Any per-aggregate FILTER disables the row-counting shortcuts: the
    // fast paths never evaluate predicates, so they must not run.
    let has_filters = aggregate_filters.iter().any(|f| f.is_some());

    loop {
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

                let mut reader = crate::executor::streaming::spill::RunReader::open(run)?;
                let mut partition_results = Vec::new();
                let mut group_map: HashMap<RowKey, Vec<AggregateAccumulator>> = HashMap::new();
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
                    state.result_batch.clear();
                    state.emitted_offset = 0;
                    for result_row in partition_results {
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
        // Global count shortcut: with no GROUP BY and every function a bare
        // `Count` over provably non-null inputs, the result is the visible
        // row count. Accumulate chunk lengths directly instead of hashing one
        // group key and cloning one unit value per row.
        let mut fast_counts: Vec<u64> = vec![0; aggregate_functions.len()];
        let mut fast_active = false;
        let mut accumulating = true;
        while accumulating {
            match input.advance()? {
                Some(mut chunk) => {
                    if let Some(rt) = ctx.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    // Opaque consumer: expand symbolic multiplicity so each logical row
                    // accumulates once. The selection vector stays in place (consumed via
                    // `visible_indices` below with no compaction benefit).
                    let _ = chunk.expand_multiplicity_in_place();
                    if try_typed_group_count(
                        state,
                        memory_tracker,
                        group_overhead,
                        has_filters,
                        group_by_expressions,
                        aggregate_functions,
                        &mut chunk,
                    ) {
                        continue;
                    }
                    deopt_typed_group_count(state);
                    // Per-chunk fast path flags: a `Count` over a bare
                    // variable backed by an identity column counts every
                    // visible row without evaluating its argument.
                    let count_units: Vec<bool> = aggregate_functions
                        .iter()
                        .map(|(func, func_args)| count_identity_arg(func, func_args, &chunk))
                        .collect();
                    if !has_group_keys
                        && !has_filters
                        && !aggregate_functions.is_empty()
                        && aggregate_functions
                            .iter()
                            .enumerate()
                            .all(|(i, (func, args))| {
                                matches!(func, AggregateFunction::Count)
                                    && (count_units[i] || args.is_empty() || count_star_arg(args))
                            })
                        && (chunk.selection.is_none() || !chunk.rows.is_empty())
                    {
                        let n = if chunk.rows.is_empty() {
                            chunk.typed_len().unwrap_or(0) as u64
                        } else {
                            chunk.visible_count() as u64
                        };
                        for c in fast_counts.iter_mut() {
                            *c += n;
                        }
                        fast_active = true;
                        if state.col_names.is_empty() {
                            state.col_names = chunk.col_names();
                        }
                        continue;
                    }
                    if state.col_names.is_empty() {
                        state.col_names = chunk.col_names();
                    }
                    let sm = ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager());
                    // Rowless positions: the scan skipped the row view, so the
                    // chunk carries data only in typed columns. With no group
                    // keys and every aggregate a row-counting `Count`
                    // (identity-backed, hence a unit, or argument-free), each
                    // typed position contributes one unit through the shared
                    // loop below: group keys stay empty and args stay units,
                    // so no row is ever dereferenced. Any other rowless shape
                    // is a planner-contract violation and panics loudly at
                    // the row access instead of silently dropping input.
                    let rowless_units: Option<usize> = if chunk.rows.is_empty()
                        && !has_group_keys
                        && !has_filters
                        && aggregate_functions
                            .iter()
                            .enumerate()
                            .all(|(i, (func, args))| {
                                matches!(func, AggregateFunction::Count)
                                    && (count_units[i] || args.is_empty() || count_star_arg(args))
                            }) {
                        chunk.typed_len()
                    } else {
                        None
                    };
                    let batch_eval: BatchEvalResult = if !has_filters
                        && chunk.selection.is_none()
                        && (!chunk.rows.is_empty() || rowless_units.is_some())
                    {
                        match chunk.evaluate_expressions(group_by_expressions, None) {
                            Ok(keys) => {
                                let mut args = Vec::with_capacity(aggregate_functions.len());
                                let mut ok = true;
                                for (i, (_func, func_args)) in
                                    aggregate_functions.iter().enumerate()
                                {
                                    if count_units[i] {
                                        // Identity columns are provably non-null and
                                        // `Count` only observes null-ness: feed one
                                        // unit per row without evaluating the argument.
                                        // Rowless chunks contribute one unit per
                                        // typed position.
                                        args.push(vec![
                                            Value::Int(1);
                                            rowless_units.unwrap_or(chunk.rows.len())
                                        ]);
                                        continue;
                                    }
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

                    let empty_row: Vec<Value> = Vec::new();
                    let visit: Vec<usize> = match rowless_units {
                        Some(n) if chunk.selection.is_none() => (0..n).collect(),
                        _ => chunk.visible_indices(),
                    };
                    for idx in visit {
                        let row: &[Value] = match (rowless_units, chunk.rows.get(idx)) {
                            (_, Some(stored)) => stored,
                            (Some(_), None) => &empty_row,
                            (None, None) => panic!(
                                "rowless chunk reached the row loop outside the audited count morphology"
                            ),
                        };
                        let group_key: Vec<Value> = match &batch_eval {
                            Some((keys, _)) => keys.iter().map(|c| c[idx].clone()).collect(),
                            None => eval_group_key(row, &state.col_names, group_by_expressions),
                        };
                        let arg_values: Vec<Value> = match &batch_eval {
                            Some((_, args)) => args.iter().map(|c| c[idx].clone()).collect(),
                            None => {
                                let mut values = Vec::with_capacity(aggregate_functions.len());
                                for (i, (_func, func_args)) in
                                    aggregate_functions.iter().enumerate()
                                {
                                    if count_units[i] {
                                        values.push(Value::Int(1));
                                        continue;
                                    }
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
                        // Per-row FILTER verdicts, parallel to the aggregate
                        // functions: a false entry skips accumulation for that
                        // aggregate while the group itself still exists.
                        let eval_row_filters = |row: &[Value]| -> Vec<bool> {
                            aggregate_filters
                                .iter()
                                .map(|filter| {
                                    super::shared::eval_aggregate_filter(
                                        filter,
                                        row,
                                        &state.col_names,
                                    )
                                })
                                .collect()
                        };
                        let partial_row_of = |group_key: &[Value],
                                              arg_values: &[Value],
                                              keep: &[bool]|
                         -> Vec<Value> {
                            let mut partial_row = group_key.to_vec();
                            for (i, (func, args)) in aggregate_functions.iter().enumerate() {
                                let mut acc = AggregateAccumulator::for_function(func, args)
                                    .expect("every aggregate function has an accumulator");
                                if keep[i] {
                                    let value = arg_values
                                        .get(i)
                                        .cloned()
                                        .unwrap_or_else(|| Value::Null(NullType::Null));
                                    acc.accumulate(&value);
                                }
                                partial_row.push(accumulator_to_value(&acc));
                            }
                            partial_row
                        };
                        let keep = eval_row_filters(row);
                        if let Some(ref mut spiller) = state.partition_spiller {
                            let manager = sm.clone().ok_or_else(|| {
                                QueryError::execution("Spill manager not available".to_string())
                            })?;
                            let p = crate::executor::streaming::spill::hash_row_partition(
                                &group_key,
                                spiller.num_partitions(),
                            ) as usize;
                            let partial_row = partial_row_of(&group_key, &arg_values, &keep);
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
                                    let partial_row =
                                        partial_row_of(&group_key, &arg_values, &keep);
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
                            if !keep[i] {
                                continue;
                            }
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
            let runs = finalize_partitions_with_runtime(
                state.partition_spiller.take().unwrap(),
                ctx.runtime.as_ref(),
            )?;
            state.spilled_runs = runs;
            state.current_partition = 0;
            continue;
        }

        // In-memory output: finalize accumulated groups into the result batch.
        // Fast-path counts merge into the single global group when a mixed
        // chunk forced the row loop; the pure fast path emits directly.
        deopt_typed_group_count(state);
        let mut group_map = std::mem::take(&mut state.group_map);
        state.result_batch.clear();
        state.emitted_offset = 0;
        if fast_active && !has_group_keys && state.partition_spiller.is_none() {
            if group_map.is_empty() {
                let row = fast_counts
                    .iter()
                    .map(|c| Value::BigInt(*c as i64))
                    .collect::<Vec<_>>();
                state.result_batch.append_row(row);
            } else {
                for accs in group_map.values_mut() {
                    for (acc, extra) in accs.iter_mut().zip(fast_counts.iter()) {
                        if let AggregateAccumulator::Count(c) = acc {
                            *c += *extra;
                        }
                    }
                }
                for (_group_key, accs) in group_map {
                    let mut result_row = Vec::new();
                    for acc in accs {
                        result_row.push(acc.finalize());
                    }
                    state.result_batch.append_row(result_row);
                }
            }
        } else {
            for (group_key, accs) in group_map {
                let mut result_row = if has_group_keys {
                    group_key
                } else {
                    Vec::new()
                };
                for acc in accs {
                    result_row.push(acc.finalize());
                }
                state.result_batch.append_row(result_row);
            }
        }
        // Global aggregation (no GROUP BY keys) over empty input still emits
        // a single row with default aggregate values (e.g. count = 0).
        // (Empty input never spills, so the spilled path needs no equivalent.)
        if !has_group_keys && state.result_batch.is_empty() {
            let mut result_row = Vec::new();
            for (func, args) in aggregate_functions.iter() {
                let acc = AggregateAccumulator::for_function(func, args)
                    .expect("every aggregate function has an accumulator");
                result_row.push(acc.finalize());
            }
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

pub(crate) fn close_aggregate(state: &mut Option<AggregateState>) {
    if let Some(ref s) = state {
        for r in s.spilled_runs.iter().flatten() {
            let _ = std::fs::remove_file(&r.path);
        }
    }
    *state = None;
}

pub(crate) fn spill_aggregate(
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
