//! The FETCH EDGES query planner
//! Planning for the execution of the FETCH EDGES query

use crate::core::types::expr::expression_utils::extract_string_from_expr;
use crate::query::parser::ast::{FetchTarget, Stmt};
use crate::query::planning::plan::core::nodes::GetEdgesNode;
use crate::query::planning::plan::core::PlanNodeEnum;
use crate::query::planning::plan::execution_plan::SubPlan;
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
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

        // For FETCH PROP ON EDGE with specific src/dst/rank, GetEdgesNode is sufficient
        // No need for additional Filter or Project nodes
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
