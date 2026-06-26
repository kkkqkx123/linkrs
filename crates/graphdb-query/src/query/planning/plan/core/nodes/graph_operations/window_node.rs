//! Window function plan node
//!
//! Represents a window function operation (OVER clause) in the query plan.
//! Handles PARTITION BY and ORDER BY within partitions.

use crate::core::types::expr::Expression;
use crate::define_plan_node_with_deps;

/// Specification for a window function call
#[derive(Debug, Clone)]
pub struct WindowFunctionSpec {
    pub name: String,
    pub args: Vec<Expression>,
    pub partition_by: Vec<Expression>,
    pub order_by: Vec<Expression>,
    pub order_desc: Vec<bool>,
}

define_plan_node_with_deps! {
    pub struct WindowNode {
        window_functions: Vec<WindowFunctionSpec>,
    }
    enum: Window
    input: SingleInputNode
}

impl WindowNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        window_functions: Vec<WindowFunctionSpec>,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names: Vec<String> = window_functions
            .iter()
            .map(|wf| wf.name.clone())
            .collect();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            window_functions,
            output_var: None,
            col_names,
        })
    }

    pub fn window_functions(&self) -> &[WindowFunctionSpec] {
        &self.window_functions
    }
}
