//! USE Statement Planner
//!
//! Query planning for handling the USE <space> statement

use crate::binder::BoundStatement;
use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
use crate::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{ArgumentNode, SwitchSpaceNode},
};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError};

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
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
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
}

impl Default for UsePlanner {
    fn default() -> Self {
        Self::new()
    }
}
