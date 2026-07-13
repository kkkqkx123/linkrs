use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::operators::spec::TxnSpec;
use crate::query::executor::streaming::plan::node::PhysicalNode;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

pub fn build_txn_node(
    node: &PlanNodeEnum,
    _context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    match node {
        PlanNodeEnum::BeginTransaction(_) => Ok(super::build_leaf_command(
            node.id(),
            TxnSpec::BeginTransaction { transaction_id: None },
            PhysicalNode::Txn,
        )),

        PlanNodeEnum::Commit(_) => Ok(super::build_leaf_command(
            node.id(),
            TxnSpec::Commit { transaction_id: None },
            PhysicalNode::Txn,
        )),

        PlanNodeEnum::Rollback(_) => Ok(super::build_leaf_command(
            node.id(),
            TxnSpec::Rollback { transaction_id: None },
            PhysicalNode::Txn,
        )),

        _ => Err(super::internal_routing_error(node, "txn")),
    }
}
