//! The FETCH EDGES query planner
//! Planning for the execution of the FETCH EDGES query

use crate::binder::BoundStatement;
use crate::parser::ast::{FetchTarget, Stmt};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{GetEdgesNode, PlanNodeEnum, ProjectNode};
use crate::planning::plan::execution_plan::SubPlan;
use crate::planning::plan::logical::logical_nodes::access::LogicalGetEdgesNode;
use crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use graphdb_core::types::expr::expression_utils::extract_string_from_expr;
use std::sync::Arc;

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
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let _ = qctx;

        let fetch_stmt = match validated.stmt() {
            Stmt::Fetch(fetch_stmt) => fetch_stmt,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "FetchEdgesPlanner requires a Fetch statement.".to_string(),
                ));
            }
        };

        let (src, dst, edge_type, rank) = match &fetch_stmt.target {
            FetchTarget::Edges {
                src,
                dst,
                edge_type,
                rank,
                ..
            } => (src, dst, edge_type, rank),
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "FetchEdgesPlanner requires the FETCH EDGES statement.".to_string(),
                ));
            }
        };

        let src_str = extract_string_from_expr(src).map_err(PlannerError::InvalidOperation)?;
        let dst_str = extract_string_from_expr(dst).map_err(PlannerError::InvalidOperation)?;
        let rank_str = rank
            .as_ref()
            .map(extract_string_from_expr)
            .transpose()
            .map_err(PlannerError::InvalidOperation)?
            .unwrap_or_else(|| "0".to_string());

        let get_edges_node = GetEdgesNode::new(1, &src_str, edge_type, &rank_str, &dst_str);
        let get_edges_logical = get_edges_mirror(&get_edges_node);
        let get_edges_enum = PlanNodeEnum::GetEdges(get_edges_node);

        // Apply the YIELD clause as a projection when present.
        let (root, current_logical) = if let Some(ref yield_clause) = fetch_stmt.yield_clause {
            let mut columns = Vec::new();
            for item in &yield_clause.items {
                columns.push(graphdb_core::YieldColumn {
                    expression: item.expression.clone(),
                    alias: item.alias.clone().unwrap_or_default(),
                    is_matched: false,
                });
            }
            let project_node = ProjectNode::new(get_edges_enum, columns.clone())?;
            let root = PlanNodeEnum::Project(project_node);
            let logical = LogicalNodeEnum::Project(LogicalProjectNode {
                id: next_node_id(),
                input: Some(Box::new(get_edges_logical.clone())),
                deps: vec![get_edges_logical],
                columns,
                output_var: None,
                col_names: root.col_names().to_vec(),
                column_types: vec![],
            });
            (root, logical)
        } else {
            (get_edges_enum, get_edges_logical)
        };

        // For FETCH PROP ON EDGE with specific src/dst/rank, GetEdgesNode is sufficient
        // No need for additional Filter nodes
        let sub_plan = SubPlan {
            root: Some(root),
            tail: None,
            logical_root: Some(current_logical),
        };

        Ok(sub_plan)
    }

    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        let fetch = match bound {
            BoundStatement::FetchEdges(f) => f,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "FetchEdgesPlanner requires the FetchEdges statement.".to_string(),
                ));
            }
        };

        let expr_ctx = std::sync::Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let src_ctx =
            crate::binder::expr_converter::bound_expr_to_contextual(&fetch.src, &expr_ctx)
                .map_err(PlannerError::InvalidOperation)?;
        let src_str =
            graphdb_core::types::expr::expression_utils::extract_string_from_expr(&src_ctx)
                .map_err(PlannerError::InvalidOperation)?;

        let dst_ctx =
            crate::binder::expr_converter::bound_expr_to_contextual(&fetch.dst, &expr_ctx)
                .map_err(PlannerError::InvalidOperation)?;
        let dst_str =
            graphdb_core::types::expr::expression_utils::extract_string_from_expr(&dst_ctx)
                .map_err(PlannerError::InvalidOperation)?;

        let rank_str = match &fetch.rank {
            Some(rank_expr) => {
                let rank_ctx =
                    crate::binder::expr_converter::bound_expr_to_contextual(rank_expr, &expr_ctx)
                        .map_err(PlannerError::InvalidOperation)?;
                graphdb_core::types::expr::expression_utils::extract_string_from_expr(&rank_ctx)
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

    fn match_planner(&self, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Fetch(fetch_stmt) => {
                matches!(&fetch_stmt.target, FetchTarget::Edges { .. })
            }
            _ => false,
        }
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
        column_types: vec![],
    })
}
