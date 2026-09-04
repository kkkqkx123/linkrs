//! Attribute/Tag Remover Planner
//!
//! Query planning for handling the REMOVE statement
//!
//! Migrated to generate a native LogicalNodeEnum tree; `from_logical_root`
//! performs the one-shot logical → physical lowering so the optimizer sees
//! the logical mirror.

use crate::binder::BoundStatement;
use crate::parser::ast::{RemoveStmt, Stmt};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode;
use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalRemoveNode;
use crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use graphdb_core::types::ContextualExpression;
use graphdb_core::YieldColumn;
use std::sync::Arc;

/// Attribute/Tag Remover Planner
/// Responsible for converting the REMOVE statement into an execution plan.
#[derive(Debug, Clone)]
pub struct RemovePlanner;

impl RemovePlanner {
    /// Create a new deletion planner.
    pub fn new() -> Self {
        Self
    }

    /// Extract the `RemoveStmt` from the `Stmt`.
    fn extract_remove_stmt(&self, stmt: &Stmt) -> Result<RemoveStmt, PlannerError> {
        match stmt {
            Stmt::Remove(remove_stmt) => Ok(remove_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain a REMOVE".to_string(),
            )),
        }
    }
}

impl Planner for RemovePlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        let remove = match bound {
            BoundStatement::Remove(r) => r,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain a REMOVE".to_string(),
                ));
            }
        };

        let expr_ctx = Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let mut remove_items = Vec::new();
        for item in &remove.items {
            let item_type = match item {
                crate::binder::bound::BoundExpression::Property { .. } => "property",
                crate::binder::bound::BoundExpression::Label { .. } => "tag",
                _ => "property",
            };
            let ctx_expr = crate::binder::expr_converter::bound_expr_to_contextual(item, &expr_ctx)
                .map_err(PlannerError::PlanGenerationFailed)?;
            remove_items.push((item_type.to_string(), ctx_expr));
        }

        let arg_logical = LogicalNodeEnum::Argument(LogicalArgumentNode {
            id: next_node_id(),
            var: "remove_input".to_string(),
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });

        let logical_remove = LogicalNodeEnum::Remove(LogicalRemoveNode {
            id: next_node_id(),
            input: Some(Box::new(arg_logical)),
            deps: vec![],
            remove_items,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });

        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(
            graphdb_core::Expression::Variable("removed_count".to_string()),
        );
        let id = expr_ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);

        let yield_columns = vec![YieldColumn {
            expression: ctx_expr,
            alias: "removed_count".to_string(),
            is_matched: false,
        }];

        let logical_project = LogicalNodeEnum::Project(LogicalProjectNode {
            id: next_node_id(),
            input: Some(Box::new(logical_remove)),
            deps: vec![],
            columns: yield_columns,
            output_var: None,
            col_names: vec!["removed_count".to_string()],
            column_types: vec![],
        });

        let mut sub_plan = SubPlan::from_logical_root(logical_project);
        let arg_node =
            crate::planning::plan::core::nodes::ArgumentNode::new(next_node_id(), "remove_input");
        sub_plan.set_tail(PlanNodeEnum::Argument(arg_node));
        Ok(sub_plan)
    }

    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let _ = qctx;

        // Use the verification information to optimize the planning process.
        let validation_info = &validated.validation_info;

        // Check the semantic information.
        let referenced_tags = &validation_info.semantic_info.referenced_tags;
        if !referenced_tags.is_empty() {
            log::debug!("REMOVE Referenced tags: {:?}", referenced_tags);
        }

        let referenced_properties = &validation_info.semantic_info.referenced_properties;
        if !referenced_properties.is_empty() {
            log::debug!("REMOVE Referenced properties: {:?}", referenced_properties);
        }

        let remove_stmt = self.extract_remove_stmt(validated.stmt())?;

        let arg_logical = LogicalNodeEnum::Argument(LogicalArgumentNode {
            id: next_node_id(),
            var: "remove_input".to_string(),
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });

        // Analyze the REMOVE item to determine whether it refers to the deletion of an attribute or a tag.
        let mut remove_items = Vec::new();
        for item in &remove_stmt.items {
            // Determine whether it is an attribute or a tag based on the type of the expression.
            let expr = item.get_expression();
            if let Some(expression) = expr {
                let item_type = match expression {
                    graphdb_core::Expression::Property { .. } => "property",
                    graphdb_core::Expression::Label { .. } => "tag",
                    _ => "property",
                };
                remove_items.push((item_type.to_string(), item.clone()));
            }
        }

        let logical_remove = LogicalNodeEnum::Remove(LogicalRemoveNode {
            id: next_node_id(),
            input: Some(Box::new(arg_logical)),
            deps: vec![],
            remove_items,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });

        // Build the output column – Return the number of attributes/tagging elements that were deleted.
        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(
            graphdb_core::Expression::Variable("removed_count".to_string()),
        );
        let id = validated.expr_context().register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, validated.expr_context().clone());

        let yield_columns = vec![YieldColumn {
            expression: ctx_expr,
            alias: "removed_count".to_string(),
            is_matched: false,
        }];

        let logical_project = LogicalNodeEnum::Project(LogicalProjectNode {
            id: next_node_id(),
            input: Some(Box::new(logical_remove)),
            deps: vec![],
            columns: yield_columns,
            output_var: None,
            col_names: vec!["removed_count".to_string()],
            column_types: vec![],
        });

        let mut sub_plan = SubPlan::from_logical_root(logical_project);
        let arg_node =
            crate::planning::plan::core::nodes::ArgumentNode::new(next_node_id(), "remove_input");
        sub_plan.set_tail(PlanNodeEnum::Argument(arg_node));
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Remove(_))
    }
}

impl Default for RemovePlanner {
    fn default() -> Self {
        Self::new()
    }
}
