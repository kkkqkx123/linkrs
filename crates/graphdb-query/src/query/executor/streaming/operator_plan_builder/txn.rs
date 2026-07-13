use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::operator_spec::{SourceSpec, TxnSpec};
use crate::query::executor::streaming::physical_node::PhysicalNode;
use crate::query::executor::streaming::physical_properties::PhysicalProperties;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

pub fn build_txn_node(
    node: &PlanNodeEnum,
    _context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    match node {
        PlanNodeEnum::BeginTransaction(_) => Ok(PhysicalNode::Txn(
            node.id(),
            Box::new(PhysicalNode::Source(
                0,
                SourceSpec::Start,
                PhysicalProperties::single_streaming(),
            )),
            TxnSpec::BeginTransaction {
                transaction_id: None,
            },
            PhysicalProperties::single_blocking(),
        )),

        PlanNodeEnum::Commit(_) => Ok(PhysicalNode::Txn(
            node.id(),
            Box::new(PhysicalNode::Source(
                0,
                SourceSpec::Start,
                PhysicalProperties::single_streaming(),
            )),
            TxnSpec::Commit {
                transaction_id: None,
            },
            PhysicalProperties::single_blocking(),
        )),

        PlanNodeEnum::Rollback(_) => Ok(PhysicalNode::Txn(
            node.id(),
            Box::new(PhysicalNode::Source(
                0,
                SourceSpec::Start,
                PhysicalProperties::single_streaming(),
            )),
            TxnSpec::Rollback {
                transaction_id: None,
            },
            PhysicalProperties::single_blocking(),
        )),

        _ => Err(QueryError::execution(format!(
            "Internal routing error: node {} (id={}) was incorrectly routed to txn builder",
            node.name(),
            node.id()
        ))),
    }
}
