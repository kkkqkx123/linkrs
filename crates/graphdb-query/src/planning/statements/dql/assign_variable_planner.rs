//! LET (`AssignVariable`) Statement Planner
//!
//! Plans a `LET [$]name = expr` session-variable assignment as a single-row,
//! single-column expression evaluation plan (Start -> Project), reusing the
//! RETURN expression evaluation chain. The API layer evaluates the plan and
//! stores the produced value in the session; the query engine itself never
//! touches session state.

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::Stmt;
use crate::planning::plan::core::nodes::{ProjectNode, StartNode};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use graphdb_core::YieldColumn;
use std::sync::Arc;

/// LET statement planner
/// Converts the LET statement into an execution plan.
#[derive(Debug, Clone)]
pub struct AssignVariablePlanner;

impl AssignVariablePlanner {
    /// Create a new LET planner.
    pub fn new() -> Self {
        Self
    }
}

impl Planner for AssignVariablePlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let assign = match validated.stmt() {
            Stmt::AssignVariable(assign) => assign,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain a LET assignment".to_string(),
                ));
            }
        };

        let start_node = StartNode::new();
        let current_node = PlanNodeEnum::Start(start_node.clone());

        let yield_column = YieldColumn {
            expression: assign.expression.clone(),
            alias: assign.name.clone(),
            is_matched: false,
        };
        let project_node =
            ProjectNode::new(current_node.clone(), vec![yield_column]).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
            })?;
        let current_node = PlanNodeEnum::Project(project_node);

        let sub_plan = SubPlan::new(Some(current_node), Some(PlanNodeEnum::Start(start_node)));
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
        let assign = match bound {
            BoundStatement::AssignVariable(a) => a,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain a LET assignment".to_string(),
                ));
            }
        };

        let expr_ctx = Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let ctx_expr =
            crate::binder::expr_converter::bound_expr_to_contextual(&assign.expression, &expr_ctx)
                .map_err(PlannerError::PlanGenerationFailed)?;

        let start_node = StartNode::new();
        let current_node = PlanNodeEnum::Start(start_node.clone());

        let yield_column = YieldColumn {
            expression: ctx_expr,
            alias: assign.name.clone(),
            is_matched: false,
        };
        let project_node =
            ProjectNode::new(current_node.clone(), vec![yield_column]).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
            })?;
        let current_node = PlanNodeEnum::Project(project_node);

        let sub_plan = SubPlan::new(Some(current_node), Some(PlanNodeEnum::Start(start_node)));
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::AssignVariable(_))
    }
}

impl Default for AssignVariablePlanner {
    fn default() -> Self {
        Self::new()
    }
}
