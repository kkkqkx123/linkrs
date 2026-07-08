//! Implementation of the projection node
//!
//! ProjectNode is used to project the input data stream based on a specified list expression.

use std::sync::Arc;

use crate::core::types::SerializableExpression;
use crate::core::YieldColumn;
use crate::define_plan_node_with_deps;
use crate::query::validator::context::ExpressionAnalysisContext;

define_plan_node_with_deps! {
    pub struct ProjectNode {
        columns: Vec<YieldColumn>,
        columns_serializable: Option<Vec<SerializableExpression>>,
    }
    enum: Project
    input: SingleInputNode
}

impl ProjectNode {
    /// Create a new projection node.
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        columns: Vec<YieldColumn>,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names: Vec<String> = columns.iter().map(|col| col.alias.clone()).collect();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            columns,
            columns_serializable: None,
            output_var: None,
            col_names,
        })
    }

    /// Obtain the projection column
    pub fn columns(&self) -> &[YieldColumn] {
        &self.columns
    }

    /// Set the projection column
    pub fn set_columns(&mut self, columns: Vec<YieldColumn>) {
        self.columns = columns;
        self.col_names = self.columns.iter().map(|col| col.alias.clone()).collect();
    }

    pub fn prepare_for_serialization(&mut self, _ctx: Arc<ExpressionAnalysisContext>) {
        self.columns_serializable = Some(
            self.columns
                .iter()
                .map(|col| SerializableExpression::from_contextual(&col.expression))
                .collect(),
        );
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_columns) = self.columns_serializable {
            self.columns = ser_columns
                .iter()
                .map(|ser_expr| {
                    let ctx_expr = ser_expr.clone().to_contextual(ctx.clone());
                    YieldColumn {
                        expression: ctx_expr,
                        alias: ser_expr.expression.to_expression_string(),
                        is_matched: false,
                    }
                })
                .collect();
            self.col_names = self.columns.iter().map(|col| col.alias.clone()).collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::types::ContextualExpression;
    use crate::core::Expression;
    use std::sync::Arc;
    use ExpressionAnalysisContext;

    #[test]
    fn test_project_node_creation() {
        let start_node =
            crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new(
                ),
            );

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("test".to_string());
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);

        let columns = vec![YieldColumn {
            expression: ctx_expr,
            alias: "test".to_string(),
            is_matched: false,
        }];

        let project_node = ProjectNode::new(start_node, columns)
            .expect("Project node should be created successfully");

        assert_eq!(project_node.type_name(), "ProjectNode");
        assert_eq!(project_node.col_names().len(), 1);
        assert_eq!(project_node.col_names()[0], "test");
    }

    #[test]
    fn test_project_node_columns() {
        let start_node =
            crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new(
                ),
            );

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());

        let name_expr = Expression::Variable("name".to_string());
        let name_meta = ExpressionMeta::new(name_expr);
        let name_id = expr_ctx.register_expression(name_meta);
        let name_ctx_expr = ContextualExpression::new(name_id, expr_ctx.clone());

        let age_expr = Expression::Variable("age".to_string());
        let age_meta = ExpressionMeta::new(age_expr);
        let age_id = expr_ctx.register_expression(age_meta);
        let age_ctx_expr = ContextualExpression::new(age_id, expr_ctx);

        let columns = vec![
            YieldColumn {
                expression: name_ctx_expr,
                alias: "name".to_string(),
                is_matched: false,
            },
            YieldColumn {
                expression: age_ctx_expr,
                alias: "age".to_string(),
                is_matched: false,
            },
        ];

        let project_node = ProjectNode::new(start_node, columns)
            .expect("Project node should be created successfully");

        assert_eq!(project_node.columns().len(), 2);
        assert_eq!(project_node.columns()[0].alias, "name");
        assert_eq!(project_node.columns()[1].alias, "age");
    }
}
