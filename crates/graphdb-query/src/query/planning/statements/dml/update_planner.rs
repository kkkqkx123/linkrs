//! Update Operation Planner
//!
//! Query planning for processing UPDATE VERTEX/EDGE statements

use crate::core::types::ContextualExpression;
use crate::query::parser::ast::{Stmt, UpdateStmt, UpdateTarget};
use crate::query::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{EdgeUpdateInfo, UpdateNode, UpdateTargetType, VertexUpdateInfo},
};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use std::collections::HashMap;
use std::sync::Arc;

/// Update Operation Planner
/// Responsible for converting UPDATE statements into execution plans.
#[derive(Debug, Clone)]
pub struct UpdatePlanner;

impl UpdatePlanner {
    /// Create a new update planner.
    pub fn new() -> Self {
        Self
    }

    /// Extract the UpdateStmt from the Stmt.
    fn extract_update_stmt(&self, stmt: &Stmt) -> Result<UpdateStmt, PlannerError> {
        match stmt {
            Stmt::Update(update_stmt) => Ok(update_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "Statement does not contain UPDATE".to_string(),
            )),
        }
    }

    /// Build vertex update info from UPDATE statement
    fn build_vertex_update_info(
        &self,
        update_stmt: &UpdateStmt,
        vertex_id: ContextualExpression,
        space_name: String,
    ) -> Result<VertexUpdateInfo, PlannerError> {
        // Convert assignments to properties HashMap
        let mut properties = HashMap::new();
        for assignment in &update_stmt.set_clause.assignments {
            properties.insert(assignment.property.clone(), assignment.value.clone());
        }

        Ok(VertexUpdateInfo {
            space_name,
            vertex_id,
            tag_name: None, // Will be determined at execution time
            properties,
            condition: update_stmt.where_clause.clone(),
            is_upsert: update_stmt.is_upsert,
        })
    }

    /// Build edge update info from UPDATE statement
    fn build_edge_update_info(
        &self,
        update_stmt: &UpdateStmt,
        src: ContextualExpression,
        dst: ContextualExpression,
        edge_type: Option<String>,
        rank: Option<ContextualExpression>,
        space_name: String,
    ) -> Result<EdgeUpdateInfo, PlannerError> {
        // Convert assignments to properties HashMap
        let mut properties = HashMap::new();
        for assignment in &update_stmt.set_clause.assignments {
            properties.insert(assignment.property.clone(), assignment.value.clone());
        }

        Ok(EdgeUpdateInfo {
            space_name,
            src,
            dst,
            edge_type,
            rank,
            properties,
            condition: update_stmt.where_clause.clone(),
            is_upsert: update_stmt.is_upsert,
        })
    }
}

impl Planner for UpdatePlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let update_stmt = self.extract_update_stmt(validated.stmt())?;

        // Get current space name from query context
        let space_name = qctx
            .space_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "default".to_string());

        // Build update target based on the update statement target
        let update_target = match &update_stmt.target {
            UpdateTarget::Vertex(vertex_id) => {
                let vertex_info =
                    self.build_vertex_update_info(&update_stmt, vertex_id.clone(), space_name)?;
                UpdateTargetType::Vertex(vertex_info)
            }
            UpdateTarget::Edge {
                src,
                dst,
                edge_type,
                rank,
            } => {
                let edge_info = self.build_edge_update_info(
                    &update_stmt,
                    src.clone(),
                    dst.clone(),
                    edge_type.clone(),
                    rank.clone(),
                    space_name,
                )?;
                UpdateTargetType::Edge(edge_info)
            }
            UpdateTarget::Tag(tag_name) => {
                let mut properties = HashMap::new();
                for assignment in &update_stmt.set_clause.assignments {
                    properties.insert(assignment.property.clone(), assignment.value.clone());
                }

                let mut scan_node =
                    crate::query::planning::plan::core::nodes::ScanVerticesNode::new(
                        0,
                        &space_name,
                    );
                scan_node.set_tag(tag_name);

                let vertex_info = VertexUpdateInfo {
                    space_name: space_name.clone(),
                    vertex_id: ContextualExpression::new(
                        crate::core::types::expr::ExpressionId::new(0),
                        validated.ast.expr_context().clone(),
                    ),
                    tag_name: Some(tag_name.clone()),
                    properties,
                    condition: update_stmt.where_clause.clone(),
                    is_upsert: update_stmt.is_upsert,
                };

                let update_node =
                    UpdateNode::new(next_node_id(), UpdateTargetType::Vertex(vertex_info));

                let scan_enum = PlanNodeEnum::ScanVertices(scan_node);
                let update_enum = PlanNodeEnum::Update(update_node);

                let sub_plan = SubPlan::new(Some(scan_enum), Some(update_enum));
                return Ok(sub_plan);
            }
            UpdateTarget::TagOnVertex { vid, tag_name } => {
                // Update specific tag on a specific vertex
                let mut properties = HashMap::new();
                for assignment in &update_stmt.set_clause.assignments {
                    properties.insert(assignment.property.clone(), assignment.value.clone());
                }

                let vertex_info = VertexUpdateInfo {
                    space_name,
                    vertex_id: *vid.clone(),
                    tag_name: Some(tag_name.clone()),
                    properties,
                    condition: update_stmt.where_clause.clone(),
                    is_upsert: update_stmt.is_upsert,
                };
                UpdateTargetType::Vertex(vertex_info)
            }
        };

        // Create the UpdateNode
        let update_node = UpdateNode::new(next_node_id(), update_target);
        let update_node_enum = PlanNodeEnum::Update(update_node);

        // Create a SubPlan with the update node as the final node
        let sub_plan = SubPlan::new(Some(update_node_enum.clone()), Some(update_node_enum));

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Update(_))
    }
}

impl Default for UpdatePlanner {
    fn default() -> Self {
        Self::new()
    }
}
