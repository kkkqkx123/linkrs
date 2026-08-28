//! Implementation of the filtering node
//!
//! The `FilterNode` is used to filter the input data stream based on specified conditions.

use std::sync::Arc;

use crate::define_plan_node_with_deps;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::statements::clauses::exists_planner::PlannedSubquery;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::{ContextualExpression, SerializableExpression};

define_plan_node_with_deps! {
    pub struct FilterNode {
        condition: ContextualExpression,
        condition_serializable: Option<SerializableExpression>,
        // Expression-level EXISTS / IN subqueries compiled for this filter
        // Pre-execution only; never serialized.
        subqueries: Vec<PlannedSubquery>,
        // Whether constant folding replaced part of the condition.
        has_folded_expressions: bool,
    }
    enum: Filter
    input: SingleInputNode
}

impl FilterNode {
    /// Create a new filter node.
    pub fn new(
        input: PlanNodeEnum,
        condition: ContextualExpression,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            condition,
            condition_serializable: None,
            subqueries: Vec::new(),
            has_folded_expressions: false,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    /// Attach expression-level subqueries to this filter.
    pub fn with_subqueries(mut self, subqueries: Vec<PlannedSubquery>) -> Self {
        self.subqueries = subqueries;
        self
    }

    /// Expression-level subqueries compiled for this filter.
    pub fn subqueries(&self) -> &[PlannedSubquery] {
        &self.subqueries
    }

    /// Whether constant folding replaced part of the condition.
    pub fn has_folded_expressions(&self) -> bool {
        self.has_folded_expressions
    }

    /// Mark whether constant folding replaced part of the condition.
    pub fn set_has_folded_expressions(&mut self, val: bool) {
        self.has_folded_expressions = val;
    }

    /// Obtain the filtering criteria
    pub fn condition(&self) -> &ContextualExpression {
        &self.condition
    }

    /// Set filter criteria
    pub fn set_condition(&mut self, condition: ContextualExpression) {
        self.condition = condition;
        self.condition_serializable = None;
    }

    pub fn prepare_for_serialization(&mut self) -> Result<(), String> {
        self.condition_serializable =
            Some(SerializableExpression::from_contextual(&self.condition)?);
        Ok(())
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_expr) = self.condition_serializable {
            self.condition = ser_expr.clone().to_contextual(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::expr::ExpressionMeta;
    use graphdb_core::Expression;
    use std::sync::Arc;
    use ExpressionAnalysisContext;

    #[test]
    fn test_filter_node_creation() {
        let start_node =
            crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        let start_node_enum =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                start_node,
            );

        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("test".to_string());
        let expr_meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let condition = ContextualExpression::new(id, ctx);

        let filter_node = FilterNode::new(start_node_enum, condition)
            .expect("Filter node should be created successfully");

        assert_eq!(filter_node.type_name(), "FilterNode");
        assert!(filter_node.condition().is_variable());
    }
}
