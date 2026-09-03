//! Update Operation Planner
//!
//! Query planning for processing UPDATE VERTEX/EDGE statements

use crate::binder::BoundStatement;
use crate::parser::ast::{Stmt, UpdateStmt, UpdateTarget};
use crate::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{EdgeUpdateInfo, UpdateNode, UpdateTargetType, VertexUpdateInfo},
};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::planning::statements::clauses::exists_planner;
use crate::QueryContext;
use graphdb_core::types::{ContextualExpression, ExpressionMeta};
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
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        let update = match bound {
            BoundStatement::Update(u) => u,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "Statement does not contain UPDATE".to_string(),
                ));
            }
        };

        let space_name = qctx
            .space_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "default".to_string());

        let expr_ctx = Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let update_target = match &update.target {
            crate::binder::bound::BoundUpdateTarget::Vertex(vid) => {
                let vertex_id =
                    crate::binder::expr_converter::bound_expr_to_contextual(vid, &expr_ctx)
                        .map_err(PlannerError::PlanGenerationFailed)?;

                let mut properties = HashMap::new();
                for assignment in &update.assignments {
                    let value = crate::binder::expr_converter::bound_expr_to_contextual(
                        &assignment.value,
                        &expr_ctx,
                    )
                    .map_err(PlannerError::PlanGenerationFailed)?;
                    properties.insert(assignment.property.clone(), value);
                }

                let condition = update
                    .where_clause
                    .as_ref()
                    .map(|wc| {
                        crate::binder::expr_converter::bound_expr_to_contextual(wc, &expr_ctx)
                            .map_err(PlannerError::PlanGenerationFailed)
                    })
                    .transpose()?;

                let vertex_info = VertexUpdateInfo {
                    space_name,
                    vertex_id,
                    tag_name: None,
                    properties,
                    condition,
                    is_upsert: update.is_upsert,
                };
                UpdateTargetType::Vertex(vertex_info)
            }
            crate::binder::bound::BoundUpdateTarget::Edge(edge) => {
                let crate::binder::bound::BoundEdgeUpdateTarget {
                    src,
                    dst,
                    edge_type,
                    rank,
                } = &**edge;
                let src_ctx =
                    crate::binder::expr_converter::bound_expr_to_contextual(src, &expr_ctx)
                        .map_err(PlannerError::PlanGenerationFailed)?;
                let dst_ctx =
                    crate::binder::expr_converter::bound_expr_to_contextual(dst, &expr_ctx)
                        .map_err(PlannerError::PlanGenerationFailed)?;
                let rank_ctx = rank
                    .as_ref()
                    .map(|r| {
                        crate::binder::expr_converter::bound_expr_to_contextual(r, &expr_ctx)
                            .map_err(PlannerError::PlanGenerationFailed)
                    })
                    .transpose()?;

                let mut properties = HashMap::new();
                for assignment in &update.assignments {
                    let value = crate::binder::expr_converter::bound_expr_to_contextual(
                        &assignment.value,
                        &expr_ctx,
                    )
                    .map_err(PlannerError::PlanGenerationFailed)?;
                    properties.insert(assignment.property.clone(), value);
                }

                let condition = update
                    .where_clause
                    .as_ref()
                    .map(|wc| {
                        crate::binder::expr_converter::bound_expr_to_contextual(wc, &expr_ctx)
                            .map_err(PlannerError::PlanGenerationFailed)
                    })
                    .transpose()?;

                let edge_info = EdgeUpdateInfo {
                    space_name,
                    src: src_ctx,
                    dst: dst_ctx,
                    edge_type: edge_type.clone(),
                    rank: rank_ctx,
                    properties,
                    condition,
                    is_upsert: update.is_upsert,
                };
                UpdateTargetType::Edge(edge_info)
            }
            crate::binder::bound::BoundUpdateTarget::Tag(tag_name) => {
                let mut properties = HashMap::new();
                for assignment in &update.assignments {
                    let value = crate::binder::expr_converter::bound_expr_to_contextual(
                        &assignment.value,
                        &expr_ctx,
                    )
                    .map_err(PlannerError::PlanGenerationFailed)?;
                    properties.insert(assignment.property.clone(), value);
                }

                let placeholder_meta = ExpressionMeta::new(graphdb_core::Expression::Variable(
                    "_tag_placeholder".to_string(),
                ));
                let placeholder_id = expr_ctx.register_expression(placeholder_meta);

                let mut scan_node =
                    crate::planning::plan::core::nodes::ScanVerticesNode::new(0, &space_name);
                scan_node.set_tag(tag_name);

                let vertex_info = VertexUpdateInfo {
                    space_name: space_name.clone(),
                    vertex_id: ContextualExpression::new(placeholder_id, expr_ctx.clone()),
                    tag_name: Some(tag_name.clone()),
                    properties,
                    condition: None,
                    is_upsert: update.is_upsert,
                };

                let update_node =
                    UpdateNode::new(next_node_id(), UpdateTargetType::Vertex(vertex_info));

                let scan_enum = PlanNodeEnum::ScanVertices(scan_node);
                let update_enum = PlanNodeEnum::Update(update_node);

                let sub_plan = SubPlan::new(Some(scan_enum), Some(update_enum));
                return Ok(sub_plan);
            }
            crate::binder::bound::BoundUpdateTarget::TagOnVertex { vid, tag_name } => {
                let vid_ctx =
                    crate::binder::expr_converter::bound_expr_to_contextual(vid, &expr_ctx)
                        .map_err(PlannerError::PlanGenerationFailed)?;

                let mut properties = HashMap::new();
                for assignment in &update.assignments {
                    let value = crate::binder::expr_converter::bound_expr_to_contextual(
                        &assignment.value,
                        &expr_ctx,
                    )
                    .map_err(PlannerError::PlanGenerationFailed)?;
                    properties.insert(assignment.property.clone(), value);
                }

                let vertex_info = VertexUpdateInfo {
                    space_name,
                    vertex_id: vid_ctx,
                    tag_name: Some(tag_name.clone()),
                    properties,
                    condition: None,
                    is_upsert: update.is_upsert,
                };
                UpdateTargetType::Vertex(vertex_info)
            }
        };

        let update_node = UpdateNode::new(next_node_id(), update_target);
        let update_node_enum = PlanNodeEnum::Update(update_node);
        let sub_plan = SubPlan::new(Some(update_node_enum.clone()), Some(update_node_enum));
        Ok(sub_plan)
    }

    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let update_stmt = self.extract_update_stmt(validated.stmt())?;

        // Unified entry for expression-level EXISTS / IN: subqueries in
        // UPDATE SET values or the UPDATE WHERE condition are rejected at
        // planning time with a precise error.
        let check_space_id = qctx.space_id().unwrap_or(1);
        let check_space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let outer_col_names: Vec<String> = Vec::new();
        for assignment in &update_stmt.set_clause.assignments {
            if let Some(expr_meta) = assignment.value.expression() {
                exists_planner::check_expression_subqueries(
                    expr_meta.inner(),
                    &qctx,
                    check_space_id,
                    &check_space_name,
                    &outer_col_names,
                )?;
            }
        }
        if let Some(where_cond) = &update_stmt.where_clause {
            if let Some(expr_meta) = where_cond.expression() {
                exists_planner::check_expression_subqueries(
                    expr_meta.inner(),
                    &qctx,
                    check_space_id,
                    &check_space_name,
                    &outer_col_names,
                )?;
            }
        }

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
                    crate::planning::plan::core::nodes::ScanVerticesNode::new(0, &space_name);
                scan_node.set_tag(tag_name);

                let vertex_info = VertexUpdateInfo {
                    space_name: space_name.clone(),
                    vertex_id: ContextualExpression::new(
                        graphdb_core::types::expr::ExpressionId::new(0),
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
