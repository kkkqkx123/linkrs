//! USE Statement Planner
//!
//! Query planning for handling the USE <space> statement

use crate::binder::BoundStatement;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
use crate::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{ArgumentNode, SwitchSpaceNode},
};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
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
}

impl Planner for UsePlanner {
    fn plan_bound(
        &mut self,
        bound: &BoundStatement,
        _qctx: Arc<QueryContext>,
        _metadata: Option<&crate::metadata::MetadataContext>,
        _validated: &ValidatedStatement,
    ) -> Result<SubPlan, PlannerError> {
        let space = match bound {
            BoundStatement::Use(u) => &u.space,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "UsePlanner requires BoundStatement::Use".to_string(),
                ));
            }
        };

        let arg_node = ArgumentNode::new(next_node_id(), "use_input");
        let arg_node_enum = PlanNodeEnum::Argument(arg_node);
        let switch_space_node = SwitchSpaceNode::new(next_node_id(), space.clone());
        let final_node = PlanNodeEnum::SpaceManage(SpaceManageNode::Switch(switch_space_node));
        let sub_plan = SubPlan::new(Some(final_node), Some(arg_node_enum));
        Ok(sub_plan)
    }

    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let use_stmt = match validated.stmt() {
            Stmt::Use(s) => s,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain the USE".to_string(),
                ));
            }
        };

        let arg_node = ArgumentNode::new(next_node_id(), "use_input");
        let arg_node_enum = PlanNodeEnum::Argument(arg_node);
        let switch_space_node = SwitchSpaceNode::new(next_node_id(), use_stmt.space.clone());
        let final_node = PlanNodeEnum::SpaceManage(SpaceManageNode::Switch(switch_space_node));
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
