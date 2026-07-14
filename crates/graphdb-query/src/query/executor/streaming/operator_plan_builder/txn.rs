use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::operators::spec::TxnSpec;
use crate::query::executor::streaming::plan::node::PhysicalNode;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

pub fn build_txn_node(
    node: &PlanNodeEnum,
    _context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
    match node {
        PlanNodeEnum::BeginTransaction(_) => Ok(super::build_leaf_command(
            node.id(),
            TxnSpec::BeginTransaction,
            PhysicalNode::Txn,
        )),

        PlanNodeEnum::Commit(_) => Ok(super::build_leaf_command(
            node.id(),
            TxnSpec::Commit,
            PhysicalNode::Txn,
        )),

        PlanNodeEnum::Rollback(_) => Ok(super::build_leaf_command(
            node.id(),
            TxnSpec::Rollback,
            PhysicalNode::Txn,
        )),

        _ => Err(super::internal_routing_error(node, "txn")),
    }
}
