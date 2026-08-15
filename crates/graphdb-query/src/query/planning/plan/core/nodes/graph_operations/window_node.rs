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
        // Whether constant folding replaced part of the window specs.
        has_folded_expressions: bool,
    }
    enum: Window
    input: SingleInputNode
}

impl WindowNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        window_functions: Vec<WindowFunctionSpec>,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names: Vec<String> = window_functions.iter().map(|wf| wf.name.clone()).collect();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            window_functions,
            has_folded_expressions: false,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn window_functions(&self) -> &[WindowFunctionSpec] {
        &self.window_functions
    }

    /// Replace the window function specifications.
    pub fn set_window_functions(&mut self, window_functions: Vec<WindowFunctionSpec>) {
        self.window_functions = window_functions;
    }

    /// Whether constant folding replaced part of the window specs.
    pub fn has_folded_expressions(&self) -> bool {
        self.has_folded_expressions
    }

    /// Mark whether constant folding replaced part of the window specs.
    pub fn set_has_folded_expressions(&mut self, val: bool) {
        self.has_folded_expressions = val;
    }
}
