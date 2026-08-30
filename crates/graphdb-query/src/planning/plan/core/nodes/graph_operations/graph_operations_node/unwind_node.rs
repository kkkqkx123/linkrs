//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::define_plan_node_with_deps;
use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::planning::statements::clauses::exists_planner::PlannedSubquery;
use graphdb_core::types::expr::contextual::ContextualExpression;

define_plan_node_with_deps! {
    pub struct UnwindNode {
        alias: String,
        list_expression: ContextualExpression,
    }
    enum: Unwind
    input: SingleInputNode
}

impl UnwindNode {
    pub fn new(
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        alias: &str,
        list_expression: ContextualExpression,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let mut col_names = input.col_names().to_vec();
        col_names.push(alias.to_string());

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            alias: alias.to_string(),
            list_expression,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn alias(&self) -> &str {
        &self.alias
    }

    pub fn list_expression(&self) -> &ContextualExpression {
        &self.list_expression
    }
}
