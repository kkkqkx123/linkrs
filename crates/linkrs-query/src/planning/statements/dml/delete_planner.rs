//! Deletion Operation Planner
//!
//! Query planning for handling DELETE VERTEX/EDGE/TAG statements.
//! Supports both standalone deletion and pipe-based deletion (e.g., GO ... | DELETE VERTEX $-.id).
//!
//! Migrated to generate a native LogicalNodeEnum tree; `from_logical_root`
//! performs the one-shot logical → physical lowering so the optimizer sees
//! the logical mirror.

use crate::binder::BoundStatement;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{EdgeDeleteInfo, IndexDeleteInfo, VertexDeleteInfo};
use crate::planning::plan::logical::logical_nodes::dml::{
    LogicalDeleteEdgesNode, LogicalDeleteIndexNode, LogicalDeleteVerticesNode,
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
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
}

impl Planner for DeletePlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
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
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let final_node = match &delete.target {
            crate::binder::bound::BoundDeleteTarget::Vertices { tag, ids } => {
                let converted_ids: Vec<linkrs_core::types::ContextualExpression> = ids
                    .iter()
                    .map(|id| {
                        crate::binder::expr_converter::bound_expr_to_contextual(id, &expr_ctx)
                            .map_err(PlannerError::PlanGenerationFailed)
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                let info = VertexDeleteInfo {
                    space_name,
                    tag: Some(tag.clone()),
                    vertex_ids: converted_ids,
                    with_edge: delete.with_edge,
                    cascade: delete.with_edge || delete.detach,
                };
                LogicalNodeEnum::DeleteVertices(LogicalDeleteVerticesNode {
                    id: next_node_id(),
                    info,
                    output_var: None,
                    col_names: vec!["deleted".to_string()],
                    column_types: vec![],
                })
            }
            crate::binder::bound::BoundDeleteTarget::Edges { edge_type, edges } => {
                let converted_edges: Vec<(
                    linkrs_core::types::ContextualExpression,
                    linkrs_core::types::ContextualExpression,
                    Option<linkrs_core::types::ContextualExpression>,
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

                let info = EdgeDeleteInfo {
                    space_name,
                    edge_type: edge_type.clone(),
                    edges: converted_edges,
                };
                LogicalNodeEnum::DeleteEdges(LogicalDeleteEdgesNode {
                    id: next_node_id(),
                    info,
                    output_var: None,
                    col_names: vec!["deleted".to_string()],
                    column_types: vec![],
                })
            }
            crate::binder::bound::BoundDeleteTarget::Index(index_name) => {
                let info = IndexDeleteInfo {
                    space_name,
                    index_name: index_name.clone(),
                };
                LogicalNodeEnum::DeleteIndex(LogicalDeleteIndexNode {
                    id: next_node_id(),
                    info,
                    output_var: None,
                    col_names: vec!["deleted".to_string()],
                    column_types: vec![],
                })
            }
        };

        Ok(SubPlan::from_logical_root(final_node))
    }
}

impl Default for DeletePlanner {
    fn default() -> Self {
        Self::new()
    }
}
