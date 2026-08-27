//! LET (`AssignVariable`) Statement Planner
//!
//! Plans a `LET [$]name = expr` session-variable assignment as a single-row,
//! single-column expression evaluation plan (Start -> Project), reusing the
//! RETURN expression evaluation chain. The API layer evaluates the plan and
//! stores the produced value in the session; the query engine itself never
//! touches session state.

use crate::core::YieldColumn;
use crate::parser::ast::stmt::{AssignVariableStmt, Stmt};
use crate::planning::plan::core::nodes::{ProjectNode, StartNode};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
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

    fn extract_assign_variable_stmt(
        &self,
        stmt: &Stmt,
    ) -> Result<AssignVariableStmt, PlannerError> {
        match stmt {
            Stmt::AssignVariable(assign) => Ok(assign.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain a LET assignment".to_string(),
            )),
        }
    }
}

impl Planner for AssignVariablePlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let assign = self.extract_assign_variable_stmt(validated.stmt())?;

        // A single empty row seeds the standalone expression evaluation.
        let start_node = StartNode::new();
        let current_node = PlanNodeEnum::Start(start_node.clone());

        // Project the right-hand side expression as a single named column
        // (the variable name is the alias; the API layer reads the value and
        // stores it in the session).
        let yield_column = YieldColumn {
            expression: assign.expression,
            alias: assign.name,
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
