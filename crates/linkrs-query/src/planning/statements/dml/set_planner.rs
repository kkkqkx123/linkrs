//! SET Operation Planner
//!
//! Query planning for processing SET statements (set properties on vertices/edges)
//!
//! Migrated to generate a native LogicalNodeEnum tree; `from_logical_root`
//! performs the one-shot logical → physical lowering so the optimizer sees
//! the logical mirror.

use crate::binder::BoundStatement;
use crate::planning::plan::core::nodes::VertexUpdateInfo;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
use linkrs_core::types::ContextualExpression;
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
}

impl Planner for SetPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
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

        let space_id = qctx.space_id().unwrap_or(1);
        let reject_subquery = |expr: &ContextualExpression| -> Result<(), PlannerError> {
            if let Some(expr_meta) = expr.expression() {
                crate::planning::statements::clauses::exists_planner::check_expression_subqueries(
                    expr_meta.inner(),
                    &qctx,
                    space_id,
                    &space_name,
                    &[],
                )?;
            }
            Ok(())
        };

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
                        reject_subquery(&value)?;

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
                    reject_subquery(&value)?;
                    variable_assignments.push((assignment.property.clone(), value));
                }
            } else {
                let value = crate::binder::expr_converter::bound_expr_to_contextual(
                    &assignment.value,
                    &expr_ctx,
                )
                .map_err(PlannerError::PlanGenerationFailed)?;
                reject_subquery(&value)?;
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
