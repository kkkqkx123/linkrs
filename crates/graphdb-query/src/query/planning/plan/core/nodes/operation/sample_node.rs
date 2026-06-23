//! Implementation of the sampling node
//!
//! SampleNode is used to perform random sampling on the input data.

use crate::define_plan_node_with_deps;

define_plan_node_with_deps! {
    pub struct SampleNode {
        count: i64,
    }
    enum: Sample
    input: SingleInputNode
}

impl SampleNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        count: i64,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            count,
            output_var: None,
            col_names,
        })
    }

    pub fn count(&self) -> i64 {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;

    #[test]
    fn test_sample_node_creation() {
        let start_node = PlanNodeEnum::Start(StartNode::new());

        let sample_node =
            SampleNode::new(start_node, 10).expect("SampleNode creation should succeed");

        assert_eq!(sample_node.type_name(), "SampleNode");
        assert_eq!(sample_node.dependencies().len(), 1);
        assert_eq!(sample_node.count(), 10);
    }
}
