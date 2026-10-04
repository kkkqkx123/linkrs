//! SHOW / SHOW CREATE / DESC statement planning.

use crate::parser::ast::ShowTarget;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, IndexManageNode, SpaceManageNode, TagManageNode,
};
use crate::planning::plan::core::nodes::management::system_nodes::{
    ShowAttachedDatabasesNode, ShowExtensionsNode, ShowFunctionsNode, ShowGraphsNode,
    ShowMacrosNode,
};
use crate::planning::plan::core::nodes::{
    ShowCreateEdgeNode, ShowCreateIndexNode, ShowCreateSpaceNode, ShowCreateTagNode, ShowEdgesNode,
    ShowIndexesNode, ShowTagsNode,
};
use crate::planning::plan::core::{PlanNodeEnum, ShowSpacesNode, ShowStatsNode, ShowStatsType};

use super::MaintainPlanner;

impl MaintainPlanner {
    pub(super) fn plan_show(&self, target: &ShowTarget, current_space: &str) -> PlanNodeEnum {
        match target {
            ShowTarget::Stats => {
                let stats_node = ShowStatsNode::new(next_node_id(), ShowStatsType::Storage);
                PlanNodeEnum::ShowStats(stats_node)
            }
            ShowTarget::Tags => {
                let show_tags_node = ShowTagsNode::new(next_node_id(), current_space.to_string());
                PlanNodeEnum::TagManage(TagManageNode::Show(show_tags_node))
            }
            ShowTarget::Edges => {
                let show_edges_node = ShowEdgesNode::new(next_node_id(), current_space.to_string());
                PlanNodeEnum::EdgeManage(EdgeManageNode::Show(show_edges_node))
            }
            ShowTarget::Spaces => {
                let show_spaces_node = ShowSpacesNode::new(next_node_id());
                PlanNodeEnum::SpaceManage(SpaceManageNode::Show(show_spaces_node))
            }
            ShowTarget::Indexes => {
                let show_indexes_node =
                    ShowIndexesNode::new(next_node_id(), current_space.to_string());
                PlanNodeEnum::IndexManage(IndexManageNode::ShowIndexes(show_indexes_node))
            }
            ShowTarget::Index(_) => {
                let show_indexes_node =
                    ShowIndexesNode::new(next_node_id(), current_space.to_string());
                PlanNodeEnum::IndexManage(IndexManageNode::ShowIndexes(show_indexes_node))
            }
            ShowTarget::Functions => {
                let show_functions_node = ShowFunctionsNode::new(next_node_id());
                PlanNodeEnum::ShowFunctions(show_functions_node)
            }
            ShowTarget::Graphs => {
                let show_graphs_node = ShowGraphsNode::new(next_node_id());
                PlanNodeEnum::ShowGraphs(show_graphs_node)
            }
            ShowTarget::Macros => {
                let show_macros_node = ShowMacrosNode::new(next_node_id());
                PlanNodeEnum::ShowMacros(show_macros_node)
            }
            ShowTarget::AttachedDatabases => {
                let node = ShowAttachedDatabasesNode::new(next_node_id());
                PlanNodeEnum::ShowAttachedDatabases(node)
            }
            ShowTarget::Extensions => {
                let node = ShowExtensionsNode::new(next_node_id());
                PlanNodeEnum::ShowExtensions(node)
            }
        }
    }

    pub(super) fn plan_show_create(
        &self,
        target: &crate::parser::ast::stmt::ShowCreateTarget,
        current_space: &str,
    ) -> PlanNodeEnum {
        match target {
            crate::parser::ast::stmt::ShowCreateTarget::Tag(tag_name) => {
                let node = ShowCreateTagNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    tag_name.clone(),
                );
                PlanNodeEnum::TagManage(TagManageNode::ShowCreate(node))
            }
            crate::parser::ast::stmt::ShowCreateTarget::Space(space_name) => {
                let node = ShowCreateSpaceNode::new(next_node_id(), space_name.clone());
                PlanNodeEnum::SpaceManage(SpaceManageNode::ShowCreate(node))
            }
            crate::parser::ast::stmt::ShowCreateTarget::Edge(edge_name) => {
                let node = ShowCreateEdgeNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    edge_name.clone(),
                );
                PlanNodeEnum::EdgeManage(EdgeManageNode::ShowCreate(node))
            }
            crate::parser::ast::stmt::ShowCreateTarget::Index(index_name) => {
                let node = ShowCreateIndexNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    index_name.clone(),
                );
                PlanNodeEnum::IndexManage(IndexManageNode::ShowCreateIndex(node))
            }
        }
    }
    pub(super) fn plan_desc(
        &self,
        target: &crate::parser::ast::stmt::DescTarget,
        current_space: &str,
    ) -> PlanNodeEnum {
        match target {
            crate::parser::ast::stmt::DescTarget::Tag {
                space_name,
                tag_name,
            } => {
                let effective_space = if space_name.is_empty() {
                    current_space.to_string()
                } else {
                    space_name.clone()
                };
                let node = crate::planning::plan::core::nodes::DescTagNode::new(
                    next_node_id(),
                    effective_space,
                    tag_name.clone(),
                );
                PlanNodeEnum::TagManage(TagManageNode::Desc(node))
            }
            crate::parser::ast::stmt::DescTarget::Edge {
                space_name,
                edge_name,
            } => {
                let effective_space = if space_name.is_empty() {
                    current_space.to_string()
                } else {
                    space_name.clone()
                };
                let node = crate::planning::plan::core::nodes::DescEdgeNode::new(
                    next_node_id(),
                    effective_space,
                    edge_name.clone(),
                );
                PlanNodeEnum::EdgeManage(EdgeManageNode::Desc(node))
            }
            crate::parser::ast::stmt::DescTarget::Space(space_name) => {
                let node = crate::planning::plan::core::nodes::DescSpaceNode::new(
                    next_node_id(),
                    space_name.clone(),
                );
                PlanNodeEnum::SpaceManage(SpaceManageNode::Desc(node))
            }
        }
    }
}
