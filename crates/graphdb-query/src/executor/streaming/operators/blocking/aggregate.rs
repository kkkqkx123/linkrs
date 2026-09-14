use std::collections::HashMap;

use crate::executor::streaming::helpers::accumulator_states::{
    decode_partial_with_args, AggregateAccumulator,
};
use crate::executor::streaming::spill::{HashPartitionSpiller, SpilledRun};
use graphdb_core::columnar::{MaterializedBatch, RowKey};
use graphdb_core::types::expr::Expression;
use graphdb_core::types::operators::AggregateFunction;
use graphdb_core::Value;

/// Estimated in-memory overhead of one `AggregateAccumulator` instance
/// (enum tag plus internal state) used for memory accounting. Charged once
/// per aggregate function per group so workloads with many small keys are
/// accounted for beyond the group key itself.
pub const ACCUMULATOR_OVERHEAD_BYTES: usize = 64;

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
