//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::define_plan_node_with_deps;
use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::planning::statements::clauses::exists_planner::PlannedSubquery;
use graphdb_core::types::expr::contextual::ContextualExpression;

define_plan_node_with_deps! {
    pub struct UnionNode {
        distinct: bool,
    }
    enum: Union
    input: SingleInputNode
}

impl UnionNode {
    pub fn new(
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        union_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        distinct: bool,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input, union_input],
            distinct,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn distinct(&self) -> bool {
        self.distinct
    }

    pub fn union_input(
        &self,
    ) -> &crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.deps[1]
    }
}
