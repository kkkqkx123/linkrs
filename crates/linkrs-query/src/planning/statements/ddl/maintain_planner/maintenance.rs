//! Schema maintenance planning (CREATE / ALTER / DROP, macros and types).

use crate::parser::ast::AlterTarget;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::management::edge_nodes::EdgeAlterInfo;
use crate::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, IndexManageNode, SequenceManageNode, SpaceManageNode, TagManageNode,
};
use crate::planning::plan::core::nodes::management::sequence_nodes::SequenceManageInfo;
use crate::planning::plan::core::nodes::management::tag_nodes::TagAlterInfo;
use crate::planning::plan::core::nodes::{
    AlterEdgeNode, AlterSequenceNode, AlterTagNode, DropSequenceNode, RenameEdgeNode,
    RenameTagNode, UpdateEdgeEndpointsNode,
};
use crate::planning::plan::core::PlanNodeEnum;
use crate::planning::planner::PlannerError;

use super::MaintainPlanner;

impl MaintainPlanner {
    /// Validate sequence step and bounds at plan time so a stuck or
    /// inverted sequence fails loudly instead of misbehaving at runtime.
    fn check_sequence_bounds(
        name: &str,
        increment: Option<i64>,
        min_value: Option<i64>,
        max_value: Option<i64>,
    ) -> Result<(), PlannerError> {
        if increment == Some(0) {
            return Err(PlannerError::PlanGenerationFailed(format!(
                "Sequence '{name}' has INCREMENT=0, which would never advance"
            )));
        }
        if let (Some(min), Some(max)) = (min_value, max_value) {
            if min > max {
                return Err(PlannerError::PlanGenerationFailed(format!(
                    "Sequence '{name}' has MINVALUE ({min}) greater than MAXVALUE ({max})"
                )));
            }
        }
        Ok(())
    }

