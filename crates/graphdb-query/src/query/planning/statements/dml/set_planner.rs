//! SET Operation Planner
//!
//! Query planning for processing SET statements (set properties on vertices/edges)

use crate::core::types::{ContextualExpression, ExpressionMeta};
use crate::core::Expression;
use crate::query::parser::ast::{SetStmt, Stmt};
use crate::query::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{UpdateNode, UpdateTargetType, VertexUpdateInfo},
};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
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
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let set_stmt = self.extract_set_stmt(validated.stmt())?;

        // Get current space name from query context
        let space_name = qctx
            .space_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "default".to_string());

        // Check if this is a direct vertex property update (e.g., SET 1.age = 31)
        // or a variable-based update (e.g., SET p.age = 26)
        let mut vertex_updates: Vec<VertexUpdateInfo> = Vec::new();
        let mut variable_assignments: Vec<(String, ContextualExpression)> = Vec::new();

        for assignment in &set_stmt.assignments {
            if let Some(ref target) = assignment.target {
                // This is a direct property access like "1.age"
                // Extract vertex ID from the target expression
                if let Some(vertex_id_expr) = self.extract_vertex_id(target) {
                    // Create properties map
                    let mut properties = HashMap::new();
                    properties.insert(assignment.property.clone(), assignment.value.clone());

                    let vertex_update = VertexUpdateInfo {
                        space_name: space_name.clone(),
                        vertex_id: vertex_id_expr,
                        tag_name: None, // Will be determined at execution time
                        properties,
                        condition: None,
                        is_upsert: false,
                    };
                    vertex_updates.push(vertex_update);
                } else {
                    // Fallback to variable assignment
                    variable_assignments
                        .push((assignment.property.clone(), assignment.value.clone()));
                }
            } else {
                // This is a variable assignment like "p.age" where p is a variable
                variable_assignments.push((assignment.property.clone(), assignment.value.clone()));
            }
        }

        // If we have vertex updates, create an UpdateNode
        if !vertex_updates.is_empty() {
            // For simplicity, handle the first vertex update
            // In a full implementation, we might want to batch all updates
            let update_info = if vertex_updates.len() == 1 {
                UpdateTargetType::Vertex(vertex_updates.into_iter().next().unwrap())
            } else {
                // For multiple vertices, we would need to create a batch update
                // For now, just use the first one
                UpdateTargetType::Vertex(vertex_updates.into_iter().next().unwrap())
            };

            let update_node = UpdateNode::new(next_node_id(), update_info);
            let update_node_enum = PlanNodeEnum::Update(update_node);

            let sub_plan = SubPlan::new(Some(update_node_enum.clone()), Some(update_node_enum));
            return Ok(sub_plan);
        }

        // If no vertex updates, fall back to AssignNode for variable assignments
        if !variable_assignments.is_empty() {
            use crate::query::planning::plan::core::nodes::{ArgumentNode, AssignNode};

            let arg_node = ArgumentNode::new(next_node_id(), "set_input");
            let arg_node_enum = PlanNodeEnum::Argument(arg_node.clone());

            let assign_node = AssignNode::new(arg_node_enum.clone(), variable_assignments)
                .map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create AssignNode: {}",
                        e
                    ))
                })?;

            let assign_node_enum = PlanNodeEnum::Assign(assign_node);
            let sub_plan = SubPlan::new(Some(assign_node_enum.clone()), Some(assign_node_enum));
            return Ok(sub_plan);
        }

        // If no assignments at all, return an empty plan
        let arg_node = crate::query::planning::plan::core::nodes::ArgumentNode::new(
            next_node_id(),
            "set_input",
        );
        let arg_node_enum = PlanNodeEnum::Argument(arg_node.clone());
        let sub_plan = SubPlan::new(Some(arg_node_enum.clone()), Some(arg_node_enum));
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Set(_))
    }
}

impl Default for SetPlanner {
    fn default() -> Self {
        Self::new()
    }
}
