//! Implementation of Aggregation Nodes
//!
//! The `AggregateNode` is used to perform aggregation operations on the input data.

use crate::core::types::operators::AggregateFunction;
use crate::define_plan_node_with_deps;

define_plan_node_with_deps! {
    pub struct AggregateNode {
        group_keys: Vec<String>,
        aggregation_functions: Vec<AggregateFunction>,
    }
    enum: Aggregate
    input: SingleInputNode
}

impl AggregateNode {
    /// Create a new aggregate node.
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        group_keys: Vec<String>,
        aggregation_functions: Vec<AggregateFunction>,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let mut col_names: Vec<String> = group_keys.clone();
        for agg_func in &aggregation_functions {
            col_names.push(agg_func.name().to_string());
        }

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            group_keys,
            aggregation_functions,
            output_var: None,
            col_names,
        })
    }

    /// Create a new aggregate node with custom column names for aggregate functions.
    pub fn with_agg_aliases(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        group_keys: Vec<String>,
        aggregation_functions: Vec<AggregateFunction>,
        agg_aliases: Vec<String>,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let mut col_names: Vec<String> = group_keys.clone();
        for alias in &agg_aliases {
            col_names.push(alias.clone());
        }

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            group_keys,
            aggregation_functions,
            output_var: None,
            col_names,
        })
    }

    /// Obtain the group key
    pub fn group_keys(&self) -> &[String] {
        &self.group_keys
    }

    /// Obtain a list of aggregate functions
    pub fn aggregation_functions(&self) -> &[AggregateFunction] {
        &self.aggregation_functions
    }

    /// Obtaining aggregate expressions (also known as alias methods, which are the same as aggregation_functions)
    pub fn agg_exprs(&self) -> &[AggregateFunction] {
        &self.aggregation_functions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aggregate_node_creation() {
        let start_node =
            crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new(
                ),
            );

        let group_keys = vec!["category".to_string()];
        let aggregation_functions = vec![AggregateFunction::Count(None)];

        let aggregate_node = AggregateNode::new(start_node, group_keys, aggregation_functions)
            .expect("Aggregate node should be created successfully");

        assert_eq!(aggregate_node.type_name(), "AggregateNode");
        assert_eq!(aggregate_node.dependencies().len(), 1);
        assert_eq!(aggregate_node.group_keys().len(), 1);
        assert_eq!(aggregate_node.aggregation_functions().len(), 1);
    }
}
