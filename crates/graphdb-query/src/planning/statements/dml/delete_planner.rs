//! Deletion Operation Planner
//!
//! Query planning for handling DELETE VERTEX/EDGE/TAG statements.
//! Supports both standalone deletion and pipe-based deletion (e.g., GO ... | DELETE VERTEX $-.id).

use crate::binder::BoundStatement;
use crate::parser::ast::{DeleteStmt, DeleteTarget, Stmt};
use crate::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{
        DeleteEdgesNode, DeleteIndexNode, DeleteTagsNode, DeleteVerticesNode, EdgeDeleteInfo,
        IndexDeleteInfo, PipeDeleteEdgesNode, PipeDeleteVerticesNode, TagDeleteInfo,
        VertexDeleteInfo,
    },
};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::planning::statements::clauses::exists_planner;
use crate::QueryContext;
use std::sync::Arc;

/// Deletion Operation Planner
/// Responsible for converting DELETE statements into execution plans.
#[derive(Debug, Clone)]
pub struct DeletePlanner;

impl DeletePlanner {
    /// Create a new deletion planner.
    pub fn new() -> Self {
        Self
    }

    /// Extract the DeleteStmt from the Stmt.
    fn extract_delete_stmt(&self, stmt: &Stmt) -> Result<DeleteStmt, PlannerError> {
        match stmt {
            Stmt::Delete(delete_stmt) => Ok(delete_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain a DELETE".to_string(),
            )),
        }
    }
}

impl Planner for DeletePlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        let delete = match bound {
            BoundStatement::Delete(d) => d,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain a DELETE".to_string(),
                ));
            }
        };

        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        let expr_ctx = Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let final_node = match &delete.target {
            crate::binder::bound::BoundDeleteTarget::Vertices(vertex_ids) => {
                let converted_ids: Vec<graphdb_core::types::ContextualExpression> = vertex_ids
                    .iter()
                    .map(|id| {
                        crate::binder::expr_converter::bound_expr_to_contextual(id, &expr_ctx)
                            .map_err(PlannerError::PlanGenerationFailed)
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                let condition = delete
                    .where_clause
                    .as_ref()
                    .map(|wc| {
                        crate::binder::expr_converter::bound_expr_to_contextual(wc, &expr_ctx)
                            .map_err(PlannerError::PlanGenerationFailed)
                    })
                    .transpose()?;

                let info = VertexDeleteInfo {
                    space_name,
                    vertex_ids: converted_ids,
                    with_edge: delete.with_edge,
                    condition,
                };
                PlanNodeEnum::DeleteVertices(DeleteVerticesNode::new(next_node_id(), info))
            }
            crate::binder::bound::BoundDeleteTarget::Edges { edge_type, edges } => {
                let converted_edges: Vec<(
                    graphdb_core::types::ContextualExpression,
                    graphdb_core::types::ContextualExpression,
                    Option<graphdb_core::types::ContextualExpression>,
                )> = edges
                    .iter()
                    .map(|(src, dst, rank)| {
                        let s =
                            crate::binder::expr_converter::bound_expr_to_contextual(src, &expr_ctx)
                                .map_err(PlannerError::PlanGenerationFailed)?;
                        let d =
                            crate::binder::expr_converter::bound_expr_to_contextual(dst, &expr_ctx)
                                .map_err(PlannerError::PlanGenerationFailed)?;
                        let r = rank
                            .as_ref()
                            .map(|rk| {
                                crate::binder::expr_converter::bound_expr_to_contextual(
                                    rk, &expr_ctx,
                                )
                                .map_err(PlannerError::PlanGenerationFailed)
                            })
                            .transpose()?;
                        Ok((s, d, r))
                    })
                    .collect::<Result<Vec<_>, PlannerError>>()?;

                let condition = delete
                    .where_clause
                    .as_ref()
                    .map(|wc| {
                        crate::binder::expr_converter::bound_expr_to_contextual(wc, &expr_ctx)
                            .map_err(PlannerError::PlanGenerationFailed)
                    })
                    .transpose()?;

                let info = EdgeDeleteInfo {
                    space_name,
                    edge_type: edge_type.clone(),
                    edges: converted_edges,
                    condition,
                };
                PlanNodeEnum::DeleteEdges(DeleteEdgesNode::new(next_node_id(), info))
            }
            crate::binder::bound::BoundDeleteTarget::Tags {
                tag_names,
                vertex_ids,
                is_all_tags,
            } => {
                let converted_ids: Vec<graphdb_core::types::ContextualExpression> = vertex_ids
                    .iter()
                    .map(|id| {
                        crate::binder::expr_converter::bound_expr_to_contextual(id, &expr_ctx)
                            .map_err(PlannerError::PlanGenerationFailed)
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                let info = TagDeleteInfo {
                    space_name,
                    tag_names: tag_names.clone(),
                    vertex_ids: converted_ids,
                    is_all_tags: *is_all_tags,
                };
                PlanNodeEnum::DeleteTags(DeleteTagsNode::new(next_node_id(), info))
            }
            crate::binder::bound::BoundDeleteTarget::Index(index_name) => {
                let info = IndexDeleteInfo {
                    space_name,
                    index_name: index_name.clone(),
                };
                PlanNodeEnum::DeleteIndex(DeleteIndexNode::new(next_node_id(), info))
            }
        };

        Ok(SubPlan::new(Some(final_node), None))
    }

    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        self.transform_with_input(validated, qctx, None)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Delete(_))
    }
}

