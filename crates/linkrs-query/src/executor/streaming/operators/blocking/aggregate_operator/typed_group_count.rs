//! Shared fixed-width typed group-count state for serial and partial aggregation.
//!
//! The state keeps one count per scalar group key so a typed-only chunk can be
//! consumed without rebuilding a row view. Anything outside the narrow shape
//! transfers these counts into the generic accumulator map before the row loop
//! runs, keeping one authoritative state per group.

use std::collections::{HashMap, HashSet};

use linkrs_core::columnar::RowKey;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::Value;

use crate::executor::base::{MemoryBudget, MemoryTracker};
use crate::executor::streaming::chunk::{bitmap_is_valid, DataChunk, TypedColumn};
use crate::executor::streaming::helpers::accumulator_states::AggregateAccumulator;

/// Estimated in-memory overhead of one `AggregateAccumulator` instance
/// (enum tag plus internal state) used for memory accounting. Charged once
/// per aggregate function per group so workloads with many small keys are
/// accounted for beyond the group key itself.
pub const ACCUMULATOR_OVERHEAD_BYTES: usize = 64;

/// Column emitted by a count-only expand frontier and summed by the
/// aggregate rewrite. Its values are non-negative per-chunk edge counts.
const COUNT_ONLY_COLUMN: &str = "_expand_count";

/// Which authoritative accumulator the typed counts represent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedAggregateMode {
    Count,
    SumExpandCount,
}

/// Fixed-size group keys that can be accumulated without building one
/// `Vec<Value>` per input row. Nullable and heap-backed typed columns are
/// deliberately absent: their equality needs decoding and stays on the
/// generic path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypedGroupKey {
    I64(i64),
    I32(i32),
    Bool(bool),
}

