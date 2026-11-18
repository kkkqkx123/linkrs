use std::collections::HashMap;

use crate::executor::streaming::helpers::accumulator_states::{
    decode_partial_with_args, AggregateAccumulator,
};
use crate::executor::streaming::spill::{HashPartitionSpiller, SpilledRun};
use linkrs_core::columnar::{MaterializedBatch, RowKey};
use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::Value;

/// Estimated in-memory overhead of one `AggregateAccumulator` instance
/// (enum tag plus internal state) used for memory accounting. Charged once
/// per aggregate function per group so workloads with many small keys are
/// accounted for beyond the group key itself.
pub const ACCUMULATOR_OVERHEAD_BYTES: usize = 64;

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
    /// One reservation per distinct group, matching the generic path's
    /// key-plus-accumulator accounting.
    pub bytes_per_group: usize,
}

#[derive(Debug)]
pub struct AggregateState {
    /// Accumulator state per group key (computed key values, already minimal;
    /// `RowKey` documents the key-projection contract without changing layout).
    pub group_map: HashMap<RowKey, Vec<AggregateAccumulator>>,
    /// Per-group memory-budget overhead charged for accumulator instances.
    pub accumulator_overhead: usize,
    /// Output buffer replacing the drained row iterator.
    pub result_batch: MaterializedBatch,
    /// Output cursor into `result_batch`.
    pub emitted_offset: usize,
    pub partition_spiller: Option<HashPartitionSpiller>,
    pub spilled_runs: Vec<Option<SpilledRun>>,
    pub current_partition: usize,
    pub has_spilled: bool,
    /// True once the final aggregate result has been fully emitted.
    pub output_complete: bool,
    pub col_names: Vec<String>,
    pub typed_group_count: Option<TypedGroupCount>,
}

#[derive(Debug)]
pub struct GroupByState {
    /// Column-oriented input buffer (was `Vec<Vec<Value>>`).
    pub batch: MaterializedBatch,
    pub col_names: Vec<String>,
    /// Output buffer replacing the drained row iterator.
    pub result_batch: MaterializedBatch,
    /// Output cursor into `result_batch`.
    pub emitted_offset: usize,
    pub partition_spiller: Option<HashPartitionSpiller>,
    pub spilled_runs: Vec<Option<SpilledRun>>,
    pub current_partition: usize,
    pub has_spilled: bool,
    /// True once all groups have been emitted.
    pub output_complete: bool,
}

#[derive(Debug)]
pub struct PartialAggregateState {
    pub group_map: HashMap<RowKey, Vec<AggregateAccumulator>>,
    pub col_names: Vec<String>,
    /// Output buffer replacing the drained row iterator.
    pub result_batch: MaterializedBatch,
    /// Output cursor into `result_batch`.
    pub emitted_offset: usize,
}

#[derive(Debug)]
pub struct FinalAggregateState {
    pub group_map: HashMap<RowKey, Vec<AggregateAccumulator>>,
    pub col_names: Vec<String>,
    /// Output buffer replacing the drained row iterator.
    pub result_batch: MaterializedBatch,
    /// Output cursor into `result_batch`.
    pub emitted_offset: usize,
}

pub(crate) fn value_to_partial_accumulator(
    func: &AggregateFunction,
    args: &[Expression],
    value: &Value,
) -> Option<AggregateAccumulator> {
    decode_partial_with_args(func, args, value)
}
