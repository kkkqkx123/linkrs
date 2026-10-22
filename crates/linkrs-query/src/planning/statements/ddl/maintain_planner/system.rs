//! System and session operation planning (transactions, maintenance, LOAD/CALL).

use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
use crate::planning::plan::core::{ClearSpaceNode, PlanNodeEnum};
use crate::planning::plan::{
    BeginTransactionNode, CommitNode, ReleaseSavepointNode, RollbackNode, SavepointNode,
};
use crate::planning::planner::PlannerError;

/// Plan `CLEAR SPACE`.
///
/// The space name may be omitted, in which case the current space is
/// cleared; without a current space the statement has no target and is
/// rejected instead of defaulting to an arbitrary space.
pub(super) fn plan_clear_space(
    space_name: Option<&str>,
    current_space: &str,
) -> Result<PlanNodeEnum, PlannerError> {
    let target = match space_name {
        Some(name) => name,
        None if !current_space.is_empty() => current_space,
        None => {
            return Err(PlannerError::InvalidOperation(
                "CLEAR SPACE requires a space name when no space is selected".to_string(),
            ))
        }
    };
    let node = ClearSpaceNode::new(next_node_id(), target.to_string());
    Ok(PlanNodeEnum::SpaceManage(SpaceManageNode::Clear(node)))
}

pub(super) fn plan_begin_transaction() -> PlanNodeEnum {
    PlanNodeEnum::BeginTransaction(BeginTransactionNode::new(next_node_id()))
}

pub(super) fn plan_commit() -> PlanNodeEnum {
    PlanNodeEnum::Commit(CommitNode::new(next_node_id()))
}

pub(super) fn plan_rollback(savepoint_name: Option<&String>) -> PlanNodeEnum {
    let mut node = RollbackNode::new(next_node_id());
    if let Some(savepoint) = savepoint_name {
        node = node.with_savepoint(savepoint.clone());
    }
    PlanNodeEnum::Rollback(node)
}

pub(super) fn plan_savepoint(name: &str) -> PlanNodeEnum {
    PlanNodeEnum::Savepoint(SavepointNode::new(next_node_id(), name.to_string()))
}

pub(super) fn plan_release_savepoint(name: &str) -> PlanNodeEnum {
    PlanNodeEnum::ReleaseSavepoint(ReleaseSavepointNode::new(next_node_id(), name.to_string()))
}
