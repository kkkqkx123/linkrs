use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::operator_spec::{DdlSpec, SourceSpec};
use crate::query::executor::streaming::physical_node::PhysicalNode;
use crate::query::executor::streaming::physical_properties::PhysicalProperties;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

pub fn build_ddl_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    match node {
        PlanNodeEnum::SpaceManage(manage_node) => Ok(PhysicalNode::Ddl(
            node.id(),
            Box::new(PhysicalNode::Source(
                0,
                SourceSpec::Start,
                PhysicalProperties::single_streaming(),
            )),
            DdlSpec::SpaceManage {
                command: manage_node.clone(),
            },
            PhysicalProperties::single_blocking(),
        )),

        PlanNodeEnum::TagManage(manage_node) => Ok(PhysicalNode::Ddl(
            node.id(),
            Box::new(PhysicalNode::Source(
                0,
                SourceSpec::Start,
                PhysicalProperties::single_streaming(),
            )),
            DdlSpec::TagManage {
                space_name: context.space_name.clone().unwrap_or_default(),
                command: manage_node.clone(),
            },
            PhysicalProperties::single_blocking(),
        )),

        PlanNodeEnum::EdgeManage(manage_node) => Ok(PhysicalNode::Ddl(
            node.id(),
            Box::new(PhysicalNode::Source(
                0,
                SourceSpec::Start,
                PhysicalProperties::single_streaming(),
            )),
            DdlSpec::EdgeManage {
                space_name: context.space_name.clone().unwrap_or_default(),
                command: manage_node.clone(),
            },
            PhysicalProperties::single_blocking(),
        )),

        PlanNodeEnum::IndexManage(manage_node) => Ok(PhysicalNode::Ddl(
            node.id(),
            Box::new(PhysicalNode::Source(
                0,
                SourceSpec::Start,
                PhysicalProperties::single_streaming(),
            )),
            DdlSpec::IndexManage {
                space_name: context.space_name.clone().unwrap_or_default(),
                command: manage_node.clone(),
            },
            PhysicalProperties::single_blocking(),
        )),

        PlanNodeEnum::UserManage(manage_node) => Ok(PhysicalNode::Ddl(
            node.id(),
            Box::new(PhysicalNode::Source(
                0,
                SourceSpec::Start,
                PhysicalProperties::single_streaming(),
            )),
            DdlSpec::UserManage {
                command: manage_node.clone(),
            },
            PhysicalProperties::single_blocking(),
        )),

        PlanNodeEnum::ShowStats(_) => Ok(PhysicalNode::Ddl(
            node.id(),
            Box::new(PhysicalNode::Source(
                0,
                SourceSpec::Start,
                PhysicalProperties::single_streaming(),
            )),
            DdlSpec::ShowStats {
                space_name: context.space_name.clone().unwrap_or_default(),
            },
            PhysicalProperties::single_blocking(),
        )),

        PlanNodeEnum::DeleteIndex(node) => {
            let info = node.info();
            Ok(PhysicalNode::Ddl(
                node.id(),
                Box::new(PhysicalNode::Source(
                    0,
                    SourceSpec::Start,
                    PhysicalProperties::single_streaming(),
                )),
                DdlSpec::DeleteIndex {
                    space_name: info.space_name.clone(),
                    index_name: info.index_name.clone(),
                },
                PhysicalProperties::single_blocking(),
            ))
        }

        _ => Err(QueryError::execution(format!(
            "Internal routing error: node {} (id={}) was incorrectly routed to ddl builder",
            node.name(),
            node.id()
        ))),
    }
}
