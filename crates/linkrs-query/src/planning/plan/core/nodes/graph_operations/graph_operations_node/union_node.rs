//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::define_plan_node;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

define_plan_node! {
    pub struct UnionNode {
        distinct: bool,
    }
    enum: Union
    input: MultipleInputNode
}

impl UnionNode {
    pub fn new(
        input: PlanNodeEnum,
        union_input: PlanNodeEnum,
        distinct: bool,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
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

    pub fn union_input(&self) -> &PlanNodeEnum {
        &self.deps[1]
    }
}

// The primary input is `deps[0]`; there is no second copy of the subtree.
impl SingleInputNode for UnionNode {
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
