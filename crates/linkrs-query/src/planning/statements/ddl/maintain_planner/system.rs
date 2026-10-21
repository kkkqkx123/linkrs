//! System and session operation planning (transactions, maintenance, LOAD/CALL).

use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
use crate::planning::plan::core::{ClearSpaceNode, PlanNodeEnum};
use crate::planning::plan::{
    BeginTransactionNode, CommitNode, ReleaseSavepointNode, RollbackNode, SavepointNode,
};

pub(super) fn plan_clear_space(space_name: &str) -> PlanNodeEnum {
    let node = ClearSpaceNode::new(next_node_id(), space_name.to_string());
    PlanNodeEnum::SpaceManage(SpaceManageNode::Clear(node))
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
