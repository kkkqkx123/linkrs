use std::collections::HashMap;

use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::core::Value;
use crate::executor::streaming::helpers::accumulator_states::{
    decode_partial_with_args, AggregateAccumulator,
};
use crate::executor::streaming::spill::{HashPartitionSpiller, SpilledFile, SpilledRun};

/// Estimated in-memory overhead of one `AggregateAccumulator` instance
/// (enum tag plus internal state) used for memory accounting. Charged once
/// per aggregate function per group so workloads with many small keys are
/// accounted for beyond the group key itself.
pub const ACCUMULATOR_OVERHEAD_BYTES: usize = 64;

#[derive(Debug)]
pub struct AggregateState {
    /// Accumulator state per group key; every aggregate function has an
    /// `AggregateAccumulator`, so no row-based fallback exists.
    pub group_map: HashMap<Vec<Value>, Vec<AggregateAccumulator>>,
    /// Per-group memory-budget overhead charged for accumulator instances.
    pub accumulator_overhead: usize,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
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
    pub all_rows: Vec<Vec<Value>>,
    pub col_names: Vec<String>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
    pub partition_spiller: Option<HashPartitionSpiller>,
    pub spilled_runs: Vec<Option<SpilledRun>>,
    pub current_partition: usize,
    pub has_spilled: bool,
    /// True once all groups have been emitted.
    pub output_complete: bool,
}

#[derive(Debug)]
pub struct PartialAggregateState {
    pub group_map: HashMap<Vec<Value>, Vec<AggregateAccumulator>>,
    pub col_names: Vec<String>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
}

#[derive(Debug)]
pub struct FinalAggregateState {
    pub group_map: HashMap<Vec<Value>, Vec<AggregateAccumulator>>,
    pub col_names: Vec<String>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
}

pub(crate) fn value_to_partial_accumulator(
    func: &AggregateFunction,
    args: &[Expression],
    value: &Value,
) -> Option<AggregateAccumulator> {
    decode_partial_with_args(func, args, value)
}
