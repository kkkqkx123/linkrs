//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::define_plan_node_with_deps;
use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::planning::statements::clauses::exists_planner::PlannedSubquery;
use graphdb_core::types::expr::contextual::ContextualExpression;

define_plan_node_with_deps! {
    pub struct AssignNode {
        assignments: Vec<(String, ContextualExpression)>,
        // Expression-level EXISTS / IN subqueries compiled for this assign
        // Pre-execution only; never serialized.
        subqueries: Vec<PlannedSubquery>,
        // Whether constant folding replaced part of the assignments.
        has_folded_expressions: bool,
    }
    enum: Assign
    input: SingleInputNode
}

impl AssignNode {
    pub fn new(
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        assignments: Vec<(String, ContextualExpression)>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            assignments,
            subqueries: Vec::new(),
            has_folded_expressions: false,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn assignments(&self) -> &[(String, ContextualExpression)] {
        &self.assignments
    }

    /// Replace the assignments (preserving subqueries).
    pub fn set_assignments(&mut self, assignments: Vec<(String, ContextualExpression)>) {
        self.assignments = assignments;
    }

    /// Attach expression-level subqueries to this assign.
    pub fn with_subqueries(mut self, subqueries: Vec<PlannedSubquery>) -> Self {
        self.subqueries = subqueries;
        self
    }

    /// Expression-level subqueries compiled for this assign.
    pub fn subqueries(&self) -> &[PlannedSubquery] {
        &self.subqueries
    }

    /// Whether constant folding replaced part of the assignments.
    pub fn has_folded_expressions(&self) -> bool {
        self.has_folded_expressions
    }

    /// Mark whether constant folding replaced part of the assignments.
    pub fn set_has_folded_expressions(&mut self, val: bool) {
        self.has_folded_expressions = val;
    }
}
