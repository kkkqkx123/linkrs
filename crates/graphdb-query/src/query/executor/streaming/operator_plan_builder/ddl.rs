use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::plan::node::PhysicalNode;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

use crate::query::executor::streaming::operators::spec::DdlSpec;

pub fn build_ddl_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
    match node {
        PlanNodeEnum::SpaceManage(manage_node) => Ok(super::build_leaf_command(
            node.id(),
            DdlSpec::SpaceManage {
                command: manage_node.clone(),
            },
            PhysicalNode::Ddl,
        )),

        PlanNodeEnum::TagManage(manage_node) => Ok(super::build_leaf_command(
            node.id(),
            DdlSpec::TagManage {
                space_name: context.space_name.clone().unwrap_or_default(),
                command: manage_node.clone(),
            },
            PhysicalNode::Ddl,
        )),

        PlanNodeEnum::EdgeManage(manage_node) => Ok(super::build_leaf_command(
            node.id(),
            DdlSpec::EdgeManage {
                space_name: context.space_name.clone().unwrap_or_default(),
                command: manage_node.clone(),
            },
            PhysicalNode::Ddl,
        )),

        PlanNodeEnum::IndexManage(manage_node) => Ok(super::build_leaf_command(
            node.id(),
            DdlSpec::IndexManage {
                space_name: context.space_name.clone().unwrap_or_default(),
                command: manage_node.clone(),
            },
            PhysicalNode::Ddl,
        )),

        PlanNodeEnum::UserManage(manage_node) => Ok(super::build_leaf_command(
            node.id(),
            DdlSpec::UserManage {
                command: manage_node.clone(),
            },
            PhysicalNode::Ddl,
        )),

        PlanNodeEnum::ShowStats(_) => Ok(super::build_leaf_command(
            node.id(),
            DdlSpec::ShowStats {
                space_name: context.space_name.clone().unwrap_or_default(),
            },
            PhysicalNode::Ddl,
        )),

        PlanNodeEnum::DeleteIndex(node) => {
            let info = node.info();
            Ok(super::build_leaf_command(
                node.id(),
                DdlSpec::DeleteIndex {
                    space_name: info.space_name.clone(),
                    index_name: info.index_name.clone(),
                },
                PhysicalNode::Ddl,
            ))
        }

        _ => Err(super::internal_routing_error(node, "ddl")),
    }
}
