use std::collections::HashSet;

use crate::core::Value;
use crate::query::executor::streaming::spill::SpilledFile;

#[derive(Debug)]
pub struct DistinctState {
    pub seen_rows: HashSet<Vec<Value>>,
    pub col_names: Vec<String>,
    pub spill_files: Vec<SpilledFile>,
}

#[derive(Debug)]
pub struct MaterializeState {
    pub materialized_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub materialized: bool,
    pub spill_files: Vec<SpilledFile>,
}

#[derive(Debug)]
pub struct DataCollectState {
    pub all_rows: Vec<Vec<Value>>,
    pub emitted: bool,
    pub spill_files: Vec<SpilledFile>,
}

#[derive(Debug)]
pub struct RollUpApplyState {
    pub all_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
}
