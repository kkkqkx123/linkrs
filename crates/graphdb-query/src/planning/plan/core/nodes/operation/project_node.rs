//! Implementation of the projection node
//!
//! ProjectNode is used to project the input data stream based on a specified list expression.

use std::sync::Arc;

use crate::define_plan_node_with_deps;
use crate::planning::statements::clauses::exists_planner::PlannedSubquery;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::SerializableYieldColumn;
use graphdb_core::DataType;
use graphdb_core::YieldColumn;

define_plan_node_with_deps! {
    pub struct ProjectNode {
        columns: Vec<YieldColumn>,
        columns_serializable: Option<Vec<SerializableYieldColumn>>,
        // Expression-level EXISTS / IN subqueries compiled for this project
        // Pre-execution only; never serialized.
        subqueries: Vec<PlannedSubquery>,
        // Whether constant folding replaced part of the columns.
        has_folded_expressions: bool,
    }
    enum: Project
    input: SingleInputNode
}

impl ProjectNode {
    /// Derive per-column types from the contextual type slots.
    ///
    /// Unknown when the originating binder did not resolve a type; keeps
    /// `column_types.len() == col_names.len()` so downstream width estimates
    /// and slot layouts never observe a truncated vector.
    pub fn derive_column_types(columns: &[YieldColumn]) -> Vec<DataType> {
        columns
            .iter()
            .map(|col| col.expression.data_type().unwrap_or(DataType::Unknown))
            .collect()
    }

    /// Create a new projection node.
    pub fn new(
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        columns: Vec<YieldColumn>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        if columns.is_empty() {
            return Err(
                crate::planning::planner::PlannerError::PlanGenerationFailed(
                    "ProjectNode requires at least one output column".to_string(),
                ),
            );
        }
        let aliases: Vec<String> = columns.iter().map(|col| col.alias.clone()).collect();
        crate::planning::statements::projection_util::validate_output_aliases(&aliases)?;
        let col_names: Vec<String> = columns.iter().map(|col| col.alias.clone()).collect();
        let column_types = Self::derive_column_types(&columns);

        Ok(Self {
            id: -1,
            input: Some(Box::new(input)),
            columns,
            columns_serializable: None,
            subqueries: Vec::new(),
            has_folded_expressions: false,
            output_var: None,
            col_names,
            column_types,
        })
    }

    /// Attach expression-level subqueries to this projection.
    pub fn with_subqueries(mut self, subqueries: Vec<PlannedSubquery>) -> Self {
        self.subqueries = subqueries;
        self
    }

    /// Expression-level subqueries compiled for this projection.
    pub fn subqueries(&self) -> &[PlannedSubquery] {
        &self.subqueries
    }

    /// Whether constant folding replaced part of the columns.
    pub fn has_folded_expressions(&self) -> bool {
        self.has_folded_expressions
    }

    /// Mark whether constant folding replaced part of the columns.
    pub fn set_has_folded_expressions(&mut self, val: bool) {
        self.has_folded_expressions = val;
    }

    /// Obtain the projection column
    pub fn columns(&self) -> &[YieldColumn] {
        &self.columns
    }

    /// Set the projection column
    pub fn set_columns(&mut self, columns: Vec<YieldColumn>) {
        self.column_types = Self::derive_column_types(&columns);
        self.columns = columns;
        self.col_names = self.columns.iter().map(|col| col.alias.clone()).collect();
    }

    pub fn prepare_for_serialization(
        &mut self,
        _ctx: Arc<ExpressionAnalysisContext>,
    ) -> Result<(), String> {
        self.columns_serializable = Some(
            self.columns
                .iter()
                .map(SerializableYieldColumn::from_yield_column)
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(())
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_columns) = self.columns_serializable {
            self.columns = ser_columns
                .iter()
                .cloned()
                .map(|ser_col| ser_col.to_yield_column(ctx.clone()))
                .collect();
            self.column_types = Self::derive_column_types(&self.columns);
            self.col_names = self.columns.iter().map(|col| col.alias.clone()).collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::expr::ExpressionMeta;
    use graphdb_core::types::ContextualExpression;
    use graphdb_core::Expression;
    use std::sync::Arc;
    use ExpressionAnalysisContext;

    #[test]
    fn test_project_node_creation() {
        let start_node =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
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
        assert_eq!(project_node.column_types().len(), 1);
    }

    #[test]
    fn test_project_column_types_derive_from_context() {
        let start_node =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
            );
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = expr_ctx
            .register_expression(ExpressionMeta::new(Expression::Variable("n".to_string())));
        expr_ctx.set_type(&id, DataType::Int);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);
        let columns = vec![YieldColumn {
            expression: ctx_expr,
            alias: "n".to_string(),
            is_matched: false,
        }];
        let node = ProjectNode::new(start_node, columns).expect("Project node should build");
        assert_eq!(node.column_types(), &[DataType::Int]);
    }

    #[test]
    fn test_project_node_columns() {
        let start_node =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
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

    #[test]
    fn test_project_serialization_preserves_alias() {
        let start_node =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
            );

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("n".to_string());
        let id = expr_ctx.register_expression(ExpressionMeta::new(expr));
        let ctx_expr = ContextualExpression::new(id, expr_ctx.clone());

        let columns = vec![YieldColumn {
            expression: ctx_expr,
            alias: "custom_alias".to_string(),
            is_matched: true,
        }];

        let mut node = ProjectNode::new(start_node, columns).expect("Project node should build");
        node.prepare_for_serialization(expr_ctx.clone())
            .expect("serialization prepare should succeed");
        node.after_deserialization(expr_ctx);

        assert_eq!(node.columns().len(), 1);
        assert_eq!(node.columns()[0].alias, "custom_alias");
        assert!(node.columns()[0].is_matched);
        assert_eq!(node.col_names(), &["custom_alias".to_string()]);
    }

    fn test_column(ctx: &Arc<ExpressionAnalysisContext>, alias: &str) -> YieldColumn {
        let id =
            ctx.register_expression(ExpressionMeta::new(Expression::Variable(alias.to_string())));
        YieldColumn {
            expression: ContextualExpression::new(id, ctx.clone()),
            alias: alias.to_string(),
            is_matched: false,
        }
    }

    #[test]
    fn test_project_rejects_empty_columns() {
        let start_node =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
            );
        let err = ProjectNode::new(start_node, Vec::new()).expect_err("empty columns must fail");
        assert!(err.to_string().contains("at least one"));
    }

    #[test]
    fn test_project_rejects_empty_alias() {
        let start_node =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
            );
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let col = test_column(&ctx, "");
        let err = ProjectNode::new(start_node, vec![col]).expect_err("empty alias must fail");
        assert!(err.to_string().contains("empty alias"));
    }

    #[test]
    fn test_project_rejects_duplicate_alias() {
        let start_node =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
            );
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let first = test_column(&ctx, "a");
        let second = test_column(&ctx, "a");
        let err = ProjectNode::new(start_node, vec![first, second])
            .expect_err("duplicate alias must fail");
        assert!(err.to_string().contains("duplicate"));
    }
}
