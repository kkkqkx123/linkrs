//! USE Statement Planner
//!
//! Query planning for handling the USE <space> statement

use crate::query::parser::ast::{Stmt, UseStmt};
use crate::query::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
use crate::query::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{ArgumentNode, SwitchSpaceNode},
};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use std::sync::Arc;

/// USE Statement Planner
/// Responsible for converting USE statements into execution plans
#[derive(Debug, Clone)]
pub struct UsePlanner;

impl UsePlanner {
    /// Create a new USE planner.
    pub fn new() -> Self {
        Self
    }

    /// Extract the UseStmt from the Stmt.
    fn extract_use_stmt(&self, stmt: &Stmt) -> Result<UseStmt, PlannerError> {
        match stmt {
            Stmt::Use(use_stmt) => Ok(use_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain the USE".to_string(),
            )),
        }
    }
}

impl Planner for UsePlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let use_stmt = self.extract_use_stmt(validated.stmt())?;

        // Create a parameter node as input.
        let arg_node = ArgumentNode::new(next_node_id(), "use_input");
        let arg_node_enum = PlanNodeEnum::Argument(arg_node.clone());

        // Create a SwitchSpace node
        let switch_space_node = SwitchSpaceNode::new(next_node_id(), use_stmt.space.clone());

        let final_node = PlanNodeEnum::SpaceManage(SpaceManageNode::Switch(switch_space_node));

        // Create a SubPlan
        let sub_plan = SubPlan::new(Some(final_node), Some(arg_node_enum));

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Use(_))
    }
}

impl Default for UsePlanner {
    fn default() -> Self {
        Self::new()
    }
}
