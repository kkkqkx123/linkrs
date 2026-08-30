//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::define_plan_node_with_deps;
use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::planning::statements::clauses::exists_planner::PlannedSubquery;
use graphdb_core::types::expr::contextual::ContextualExpression;

define_plan_node_with_deps! {
    pub struct DataCollectNode {
        collect_kind: String,
    }
    enum: DataCollect
    input: SingleInputNode
}

impl DataCollectNode {
    pub fn new(
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        collect_kind: &str,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            collect_kind: collect_kind.to_string(),
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn collect_kind(&self) -> &str {
        &self.collect_kind
    }
}
