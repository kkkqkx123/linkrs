//! Attribute/Tag Remover Planner
//!
//! Query planning for handling the REMOVE statement

use crate::core::types::ContextualExpression;
use crate::core::YieldColumn;
use crate::query::parser::ast::{RemoveStmt, Stmt};
use crate::query::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{ArgumentNode, ProjectNode, RemoveNode},
};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
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

        // Create a parameter node as input.
        let arg_node = ArgumentNode::new(next_node_id(), "remove_input");
        let arg_node_enum = PlanNodeEnum::Argument(arg_node.clone());

        // Analyze the REMOVE item to determine whether it refers to the deletion of an attribute or a tag.
        let mut remove_items = Vec::new();
        for item in &remove_stmt.items {
            // Determine whether it is an attribute or a tag based on the type of the expression.
            let expr = item.get_expression();
            if let Some(expression) = expr {
                let item_type = match expression {
                    crate::core::Expression::Property { .. } => "property",
                    crate::core::Expression::Label { .. } => "tag",
                    _ => "property",
                };
                remove_items.push((item_type.to_string(), item.clone()));
            }
        }

        // Create a Remove node
        let remove_node = RemoveNode::new(arg_node_enum.clone(), remove_items).map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create RemoveNode: {}", e))
        })?;

        let remove_node_enum = PlanNodeEnum::Remove(remove_node);

        // Build the output column – Return the number of attributes/tagging elements that were deleted.
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(
            crate::core::Expression::Variable("removed_count".to_string()),
        );
        let id = validated.expr_context().register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, validated.expr_context().clone());

        let yield_columns = vec![YieldColumn {
            expression: ctx_expr,
            alias: "removed_count".to_string(),
            is_matched: false,
        }];

        // Create a projection node to output the deletion results.
        let project_node =
            ProjectNode::new(remove_node_enum.clone(), yield_columns).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
            })?;

        let final_node = PlanNodeEnum::Project(project_node);

        // Create a SubPlan
        let sub_plan = SubPlan::new(Some(final_node), Some(arg_node_enum));

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
