//! Standalone WHERE (Filter) Statement Planner
//!
//! A standalone WHERE stage filters the rows of the previous pipe stage,
//! e.g. `GO FROM 1 OVER KNOWS | YIELD target.name AS name | WHERE name != 'x'`.

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::Stmt;
use crate::planning::plan::core::nodes::{FilterNode, StartNode};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use std::sync::Arc;

/// Standalone WHERE statement planner.
#[derive(Debug, Clone)]
pub struct FilterPlanner;

impl FilterPlanner {
    pub fn new() -> Self {
        Self
    }
}

impl Planner for FilterPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let filter_stmt = match validated.stmt() {
            Stmt::Filter(filter_stmt) => filter_stmt,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "FilterPlanner requires the Filter statement.".to_string(),
                ));
            }
        };

        let start_node = StartNode::new();
        let start_enum = PlanNodeEnum::Start(start_node);

        let filter_node = FilterNode::new(start_enum.clone(), filter_stmt.expression.clone())
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create FilterNode: {}", e))
            })?;

        let sub_plan = SubPlan::new(Some(PlanNodeEnum::Filter(filter_node)), Some(start_enum));
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
        let filter = match bound {
            BoundStatement::Filter(f) => f,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "FilterPlanner requires BoundStatement::Filter.".to_string(),
                ));
            }
        };

        let expr_ctx = Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let condition =
            crate::binder::expr_converter::bound_expr_to_contextual(&filter.condition, &expr_ctx)
                .map_err(|e| PlannerError::PlanGenerationFailed(e))?;

        let start_node = StartNode::new();
        let start_enum = PlanNodeEnum::Start(start_node);

        let filter_node = FilterNode::new(start_enum.clone(), condition).map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create FilterNode: {}", e))
        })?;

        let sub_plan = SubPlan::new(Some(PlanNodeEnum::Filter(filter_node)), Some(start_enum));
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Filter(_))
    }
}

impl Default for FilterPlanner {
    fn default() -> Self {
        Self::new()
    }
}
