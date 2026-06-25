//! Implementation of Aggregation Nodes
//!
//! The `AggregateNode` is used to perform aggregation operations on the input data.

use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::define_plan_node_with_deps;

define_plan_node_with_deps! {
    pub struct AggregateNode {
        group_keys: Vec<String>,
        aggregation_functions: Vec<AggregateFunction>,
        aggregation_distinct: Vec<bool>,
        aggregation_filters: Vec<Option<Expression>>,
        grouping_sets: Vec<Vec<String>>,
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
        let num_agg = aggregation_functions.len();
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
            aggregation_distinct: vec![false; num_agg],
            aggregation_filters: vec![None; num_agg],
            grouping_sets: Vec::new(),
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
        let num_agg = aggregation_functions.len();
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
            aggregation_distinct: vec![false; num_agg],
            aggregation_filters: vec![None; num_agg],
            grouping_sets: Vec::new(),
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

    /// Obtain distinct flags for aggregate functions
    pub fn aggregation_distinct(&self) -> &[bool] {
        &self.aggregation_distinct
    }

    /// Obtain filters for aggregate functions
    pub fn aggregation_filters(&self) -> &[Option<Expression>] {
        &self.aggregation_filters
    }

    /// Obtaining grouping sets for ROLLUP/CUBE/GROUPING SETS support
    pub fn grouping_sets(&self) -> &[Vec<String>] {
        &self.grouping_sets
    }

    /// Set distinct flags for aggregate functions
    pub fn set_aggregation_distinct(&mut self, distinct: Vec<bool>) {
        self.aggregation_distinct = distinct;
    }

    /// Set filters for aggregate functions
    pub fn set_aggregation_filters(&mut self, filters: Vec<Option<Expression>>) {
        self.aggregation_filters = filters;
    }

    /// Set grouping sets for ROLLUP/CUBE/GROUPING SETS support
    pub fn set_grouping_sets(&mut self, sets: Vec<Vec<String>>) {
        self.grouping_sets = sets;
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

    #[test]
    fn test_aggregate_node_with_grouping_sets() {
        let start_node =
            crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new(
                ),
            );

        let group_keys = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let aggregation_functions = vec![AggregateFunction::Count(None)];

        let mut aggregate_node =
            AggregateNode::new(start_node, group_keys, aggregation_functions)
                .expect("Aggregate node should be created successfully");

        let sets = vec![
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec!["a".to_string(), "b".to_string()],
            vec!["a".to_string()],
            vec![],
        ];
        aggregate_node.set_grouping_sets(sets.clone());

        assert_eq!(aggregate_node.grouping_sets(), &sets);
    }
}
