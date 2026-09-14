//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::define_plan_node_with_deps;
use graphdb_core::types::expr::contextual::ContextualExpression;

define_plan_node_with_deps! {
    pub struct UnwindNode {
        alias: String,
        list_expression: ContextualExpression,
    }
    enum: Unwind
    input: SingleInputNode
}

impl UnwindNode {
    pub fn new(
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        alias: &str,
        list_expression: ContextualExpression,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        if alias.is_empty() {
            return Err(
                crate::planning::planner::PlannerError::PlanGenerationFailed(
                    "UNWIND output alias has an empty alias".to_string(),
                ),
            );
        }
        let mut col_names = input.col_names().to_vec();
        if col_names.iter().any(|name| name == alias) {
            return Err(
                crate::planning::planner::PlannerError::PlanGenerationFailed(format!(
                    "UNWIND alias '{alias}' conflicts with existing column"
                )),
            );
        }
        crate::planning::statements::projection_util::validate_output_aliases(&col_names)?;
        col_names.push(alias.to_string());
        let column_types = vec![graphdb_core::DataType::Unknown; col_names.len()];

        Ok(Self {
            id: -1,
            input: Some(Box::new(input)),
            alias: alias.to_string(),
            list_expression,
            output_var: None,
            col_names,
            column_types,
        })
    }

    pub fn alias(&self) -> &str {
        &self.alias
    }

    pub fn list_expression(&self) -> &ContextualExpression {
        &self.list_expression
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::ExpressionMeta;
    use graphdb_core::Expression;
    use std::sync::Arc;

    fn list_expr() -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(Expression::List(vec![])));
        ContextualExpression::new(id, ctx)
    }

    fn start_enum() -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
            crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
        )
    }

    #[test]
    fn test_unwind_rejects_empty_alias() {
        let err = UnwindNode::new(start_enum(), "", list_expr()).expect_err("empty must fail");
        assert!(err.to_string().contains("empty alias"));
    }

    #[test]
    fn test_unwind_rejects_conflicting_alias() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id =
            ctx.register_expression(ExpressionMeta::new(Expression::Variable("x".to_string())));
        let col = graphdb_core::YieldColumn {
            expression: ContextualExpression::new(id, ctx),
            alias: "x".to_string(),
            is_matched: false,
        };
        let project =
            crate::planning::plan::core::nodes::operation::project_node::ProjectNode::new(
                start_enum(),
                vec![col],
            )
            .expect("project builds");
        let input = crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Project(
            project,
        );
        let err = UnwindNode::new(input, "x", list_expr()).expect_err("conflict must fail");
        assert!(err.to_string().contains("conflicts"));
    }

    #[test]
    fn test_unwind_column_types_follow_names() {
        let node = UnwindNode::new(start_enum(), "xs", list_expr()).expect("builds");
        assert_eq!(node.col_names, vec!["xs".to_string()]);
        assert_eq!(node.column_types.len(), node.col_names.len());
    }
}