impl TypedGroupKey {
    pub fn into_value(self) -> Value {
        match self {
            Self::I64(v) => Value::BigInt(v),
            Self::I32(v) => Value::Int(v),
            Self::Bool(v) => Value::Bool(v),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedGroupKeyKind {
    I64,
    I32,
    Bool,
}

#[derive(Debug)]
pub struct TypedGroupCount {
    pub counts: HashMap<TypedGroupKey, u64>,
    pub key_kind: TypedGroupKeyKind,
    pub mode: TypedAggregateMode,
    /// One reservation per distinct group, matching the generic path's
    /// key-plus-accumulator accounting.
    pub bytes_per_group: usize,
}

fn key_kind(column: &TypedColumn) -> Option<TypedGroupKeyKind> {
    match column {
        TypedColumn::I64(_) => Some(TypedGroupKeyKind::I64),
        TypedColumn::I32(_) => Some(TypedGroupKeyKind::I32),
        TypedColumn::Bool(_) => Some(TypedGroupKeyKind::Bool),
        _ => None,
    }
}

fn key_at(column: &TypedColumn, kind: TypedGroupKeyKind, idx: usize) -> Option<TypedGroupKey> {
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

fn expand_count_values(chunk: &DataChunk) -> Option<Vec<i64>> {
    let slot = chunk.get_layout().slot_id(COUNT_ONLY_COLUMN)?;
    let TypedColumn::I64(values) = chunk.typed_column(slot)? else {
        return None;
    };
    (values.len() == chunk.eval_len()).then(|| values.clone())
}

fn group_bytes(kind: TypedGroupKeyKind, overhead: usize) -> usize {
    let sample = match kind {
        TypedGroupKeyKind::I64 => TypedGroupKey::I64(0).into_value(),
        TypedGroupKeyKind::I32 => TypedGroupKey::I32(0).into_value(),
        TypedGroupKeyKind::Bool => TypedGroupKey::Bool(false).into_value(),
    };
    MemoryBudget::estimate_row_memory(&[sample]) + overhead
}

/// NULL bitmap of a `count(variable)` argument, with its typed length.
/// `None` means every visible row contributes one to the count.
fn count_validity<'a>(
    func: &AggregateFunction,
    args: &[Expression],
    chunk: &'a DataChunk,
) -> Option<(Option<&'a [u64]>, usize)> {
    if !matches!(func, AggregateFunction::Count) {
        return None;
    }
    if args.is_empty() || super::super::helpers::count_star_arg(args) {
        return Some((None, chunk.eval_len()));
    }
    let Expression::Variable(name) = args.first()? else {
        return None;
    };
    let slot = chunk.get_layout().slot_id(name)?;
    let column = chunk.typed_column(slot)?;
    // Identity-style columns are non-null by construction; the generic path
    // has the same proof in `count_identity_arg`. Reuse it here so a typed
    // group-count state does not fall back only because the COUNT argument
    // is an edge or vertex identity rather than a scalar column.
    if matches!(
        column,
        TypedColumn::VertexIdentity(_) | TypedColumn::VertexHandle(_) | TypedColumn::EdgeHeader(_)
    ) {
        return Some((None, column.len()));
    }
    if column.is_nullable() && column.validity_bitmap().is_none() {
        return None;
    }
    Some((column.validity_bitmap(), column.len()))
}

/// Accumulate one chunk through the typed group-count path. Returns false
/// without touching `state` when the chunk does not satisfy the narrow shape,
/// so the caller can transfer any accumulated counts and process the chunk on
/// the generic path unchanged.
pub(crate) fn try_accumulate(
    state: &mut Option<TypedGroupCount>,
    group_map: &HashMap<RowKey, Vec<AggregateAccumulator>>,
    memory_tracker: &mut MemoryTracker,
    group_overhead: usize,
    has_filters: bool,
    group_by_expressions: &[Expression],
    aggregate_functions: &[(AggregateFunction, Vec<Expression>)],
    chunk: &mut DataChunk,
    multiplicity: u64,
) -> bool {
    if has_filters || !group_map.is_empty() || group_by_expressions.len() != 1 {
        return false;
    }
    let mode = match aggregate_functions {
        [(AggregateFunction::Count, _)] => TypedAggregateMode::Count,
        [(AggregateFunction::Sum, args)] if matches!(args.as_slice(), [Expression::Variable(name)] if name == COUNT_ONLY_COLUMN) => {
            TypedAggregateMode::SumExpandCount
        }
        _ => return false,
    };
    let Expression::Variable(group_name) = &group_by_expressions[0] else {
        return false;
    };
    if chunk.selection().is_some() {
        return false;
    }
    let len = chunk.eval_len();
    if len == 0 || multiplicity == 0 {
        return false;
    }

    chunk.ensure_typed_columns();
    let Some(group_slot) = chunk.get_layout().slot_id(group_name) else {
        return false;
    };
    let Some(key_column) = chunk.typed_column(group_slot) else {
        return false;
    };
    let Some(kind) = key_kind(key_column) else {
        return false;
    };
    if key_column.len() != len {
        return false;
    }
    if state
        .as_ref()
        .is_some_and(|fast| fast.key_kind != kind || fast.mode != mode)
    {
        return false;
    }
    let validity = match mode {
        TypedAggregateMode::Count => {
            let Some((validity, arg_len)) =
                count_validity(&aggregate_functions[0].0, &aggregate_functions[0].1, chunk)
            else {
                return false;
            };
            if arg_len != len {
                return false;
            }
            (validity, None)
        }
        TypedAggregateMode::SumExpandCount => {
            let Some(values) = expand_count_values(chunk) else {
                return false;
            };
            (None, Some(values))
        }
    };

    let mut new_keys = HashSet::new();
    for idx in 0..len {
        let Some(key) = key_at(key_column, kind, idx) else {
            return false;
        };
        if state
            .as_ref()
            .is_none_or(|fast| !fast.counts.contains_key(&key))
        {
            new_keys.insert(key);
        }
    }

    let bytes_per_group = state
        .as_ref()
        .map(|fast| fast.bytes_per_group)
        .unwrap_or_else(|| group_bytes(kind, group_overhead));
    if !new_keys.is_empty()
        && memory_tracker
            .try_reserve(bytes_per_group * new_keys.len())
            .is_err()
    {
        return false;
    }
    if state.is_none() {
        *state = Some(TypedGroupCount {
            counts: HashMap::new(),
            key_kind: kind,
            mode,
            bytes_per_group,
        });
    }
    let fast = state.as_mut().expect("typed group count initialized above");
    let (validity_bitmap, expand_counts) = validity;
    for idx in 0..len {
        let key = key_at(key_column, kind, idx).expect("key was validated in the preflight pass");
        let contribution = match mode {
            TypedAggregateMode::Count => {
                if validity_bitmap.is_some_and(|bitmap| !bitmap_is_valid(bitmap, idx)) {
                    0
                } else {
                    multiplicity
                }
            }
            TypedAggregateMode::SumExpandCount => {
                let value = expand_counts.as_ref().expect("sum values were validated")[idx];
                if value < 0 {
                    return false;
                }
                (value as u64).saturating_mul(multiplicity)
            }
        };
        let count = fast.counts.entry(key).or_insert(0);
        *count += contribution;
    }
    true
}

/// Accumulate keys handed directly from a packed expand. A weight folds the
/// edges sharing one unique key into one contribution, so no row or
/// typed-column materialization is needed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn accumulate_packed_keys(
    state: &mut Option<TypedGroupCount>,
    group_map: &HashMap<RowKey, Vec<AggregateAccumulator>>,
    memory_tracker: &mut MemoryTracker,
    group_overhead: usize,
    keys: &[Value],
    weights: &[u64],
    multiplicity: u64,
) -> bool {
    if !group_map.is_empty()
        || keys.is_empty()
        || (!weights.is_empty() && weights.len() != keys.len())
    {
        return false;
    }
    let mut typed_keys = Vec::with_capacity(keys.len());
    for value in keys {
        let key = match value {
            Value::BigInt(value) => TypedGroupKey::I64(*value),
            Value::Int(value) => TypedGroupKey::I32(*value),
            Value::Bool(value) => TypedGroupKey::Bool(*value),
            Value::VertexId(value) => match value.as_int64() {
                Some(value) => TypedGroupKey::I64(value),
                None => return false,
            },
            _ => return false,
        };
        typed_keys.push(key);
    }
    let kind = match typed_keys[0] {
        TypedGroupKey::I64(_) => TypedGroupKeyKind::I64,
        TypedGroupKey::I32(_) => TypedGroupKeyKind::I32,
        TypedGroupKey::Bool(_) => TypedGroupKeyKind::Bool,
    };
    if typed_keys.iter().any(|key| key_kind_of(*key) != kind) {
        return false;
    }
    if state
        .as_ref()
        .is_some_and(|fast| fast.key_kind != kind || fast.mode != TypedAggregateMode::Count)
    {
        return false;
    }
    let mut new_keys = HashSet::new();
    for key in &typed_keys {
        if state
            .as_ref()
            .is_none_or(|fast| !fast.counts.contains_key(key))
        {
            new_keys.insert(*key);
        }
    }
    let bytes_per_group = state
        .as_ref()
        .map(|fast| fast.bytes_per_group)
        .unwrap_or_else(|| group_bytes(kind, group_overhead));
    if !new_keys.is_empty()
        && memory_tracker
            .try_reserve(bytes_per_group * new_keys.len())
            .is_err()
    {
        return false;
    }
    if state.is_none() {
        *state = Some(TypedGroupCount {
            counts: HashMap::new(),
            key_kind: kind,
            mode: TypedAggregateMode::Count,
            bytes_per_group,
        });
    }
    let fast = state.as_mut().expect("typed group count initialized above");
    for (index, key) in typed_keys.into_iter().enumerate() {
        let weight = weights.get(index).copied().unwrap_or(1);
        *fast.counts.entry(key).or_insert(0) += weight.saturating_mul(multiplicity);
    }
    true
}

fn key_kind_of(key: TypedGroupKey) -> TypedGroupKeyKind {
    match key {
        TypedGroupKey::I64(_) => TypedGroupKeyKind::I64,
        TypedGroupKey::I32(_) => TypedGroupKeyKind::I32,
        TypedGroupKey::Bool(_) => TypedGroupKeyKind::Bool,
    }
}

/// Move accumulated typed counts into the generic accumulator map before a
/// chunk or a spill takes the generic path. Memory stays reserved because
/// the same groups are still live.
pub(crate) fn deopt(
    state: &mut Option<TypedGroupCount>,
    group_map: &mut HashMap<RowKey, Vec<AggregateAccumulator>>,
) {
    if let Some(fast) = state.take() {
        let mode = fast.mode;
        for (key, count) in fast.counts {
            let accumulator = match mode {
                TypedAggregateMode::Count => AggregateAccumulator::Count(count),
                TypedAggregateMode::SumExpandCount => AggregateAccumulator::Sum(count as f64),
            };
            group_map.insert(vec![key.into_value()], vec![accumulator]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::streaming::slot::SlotLayout;
    use linkrs_core::types::storage_ids::VertexId;
    use std::sync::Arc;

    fn typed_chunk(columns: Vec<TypedColumn>, names: &[&str]) -> DataChunk {
        let names: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        DataChunk::project_typed_columns(columns, Arc::new(SlotLayout::from_names(&names)))
            .expect("typed chunk")
    }

    fn group(name: &str) -> Vec<Expression> {
        vec![Expression::variable(name.to_string())]
    }

    fn count(name: &str) -> Vec<(AggregateFunction, Vec<Expression>)> {
        vec![(AggregateFunction::Count, group(name))]
    }

    fn stored_count(accs: Option<&Vec<AggregateAccumulator>>) -> Option<u64> {
        match accs.map(Vec::as_slice) {
            Some([AggregateAccumulator::Count(value)]) => Some(*value),
            _ => None,
        }
    }

    fn accumulate(
        typed: &mut Option<TypedGroupCount>,
        group_map: &HashMap<RowKey, Vec<AggregateAccumulator>>,
        tracker: &mut MemoryTracker,
        chunk: &mut DataChunk,
    ) -> bool {
        try_accumulate(
            typed,
            group_map,
            tracker,
            ACCUMULATOR_OVERHEAD_BYTES,
            false,
            &group("k"),
            &count("c"),
            chunk,
            1,
        )
    }

    #[test]
    fn fixed_width_keys_count_only_valid_arguments() {
        let mut typed = None;
        let group_map = HashMap::new();
        let mut tracker = MemoryTracker::new(MemoryBudget::new(1024 * 1024));
        let mut chunk = typed_chunk(
            vec![
                TypedColumn::I64(vec![1, 2, 1]),
                TypedColumn::NullableI64(vec![10, 20, 30], vec![0b101]),
            ],
            &["k", "c"],
        );

        assert!(accumulate(&mut typed, &group_map, &mut tracker, &mut chunk));
        let fast = typed.expect("typed state");
        assert_eq!(fast.key_kind, TypedGroupKeyKind::I64);
        assert_eq!(fast.counts.get(&TypedGroupKey::I64(1)), Some(&2));
        // The NULL argument still creates its group but does not count.
        assert_eq!(fast.counts.get(&TypedGroupKey::I64(2)), Some(&0));
        assert_eq!(fast.counts.len(), 2);
    }

    #[test]
    fn sum_expand_count_keeps_typed_state_and_deopts_to_sum() {
        let mut typed = None;
        let mut group_map: HashMap<RowKey, Vec<AggregateAccumulator>> = HashMap::new();
        let mut tracker = MemoryTracker::new(MemoryBudget::new(1024 * 1024));
        let mut chunk = typed_chunk(
            vec![
                TypedColumn::I64(vec![1, 2, 1]),
                TypedColumn::I64(vec![2, 3, 4]),
            ],
            &["k", COUNT_ONLY_COLUMN],
        );
        let funcs = vec![(
            AggregateFunction::Sum,
            vec![Expression::variable(COUNT_ONLY_COLUMN.to_string())],
        )];
        assert!(try_accumulate(
            &mut typed,
            &group_map,
            &mut tracker,
            ACCUMULATOR_OVERHEAD_BYTES,
            false,
            &group("k"),
            &funcs,
            &mut chunk,
            1,
        ));
        assert_eq!(
            typed.as_ref().expect("typed state").counts,
            HashMap::from([(TypedGroupKey::I64(1), 6), (TypedGroupKey::I64(2), 3)])
        );
        deopt(&mut typed, &mut group_map);
        assert!(matches!(
            group_map[&vec![Value::BigInt(1)]].as_slice(),
            [AggregateAccumulator::Sum(value)] if *value == 6.0
        ));
    }

    #[test]
    fn identity_count_argument_is_always_valid() {
        let id = VertexId::try_from_int64(7).expect("valid vertex id");
        let chunk = typed_chunk(
            vec![
                TypedColumn::I64(vec![1, 1]),
                TypedColumn::VertexIdentity(vec![id, id]),
            ],
            &["k", "h"],
        );
        let args = group("h");
        assert_eq!(
            count_validity(&AggregateFunction::Count, &args, &chunk),
            Some((None, 2))
        );
    }

    #[test]
    fn nullable_group_key_stays_on_generic_path() {
        let mut typed = None;
        let mut group_map: HashMap<RowKey, Vec<AggregateAccumulator>> = HashMap::new();
        let mut tracker = MemoryTracker::new(MemoryBudget::new(1024 * 1024));
        let mut chunk = typed_chunk(
            vec![
                TypedColumn::NullableI64(vec![1, 2], vec![0b10]),
                TypedColumn::I64(vec![1, 1]),
            ],
            &["k", "c"],
        );

        assert!(!accumulate(
            &mut typed,
            &group_map,
            &mut tracker,
            &mut chunk
        ));
        assert!(typed.is_none());
        deopt(&mut typed, &mut group_map);
        assert!(group_map.is_empty());
    }

    #[test]
    fn mixed_chunk_transfers_counts_once_and_stays_deoptimized() {
        let mut typed = None;
        let mut group_map: HashMap<RowKey, Vec<AggregateAccumulator>> = HashMap::new();
        let mut tracker = MemoryTracker::new(MemoryBudget::new(1024 * 1024));
        let mut first = typed_chunk(
            vec![TypedColumn::I64(vec![7, 7]), TypedColumn::I64(vec![1, 1])],
            &["k", "c"],
        );
        assert!(accumulate(&mut typed, &group_map, &mut tracker, &mut first));

        let mut mixed = typed_chunk(
            vec![
                TypedColumn::NullableI64(vec![7], vec![0b1]),
                TypedColumn::I64(vec![1]),
            ],
            &["k", "c"],
        );
        assert!(!accumulate(
            &mut typed,
            &group_map,
            &mut tracker,
            &mut mixed
        ));
        deopt(&mut typed, &mut group_map);
        assert_eq!(
            stored_count(group_map.get(&vec![Value::BigInt(7)])),
            Some(2)
        );

        let mut later = typed_chunk(
            vec![TypedColumn::I64(vec![7]), TypedColumn::I64(vec![1])],
            &["k", "c"],
        );
        assert!(!accumulate(
            &mut typed,
            &group_map,
            &mut tracker,
            &mut later
        ));
        deopt(&mut typed, &mut group_map);
        assert_eq!(group_map.len(), 1);
        assert_eq!(
            stored_count(group_map.get(&vec![Value::BigInt(7)])),
            Some(2)
        );
    }

    #[test]
    fn packed_keys_omit_uniform_weights() {
        let keys = vec![Value::BigInt(1), Value::BigInt(2)];
        let group_map = HashMap::new();
        let mut tracker = MemoryTracker::new(MemoryBudget::new(1024 * 1024));
        let mut implicit = None;
        assert!(accumulate_packed_keys(
            &mut implicit,
            &group_map,
            &mut tracker,
            ACCUMULATOR_OVERHEAD_BYTES,
            &keys,
            &[],
            3,
        ));
        let implicit = implicit.expect("implicit weights initialize typed state");
        assert_eq!(implicit.counts.get(&TypedGroupKey::I64(1)), Some(&3));
        assert_eq!(implicit.counts.get(&TypedGroupKey::I64(2)), Some(&3));

        let mut explicit = None;
        assert!(accumulate_packed_keys(
            &mut explicit,
            &group_map,
            &mut tracker,
            ACCUMULATOR_OVERHEAD_BYTES,
            &keys,
            &[2, 5],
            3,
        ));
        let explicit = explicit.expect("explicit weights initialize typed state");
        assert_eq!(explicit.counts.get(&TypedGroupKey::I64(1)), Some(&6));
        assert_eq!(explicit.counts.get(&TypedGroupKey::I64(2)), Some(&15));
    }

    #[test]
    fn symbolic_multiplicity_weights_each_key() {
        let mut typed = None;
        let group_map = HashMap::new();
        let mut tracker = MemoryTracker::new(MemoryBudget::new(1024 * 1024));
        let mut chunk = typed_chunk(
            vec![TypedColumn::I64(vec![1, 2]), TypedColumn::I64(vec![1, 1])],
            &["k", "c"],
        )
        .with_multiplicity(3);

        assert!(try_accumulate(
            &mut typed,
            &group_map,
            &mut tracker,
            ACCUMULATOR_OVERHEAD_BYTES,
            false,
            &group("k"),
            &count("c"),
            &mut chunk,
            3,
        ));
        assert_eq!(chunk.multiplicity(), 3);
        let fast = typed.expect("typed state");
        assert_eq!(fast.counts.get(&TypedGroupKey::I64(1)), Some(&3));
        assert_eq!(fast.counts.get(&TypedGroupKey::I64(2)), Some(&3));
    }

    #[test]
    fn budget_exhaustion_does_not_partially_accumulate() {
        let mut typed = None;
        let group_map = HashMap::new();
        let mut tracker = MemoryTracker::new(MemoryBudget::new(1));
        let mut chunk = typed_chunk(
            vec![TypedColumn::I64(vec![1, 1]), TypedColumn::I64(vec![1, 1])],
            &["k", "c"],
        );

        assert!(!accumulate(
            &mut typed,
            &group_map,
            &mut tracker,
            &mut chunk
        ));
        assert!(typed.is_none());
    }
}
