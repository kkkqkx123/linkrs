//! The FETCH EDGES query planner
//! Planning for the execution of the FETCH EDGES query

use crate::binder::BoundStatement;
use crate::parser::ast::{FetchTarget, Stmt};
use crate::planning::plan::core::nodes::{GetEdgesNode, PlanNodeEnum, ProjectNode};
use crate::planning::plan::execution_plan::SubPlan;
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

        let get_edges_node = PlanNodeEnum::GetEdges(GetEdgesNode::new(
            1, &src_str, edge_type, &rank_str, &dst_str,
        ));

        // Apply the YIELD clause as a projection when present.
        let root = if let Some(ref yield_clause) = fetch_stmt.yield_clause {
            let mut columns = Vec::new();
            for item in &yield_clause.items {
                columns.push(graphdb_core::YieldColumn {
                    expression: item.expression.clone(),
                    alias: item.alias.clone().unwrap_or_default(),
                    is_matched: false,
                });
            }
            PlanNodeEnum::Project(ProjectNode::new(get_edges_node, columns)?)
        } else {
            get_edges_node
        };

        // For FETCH PROP ON EDGE with specific src/dst/rank, GetEdgesNode is sufficient
        // No need for additional Filter nodes
        let sub_plan = SubPlan::new(Some(root), None);

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
                .map_err(|e| PlannerError::InvalidOperation(e))?;
        let src_str = src_ctx
            .expression()
            .map(|m| m.inner().to_string())
            .unwrap_or_default();

        let dst_ctx =
            crate::binder::expr_converter::bound_expr_to_contextual(&fetch.dst, &expr_ctx)
                .map_err(|e| PlannerError::InvalidOperation(e))?;
        let dst_str = dst_ctx
            .expression()
            .map(|m| m.inner().to_string())
            .unwrap_or_default();

        let rank_str = match &fetch.rank {
            Some(rank_expr) => {
                let rank_ctx =
                    crate::binder::expr_converter::bound_expr_to_contextual(rank_expr, &expr_ctx)
                        .map_err(|e| PlannerError::InvalidOperation(e))?;
                rank_ctx
                    .expression()
                    .map(|m| m.inner().to_string())
                    .unwrap_or_else(|| "0".to_string())
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

        let sub_plan = SubPlan::new(Some(get_edges_node), None);
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
