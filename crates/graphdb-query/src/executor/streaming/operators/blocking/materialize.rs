use std::collections::HashSet;
use std::sync::Arc;

use graphdb_core::Value;
use crate::executor::streaming::slot::SlotLayout;
use crate::executor::streaming::spill::{HashPartitionSpiller, SpilledFile, SpilledRun};

#[derive(Debug)]
pub struct DistinctState {
    pub seen_rows: HashSet<Vec<Value>>,
    pub col_names: Vec<String>,
    pub input_layout: Option<Arc<SlotLayout>>,
    pub spill_files: Vec<SpilledFile>,
    pub partition_spiller: Option<HashPartitionSpiller>,
    pub spilled_runs: Vec<Option<SpilledRun>>,
    pub current_partition: usize,
    pub partition_seen: HashSet<Vec<Value>>,
    pub has_spilled: bool,
    pub output_iter: Option<std::vec::IntoIter<Vec<Value>>>,
}

#[derive(Debug)]
pub struct MaterializeState {
    pub materialized_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub materialized: bool,
    pub spill_files: Vec<SpilledFile>,
    pub input_layout: Option<Arc<SlotLayout>>,
}

#[derive(Debug)]
pub struct DataCollectState {
    pub all_rows: Vec<Vec<Value>>,
    pub emitted: bool,
    pub spill_files: Vec<SpilledFile>,
    pub input_layout: Option<Arc<SlotLayout>>,
}

#[derive(Debug)]
pub struct RollUpApplyState {
    pub all_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
}