    pub(super) fn plan_alter(
        &self,
        target: &AlterTarget,
        current_space: &str,
    ) -> Result<PlanNodeEnum, PlannerError> {
        match target {
            AlterTarget::Tag {
                tag_name,
                additions,
                deletions,
                changes,
                new_comment,
            } => {
                let alter_info = TagAlterInfo::new(current_space.to_string(), tag_name.clone())
                    .with_additions(additions.clone())
                    .with_deletions(deletions.clone())
                    .with_changes(changes.clone())
                    .with_new_comment(new_comment.clone());

                let node = AlterTagNode::new(next_node_id(), alter_info);
                Ok(PlanNodeEnum::TagManage(TagManageNode::Alter(node)))
            }
            AlterTarget::Edge {
                edge_name,
                additions,
                deletions,
                changes,
                new_comment,
            } => {
                let alter_info = EdgeAlterInfo::new(current_space.to_string(), edge_name.clone())
                    .with_additions(additions.clone())
                    .with_deletions(deletions.clone())
                    .with_changes(changes.clone())
                    .with_new_comment(new_comment.clone());

                let node = AlterEdgeNode::new(next_node_id(), alter_info);
                Ok(PlanNodeEnum::EdgeManage(EdgeManageNode::Alter(node)))
            }
            AlterTarget::Sequence {
                name,
                increment,
                min_value,
                max_value,
                cycle,
            } => {
                if name.trim().is_empty() {
                    return Err(PlannerError::PlanGenerationFailed(
                        "ALTER SEQUENCE requires a non-empty sequence name".to_string(),
                    ));
                }
                Self::check_sequence_bounds(name, *increment, *min_value, *max_value)?;
                let info = SequenceManageInfo::new(name.clone())
                    .with_increment(*increment)
                    .with_bounds(*min_value, *max_value)
                    .with_cycle(*cycle);
                let node = AlterSequenceNode::new(next_node_id(), info);
                Ok(PlanNodeEnum::SequenceManage(SequenceManageNode::Alter(
                    node,
                )))
            }
            AlterTarget::RenameTag { old_name, new_name } => {
                if old_name.trim().is_empty() || new_name.trim().is_empty() {
                    return Err(PlannerError::PlanGenerationFailed(
                        "RENAME TAG requires non-empty old and new names".to_string(),
                    ));
                }
                let node = RenameTagNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    old_name.clone(),
                    new_name.clone(),
                );
                Ok(PlanNodeEnum::TagManage(TagManageNode::Rename(node)))
            }
            AlterTarget::RenameEdge { old_name, new_name } => {
                if old_name.trim().is_empty() || new_name.trim().is_empty() {
                    return Err(PlannerError::PlanGenerationFailed(
                        "RENAME EDGE requires non-empty old and new names".to_string(),
                    ));
                }
                let node = RenameEdgeNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    old_name.clone(),
                    new_name.clone(),
                );
                Ok(PlanNodeEnum::EdgeManage(EdgeManageNode::Rename(node)))
            }
            AlterTarget::AddFrom {
                edge_name,
                src_tag,
                dst_tag,
            } => {
                if edge_name.trim().is_empty()
                    || src_tag.trim().is_empty()
                    || dst_tag.trim().is_empty()
                {
                    return Err(PlannerError::PlanGenerationFailed(
                        "ALTER EDGE ADD FROM requires edge, src and dst names".to_string(),
                    ));
                }
                let node = UpdateEdgeEndpointsNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    edge_name.clone(),
                    src_tag.clone(),
                    dst_tag.clone(),
                    false,
                );
                Ok(PlanNodeEnum::EdgeManage(EdgeManageNode::UpdateEndpoints(
                    node,
                )))
            }
            AlterTarget::DropFrom {
                edge_name,
                src_tag,
                dst_tag,
            } => {
                if edge_name.trim().is_empty() {
                    return Err(PlannerError::PlanGenerationFailed(
                        "ALTER EDGE DROP FROM requires an edge name".to_string(),
                    ));
                }
                let node = UpdateEdgeEndpointsNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    edge_name.clone(),
                    src_tag.clone(),
                    dst_tag.clone(),
                    true,
                );
                Ok(PlanNodeEnum::EdgeManage(EdgeManageNode::UpdateEndpoints(
                    node,
                )))
            }
        }
    }
    pub(super) fn plan_drop(
        &self,
        target: &crate::parser::ast::stmt::DropTarget,
        if_exists: bool,
        current_space: &str,
    ) -> Result<PlanNodeEnum, PlannerError> {
        use crate::parser::ast::stmt::DropTarget;

        match target {
            DropTarget::Tags(tag_names) if !tag_names.is_empty() => {
                let node = crate::planning::plan::core::nodes::DropTagNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    tag_names.clone(),
                )
                .with_if_exists(if_exists);
                Ok(PlanNodeEnum::TagManage(TagManageNode::Drop(node)))
            }
            DropTarget::Edges(edge_names) if !edge_names.is_empty() => {
                let node = crate::planning::plan::core::nodes::DropEdgeNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    edge_names.clone(),
                )
                .with_if_exists(if_exists);
                Ok(PlanNodeEnum::EdgeManage(EdgeManageNode::Drop(node)))
            }
            DropTarget::Space(space_name) => {
                let node = crate::planning::plan::core::nodes::DropSpaceNode::new(
                    next_node_id(),
                    space_name.clone(),
                )
                .with_if_exists(if_exists);
                Ok(PlanNodeEnum::SpaceManage(SpaceManageNode::Drop(node)))
            }
            DropTarget::TagIndex {
                space_name,
                index_name,
            } => {
                let resolved_space = if space_name.is_empty() {
                    current_space.to_string()
                } else {
                    space_name.clone()
                };
                let node = crate::planning::plan::core::nodes::DropTagIndexNode::new(
                    next_node_id(),
                    resolved_space,
                    index_name.clone(),
                )
                .with_if_exists(if_exists);
                Ok(PlanNodeEnum::IndexManage(IndexManageNode::DropTagIndex(
                    node,
                )))
            }
            DropTarget::EdgeIndex {
                space_name,
                index_name,
            } => {
                let resolved_space = if space_name.is_empty() {
                    current_space.to_string()
                } else {
                    space_name.clone()
                };
                let node = crate::planning::plan::core::nodes::DropEdgeIndexNode::new(
                    next_node_id(),
                    resolved_space,
                    index_name.clone(),
                )
                .with_if_exists(if_exists);
                Ok(PlanNodeEnum::IndexManage(IndexManageNode::DropEdgeIndex(
                    node,
                )))
            }
            DropTarget::Tags(_) | DropTarget::Edges(_) => Err(PlannerError::PlanGenerationFailed(
                "DROP TAG/EDGE requires at least one name; the parser guarantees a non-empty list"
                    .to_string(),
            )),
            DropTarget::Sequence(seq_name) => {
                if seq_name.trim().is_empty() {
                    return Err(PlannerError::PlanGenerationFailed(
                        "DROP SEQUENCE requires a non-empty sequence name".to_string(),
                    ));
                }
                let node = DropSequenceNode::new(next_node_id(), seq_name.clone())
                    .with_if_exists(if_exists);
                Ok(PlanNodeEnum::SequenceManage(SequenceManageNode::Drop(node)))
            }
        }
    }
}
