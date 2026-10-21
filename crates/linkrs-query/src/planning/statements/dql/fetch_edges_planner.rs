//! The FETCH EDGES query planner
//! Planning for the execution of the FETCH EDGES query

use crate::binder::BoundStatement;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{GetEdgesNode, PlanNodeEnum};
use crate::planning::plan::execution_plan::SubPlan;
use crate::planning::plan::logical::logical_nodes::access::LogicalGetEdgesNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::planner::{Planner, PlannerError};
use crate::planning::statements::projection_util;

/// The FETCH EDGES query planner
/// Responsible for converting the FETCH EDGES query into an execution plan.
#[derive(Debug, Clone)]
pub struct FetchEdgesPlanner;

impl FetchEdgesPlanner {
    /// Create a new FETCH EDGES planner.
    pub fn new() -> Self {
        Self
    }
}

impl Planner for FetchEdgesPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let fetch = match bound {
            BoundStatement::FetchEdges(f) => f,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "FetchEdgesPlanner requires the FetchEdges statement.".to_string(),
                ));
            }
        };

        let expr_ctx = std::sync::Arc::new(
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let src_ctx =
            crate::binder::expr_converter::bound_expr_to_contextual(&fetch.src, &expr_ctx)
                .map_err(PlannerError::InvalidOperation)?;
        let src_str =
            linkrs_core::types::expr::expression_utils::extract_string_from_expr(&src_ctx)
                .map_err(PlannerError::InvalidOperation)?;

        let dst_ctx =
            crate::binder::expr_converter::bound_expr_to_contextual(&fetch.dst, &expr_ctx)
                .map_err(PlannerError::InvalidOperation)?;
        let dst_str =
            linkrs_core::types::expr::expression_utils::extract_string_from_expr(&dst_ctx)
                .map_err(PlannerError::InvalidOperation)?;

        let rank_str = match &fetch.rank {
            Some(rank_expr) => {
                let rank_ctx =
                    crate::binder::expr_converter::bound_expr_to_contextual(rank_expr, &expr_ctx)
                        .map_err(PlannerError::InvalidOperation)?;
                linkrs_core::types::expr::expression_utils::extract_string_from_expr(&rank_ctx)
                    .unwrap_or_else(|_| "0".to_string())
            }
            None => "0".to_string(),
        };

        let get_edges_node = PlanNodeEnum::GetEdges(GetEdgesNode::new(
            1,
            &src_str,
            &fetch.edge_type,
            &rank_str,
            &dst_str,
        ));
        let current_logical = match &get_edges_node {
            PlanNodeEnum::GetEdges(node) => get_edges_mirror(node),
            _ => unreachable!("fetch edges root is always a GetEdges node"),
        };

        let sub_plan = SubPlan {
            root: Some(get_edges_node),
            tail: None,
            logical_root: Some(current_logical),
        };
        Ok(sub_plan)
    }
}

impl Default for FetchEdgesPlanner {
    fn default() -> Self {
        Self::new()
    }
}

/// Mirror a physical edge lookup on the native logical tree.
fn get_edges_mirror(get_edges_node: &GetEdgesNode) -> LogicalNodeEnum {
    LogicalNodeEnum::GetEdges(LogicalGetEdgesNode {
        id: next_node_id(),
        space_id: get_edges_node.space_id(),
        edge_ref: get_edges_node.edge_ref().clone(),
        src: get_edges_node.src().to_string(),
        edge_type: get_edges_node.edge_type().to_string(),
        rank: get_edges_node.rank().to_string(),
        dst: get_edges_node.dst().to_string(),
        edge_props: get_edges_node.edge_props().to_vec(),
        expression: get_edges_node.filter().cloned(),
        dedup: get_edges_node.dedup(),
        limit: get_edges_node.limit(),
        output_var: get_edges_node.output_var().map(|s| s.to_string()),
        col_names: get_edges_node.col_names().to_vec(),
        column_types: projection_util::unknown_column_types(get_edges_node.col_names().len()),
    })
}
