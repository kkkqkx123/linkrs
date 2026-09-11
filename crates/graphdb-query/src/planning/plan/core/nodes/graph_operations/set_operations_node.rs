//! Implementation of set operation nodes
//!
//! Provide definitions for the planning nodes related to set operations.

use crate::define_plan_node;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

define_plan_node! {
    pub struct MinusNode {
    }
    enum: Minus
    input: MultipleInputNode
}

impl MinusNode {
    pub fn new(
        input: PlanNodeEnum,
        minus_input: PlanNodeEnum,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            deps: vec![input, minus_input],
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn minus_input(&self) -> &PlanNodeEnum {
        &self.deps[1]
    }
}

// The primary input is `deps[0]`; there is no second copy of the subtree.
impl SingleInputNode for MinusNode {
    fn input(&self) -> &PlanNodeEnum {
        &self.deps[0]
    }

    fn input_mut(&mut self) -> &mut PlanNodeEnum {
        &mut self.deps[0]
    }

    fn set_input(&mut self, input: PlanNodeEnum) {
        if self.deps.is_empty() {
            self.deps.push(input);
        } else {
            self.deps[0] = input;
        }
    }
}

define_plan_node! {
    pub struct IntersectNode {
    }
    enum: Intersect
    input: MultipleInputNode
}

impl IntersectNode {
    pub fn new(
        input: PlanNodeEnum,
        intersect_input: PlanNodeEnum,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            deps: vec![input, intersect_input],
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn intersect_input(&self) -> &PlanNodeEnum {
        &self.deps[1]
    }
}

// The primary input is `deps[0]`; there is no second copy of the subtree.
impl SingleInputNode for IntersectNode {
    fn input(&self) -> &PlanNodeEnum {
        &self.deps[0]
    }

    fn input_mut(&mut self) -> &mut PlanNodeEnum {
        &mut self.deps[0]
    }

    fn set_input(&mut self, input: PlanNodeEnum) {
        if self.deps.is_empty() {
            self.deps.push(input);
        } else {
            self.deps[0] = input;
        }
    }
}
