//! SET Operation Planner
//!
//! Query planning for processing SET statements (set properties on vertices/edges)
//!
//! Migrated to generate a native LogicalNodeEnum tree; `from_logical_root`
//! performs the one-shot logical → physical lowering so the optimizer sees
//! the logical mirror.

use crate::binder::BoundStatement;
use crate::parser::ast::{SetStmt, Stmt};
use crate::planning::plan::core::nodes::VertexUpdateInfo;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::planning::statements::clauses::exists_planner;
use crate::QueryContext;
use linkrs_core::types::{ContextualExpression, ExpressionMeta};
use linkrs_core::Expression;
use std::collections::HashMap;
use std::sync::Arc;

/// SET Operation Planner
/// Responsible for converting SET statements into execution plans.
#[derive(Debug, Clone)]
pub struct SetPlanner;

impl SetPlanner {
    /// Create a new SET planner.
    pub fn new() -> Self {
        Self
    }

    /// Extract the SetStmt from the Stmt.
    fn extract_set_stmt(&self, stmt: &Stmt) -> Result<SetStmt, PlannerError> {
        match stmt {
            Stmt::Set(set_stmt) => Ok(set_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "Statement does not contain SET".to_string(),
            )),
        }
    }

    /// Extract vertex ID from target expression
    /// For expressions like "1.age", extract the vertex ID "1" as a new ContextualExpression
    fn extract_vertex_id(&self, target: &ContextualExpression) -> Option<ContextualExpression> {
        let expr_meta = target.expression()?;
        let expr = expr_meta.inner();

        match expr {
            Expression::Property { object, .. } => {
                // The object should be the vertex ID
                match object.as_ref() {
                    Expression::Literal(_) | Expression::Variable(_) => {
                        // Create a new ExpressionMeta with just the object
                        let object_meta = ExpressionMeta::new((**object).clone());
                        // Register the expression in the context
                        let context = target.context();
                        let object_id = context.register_expression(object_meta);
                        // Create a new ContextualExpression with the object ID
                        Some(ContextualExpression::new(object_id, context.clone()))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

impl Planner for SetPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let set = match bound {
            BoundStatement::Set(s) => s,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "Statement does not contain SET".to_string(),
                ));
            }
        };

        // Unified entry for expression-level EXISTS / IN (mirrors
        // `transform`): the bound path must also reject subqueries in SET
        // values at planning time with the precise error.
        let space_name = qctx
            .space_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "default".to_string());

        let expr_ctx = Arc::new(
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let mut vertex_updates: Vec<VertexUpdateInfo> = Vec::new();
        let mut variable_assignments: Vec<(String, ContextualExpression)> = Vec::new();

        for assignment in &set.assignments {
            if let Some(ref target) = assignment.target {
                // Check if target is a direct property access like "1.age"
                // i.e. BoundExpression::Property { object: Literal/Variable, .. }
                let is_direct_vertex = match target {
                    crate::binder::bound::BoundExpression::Property { object, .. } => {
                        matches!(
                            object.as_ref(),
                            crate::binder::bound::BoundExpression::Literal(_, _)
                                | crate::binder::bound::BoundExpression::Variable(_, _)
                        )
                    }
                    _ => false,
                };

                if is_direct_vertex {
                    // Extract vertex ID from the Property's object
                    if let crate::binder::bound::BoundExpression::Property { object, .. } = target {
                        let vertex_id = crate::binder::expr_converter::bound_expr_to_contextual(
                            object, &expr_ctx,
                        )
                        .map_err(PlannerError::PlanGenerationFailed)?;

                        let value = crate::binder::expr_converter::bound_expr_to_contextual(
                            &assignment.value,
                            &expr_ctx,
                        )
                        .map_err(PlannerError::PlanGenerationFailed)?;

                        let mut properties = HashMap::new();
                        properties.insert(assignment.property.clone(), value);

                        let vertex_update = VertexUpdateInfo {
                            space_name: space_name.clone(),
                            vertex_id,
                            tag_name: None,
                            properties,
                            condition: None,
                            is_upsert: false,
                            replace_properties: assignment.is_map_overwrite,
                        };
                        vertex_updates.push(vertex_update);
                    }
                } else {
                    let value = crate::binder::expr_converter::bound_expr_to_contextual(
                        &assignment.value,
                        &expr_ctx,
                    )
                    .map_err(PlannerError::PlanGenerationFailed)?;
                    variable_assignments.push((assignment.property.clone(), value));
                }
            } else {
                let value = crate::binder::expr_converter::bound_expr_to_contextual(
                    &assignment.value,
                    &expr_ctx,
                )
                .map_err(PlannerError::PlanGenerationFailed)?;
                variable_assignments.push((assignment.property.clone(), value));
            }
        }

        return Err(PlannerError::PlanGenerationFailed(
            "standalone SET cannot resolve a tag qualifier; use UPDATE ... ON <tag> SET instead"
                .to_string(),
        ));
    }


}

impl Default for SetPlanner {
    fn default() -> Self {
        Self::new()
    }
}
