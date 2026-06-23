//! Implementation of the statistical information node
//!
//! Provide definitions of the planning nodes related to statistical information queries.

use crate::define_plan_node;

define_plan_node! {
    pub struct ShowStatsNode {
        stats_type: ShowStatsType,
    }
    enum: ShowStats
    input: ZeroInputNode
}

impl ShowStatsNode {
    pub fn new(id: i64, stats_type: ShowStatsType) -> Self {
        Self {
            id,
            stats_type,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn stats_type(&self) -> &ShowStatsType {
        &self.stats_type
    }
}

/// Display the statistical type.
#[derive(Debug, Clone)]
pub enum ShowStatsType {
    /// Display storage statistics
    Storage,
    /// Display space statistics
    Space,
}