impl DeletePlanner {
    /// Transform with optional input plan (for pipe DELETE)
    fn transform_with_input(
        &self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
        input_plan: Option<SubPlan>,
    ) -> Result<SubPlan, PlannerError> {
        let _ = qctx;

        let validation_info = &validated.validation_info;
        let referenced_tags = &validation_info.semantic_info.referenced_tags;
        if !referenced_tags.is_empty() {
            log::debug!("DELETE Referenced tags: {:?}", referenced_tags);
        }

        let referenced_edges = &validation_info.semantic_info.referenced_edges;
        if !referenced_edges.is_empty() {
            log::debug!("DELETE Referenced edge type: {:?}", referenced_edges);
        }

        let delete_stmt = self.extract_delete_stmt(validated.stmt())?;

        // Unified entry for expression-level EXISTS / IN: subqueries in the
        // DELETE WHERE condition are rejected at planning time with a precise
        // error.
        let check_space_id = qctx.space_id().unwrap_or(1);
        let check_space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let outer_col_names: Vec<String> = Vec::new();
        if let Some(where_cond) = &delete_stmt.where_clause {
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

        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        let input_node = input_plan.as_ref().and_then(|p| p.root.clone());

        let final_node = match &delete_stmt.target {
            DeleteTarget::Vertices(vertex_ids) => {
                let info = VertexDeleteInfo {
                    space_name,
                    vertex_ids: vertex_ids.clone(),
                    with_edge: delete_stmt.with_edge,
                    condition: delete_stmt.where_clause.clone(),
                };

                if let Some(input) = input_node {
                    let node = PipeDeleteVerticesNode::new(next_node_id(), info, input);
                    PlanNodeEnum::PipeDeleteVertices(node)
                } else {
                    let node = DeleteVerticesNode::new(next_node_id(), info);
                    PlanNodeEnum::DeleteVertices(node)
                }
            }
            DeleteTarget::Edges { edge_type, edges } => {
                let info = EdgeDeleteInfo {
                    space_name,
                    edge_type: edge_type.clone(),
                    edges: edges
                        .iter()
                        .map(|(src, dst, rank)| (src.clone(), dst.clone(), rank.clone()))
                        .collect(),
                    condition: delete_stmt.where_clause.clone(),
                };

                if let Some(input) = input_node {
                    let node = PipeDeleteEdgesNode::new(next_node_id(), info, input);
                    PlanNodeEnum::PipeDeleteEdges(node)
                } else {
                    let node = DeleteEdgesNode::new(next_node_id(), info);
                    PlanNodeEnum::DeleteEdges(node)
                }
            }
            DeleteTarget::Tags {
                tag_names,
                vertex_ids,
                is_all_tags,
            } => {
                let info = TagDeleteInfo {
                    space_name,
                    tag_names: tag_names.clone(),
                    vertex_ids: vertex_ids.clone(),
                    is_all_tags: *is_all_tags,
                };

                let node = DeleteTagsNode::new(next_node_id(), info);
                PlanNodeEnum::DeleteTags(node)
            }
            DeleteTarget::Index(index_name) => {
                let info = IndexDeleteInfo {
                    space_name,
                    index_name: index_name.clone(),
                };

                let node = DeleteIndexNode::new(next_node_id(), info);
                PlanNodeEnum::DeleteIndex(node)
            }
        };

        let sub_plan = SubPlan::new(Some(final_node), None);

        Ok(sub_plan)
    }
}

impl Default for DeletePlanner {
    fn default() -> Self {
        Self::new()
    }
}
