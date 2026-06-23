//! Implementation of the filtering node
//!
//! The `FilterNode` is used to filter the input data stream based on specified conditions.

use std::sync::Arc;

use crate::core::types::{ContextualExpression, SerializableExpression};
use crate::define_plan_node_with_deps;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::validator::context::ExpressionAnalysisContext;

define_plan_node_with_deps! {
    pub struct FilterNode {
        condition: ContextualExpression,
        condition_serializable: Option<SerializableExpression>,
    }
    enum: Filter
    input: SingleInputNode
}

impl FilterNode {
    /// Create a new filter node.
    pub fn new(
        input: PlanNodeEnum,
        condition: ContextualExpression,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            condition,
            condition_serializable: None,
            output_var: None,
            col_names,
        })
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

    pub fn prepare_for_serialization(&mut self) {
        self.condition_serializable =
            Some(SerializableExpression::from_contextual(&self.condition));
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
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::Expression;
    use std::sync::Arc;
    use ExpressionAnalysisContext;

    #[test]
    fn test_filter_node_creation() {
        let start_node =
            crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        let start_node_enum =
            crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
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
