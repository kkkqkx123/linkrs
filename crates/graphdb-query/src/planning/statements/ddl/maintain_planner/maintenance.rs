//! Schema maintenance planning (CREATE / ALTER / DROP, macros and types).

use crate::parser::ast::{AlterTarget, CreateTarget, IndexType};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::management::edge_nodes::EdgeAlterInfo;
use crate::planning::plan::core::nodes::management::index_nodes::IndexManageInfo;
use crate::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, IndexManageNode, MacroManageNode, SpaceManageNode, TagManageNode,
    TypeManageNode,
};
use crate::planning::plan::core::nodes::management::space_nodes::{
    CreateSpaceNode, SpaceManageInfo,
};
use crate::planning::plan::core::nodes::management::tag_nodes::TagAlterInfo;
use crate::planning::plan::core::nodes::{
    AlterEdgeNode, AlterTagNode, CreateEdgeNode, CreateMacroNode, CreateTagNode, CreateTypeNode,
    DropMacroNode, DropTypeNode, EdgeManageInfo, MacroManageInfo, RenameEdgeNode, RenameTagNode,
    TagManageInfo, TypeManageInfo, UpdateEdgeEndpointsNode,
};
use crate::planning::plan::core::{AlterSpaceNode, PlanNodeEnum};
use crate::planning::planner::PlannerError;
use graphdb_core::types::PropertyDef;

use super::MaintainPlanner;

impl MaintainPlanner {
    pub(super) fn plan_create(
        &self,
        target: &CreateTarget,
        if_not_exists: bool,
        current_space: &str,
    ) -> Result<Option<PlanNodeEnum>, PlannerError> {
        match target {
            CreateTarget::Index {
                index_type,
                name,
                on,
                properties,
            } => {
                let space_name = current_space.to_string();
                let index_info = IndexManageInfo::new(
                    space_name,
                    name.clone(),
                    match index_type {
                        IndexType::Tag => "tag".to_string(),
                        IndexType::Edge => "edge".to_string(),
                    },
                )
                .with_target_name(on.clone())
                .with_properties(properties.clone());

                let plan_node = match index_type {
                    IndexType::Tag => {
                        let node = crate::planning::plan::core::nodes::CreateTagIndexNode::new(
                            next_node_id(),
                            index_info,
                        );
                        PlanNodeEnum::IndexManage(IndexManageNode::CreateTagIndex(node))
                    }
                    IndexType::Edge => {
                        let node = crate::planning::plan::core::nodes::CreateEdgeIndexNode::new(
                            next_node_id(),
                            index_info,
                        );
                        PlanNodeEnum::IndexManage(IndexManageNode::CreateEdgeIndex(node))
                    }
                };
                Ok(Some(plan_node))
            }
            CreateTarget::Space { name, vid_type, .. } => {
                let space_info = SpaceManageInfo::new(name.clone()).with_vid_type(vid_type.clone());
                let node = CreateSpaceNode::new(next_node_id(), space_info);
                Ok(Some(PlanNodeEnum::SpaceManage(SpaceManageNode::Create(
                    node,
                ))))
            }
            CreateTarget::Tag {
                name, properties, ..
            } => {
                let space_name = current_space.to_string();
                let tag_info = TagManageInfo::new(space_name, name.clone())
                    .with_properties(properties.clone())
                    .with_if_not_exists(if_not_exists);
                let node = CreateTagNode::new(next_node_id(), tag_info);
                Ok(Some(PlanNodeEnum::TagManage(TagManageNode::Create(node))))
            }
            CreateTarget::EdgeType {
                name,
                properties,
                src_tag,
                dst_tag,
                ..
            } => {
                let space_name = current_space.to_string();
                let mut edge_info = EdgeManageInfo::new(space_name, name.clone())
                    .with_properties(properties.clone())
                    .with_if_not_exists(if_not_exists);
                if let (Some(src), Some(dst)) = (src_tag, dst_tag) {
                    edge_info = edge_info.with_src_dst_tags(src.clone(), dst.clone());
                }
                let node = CreateEdgeNode::new(next_node_id(), edge_info);
                Ok(Some(PlanNodeEnum::EdgeManage(EdgeManageNode::Create(node))))
            }
            CreateTarget::Node { .. } | CreateTarget::Edge { .. } | CreateTarget::Path { .. } => {
                Ok(None)
            }
            CreateTarget::Sequence { .. } => {
                // Sequence creation is handled by the executor.
                Ok(None)
            }
            CreateTarget::TagAsQuery { name, .. } => {
                Err(PlannerError::UnsupportedOperation(format!(
                    "CREATE TAG {} AS (query) is only supported through the session API",
                    name
                )))
            }
            CreateTarget::EdgeAsQuery { name, .. } => {
                Err(PlannerError::UnsupportedOperation(format!(
                    "CREATE EDGE {} AS (query) is only supported through the session API",
                    name
                )))
            }
        }
    }

    fn contextual_body(
        body: &graphdb_core::types::expr::contextual::ContextualExpression,
    ) -> Result<graphdb_core::Expression, PlannerError> {
        body.expression()
            .map(|meta| meta.inner().clone())
            .ok_or_else(|| {
                PlannerError::PlanGenerationFailed(
                    "Macro body expression is missing analysis context".to_string(),
                )
            })
    }

    pub(super) fn plan_create_macro(
        &self,
        stmt: &crate::parser::ast::stmt::CreateMacroStmt,
    ) -> Result<PlanNodeEnum, PlannerError> {
        use graphdb_core::metadata::MacroParamDef;

        if stmt.name.trim().is_empty() {
            return Err(PlannerError::PlanGenerationFailed(
                "Macro name must not be empty".to_string(),
            ));
        }
        let body = Self::contextual_body(&stmt.body)?;
        let mut seen = std::collections::HashSet::new();
        let mut params = Vec::with_capacity(stmt.params.len());
        for param in &stmt.params {
            if param.name.trim().is_empty() {
                return Err(PlannerError::PlanGenerationFailed(format!(
                    "Macro '{}' has an empty parameter name",
                    stmt.name
                )));
            }
            let key = param.name.to_ascii_uppercase();
            if !seen.insert(key) {
                return Err(PlannerError::PlanGenerationFailed(format!(
                    "Macro '{}' has duplicate parameter '{}'",
                    stmt.name, param.name
                )));
            }
            let default = param
                .default_value
                .as_ref()
                .map(Self::contextual_body)
                .transpose()?;
            params.push(MacroParamDef {
                name: param.name.clone(),
                default,
            });
        }
        let info = MacroManageInfo::new(stmt.name.clone(), params, body, stmt.if_not_exists);
        let node = CreateMacroNode::new(next_node_id(), info);
        Ok(PlanNodeEnum::MacroManage(MacroManageNode::Create(node)))
    }

    pub(super) fn plan_drop_macro(
        &self,
        stmt: &crate::parser::ast::stmt::DropMacroStmt,
    ) -> PlanNodeEnum {
        let node =
            DropMacroNode::new(next_node_id(), stmt.name.clone()).with_if_exists(stmt.if_exists);
        PlanNodeEnum::MacroManage(MacroManageNode::Drop(node))
    }

    pub(super) fn plan_create_type(
        &self,
        stmt: &crate::parser::ast::stmt::CreateTypeStmt,
    ) -> Result<PlanNodeEnum, PlannerError> {
        if stmt.name.trim().is_empty() {
            return Err(PlannerError::PlanGenerationFailed(
                "Type name must not be empty".to_string(),
            ));
        }
        if stmt.underlying_type_text.trim().is_empty() {
            return Err(PlannerError::PlanGenerationFailed(format!(
                "Type '{}' has an empty underlying type",
                stmt.name
            )));
        }
        let info = TypeManageInfo::new(
            stmt.name.clone(),
            stmt.underlying_type_text.clone(),
            stmt.if_not_exists,
        );
        let node = CreateTypeNode::new(next_node_id(), info);
        Ok(PlanNodeEnum::TypeManage(TypeManageNode::Create(node)))
    }

    pub(super) fn plan_drop_type(
        &self,
        stmt: &crate::parser::ast::stmt::DropTypeStmt,
    ) -> PlanNodeEnum {
        let node =
            DropTypeNode::new(next_node_id(), stmt.name.clone()).with_if_exists(stmt.if_exists);
        PlanNodeEnum::TypeManage(TypeManageNode::Drop(node))
    }

    pub(super) fn plan_alter(
        &self,
        target: &AlterTarget,
        current_space: &str,
    ) -> Result<PlanNodeEnum, PlannerError> {
        match target {
            AlterTarget::Space {
                space_name,
                comment,
            } => {
                let options = comment
                    .as_ref()
                    .map(|c| {
                        vec![
                            crate::planning::plan::core::nodes::SpaceAlterOption::Comment(
                                c.clone(),
                            ),
                        ]
                    })
                    .unwrap_or_default();
                let node = AlterSpaceNode::new(next_node_id(), space_name.clone(), options);
                Ok(PlanNodeEnum::SpaceManage(SpaceManageNode::Alter(node)))
            }
            AlterTarget::Tag {
                tag_name,
                additions,
                deletions,
                changes,
            } => {
                let alter_info = TagAlterInfo::new(current_space.to_string(), tag_name.clone())
                    .with_additions(additions.clone())
                    .with_deletions(deletions.clone())
                    .with_changes(changes.clone());

                let node = AlterTagNode::new(next_node_id(), alter_info);
                Ok(PlanNodeEnum::TagManage(TagManageNode::Alter(node)))
            }
            AlterTarget::Edge {
                edge_name,
                additions,
                deletions,
                changes,
            } => {
                let mut alter_info =
                    EdgeAlterInfo::new(current_space.to_string(), edge_name.clone())
                        .with_additions(additions.clone())
                        .with_deletions(deletions.clone());

                for change in changes {
                    let prop = PropertyDef::new(change.new_name.clone(), change.data_type.clone());
                    alter_info.additions.push(prop);
                    alter_info.deletions.push(change.old_name.clone());
                }

                let node = AlterEdgeNode::new(next_node_id(), alter_info);
                Ok(PlanNodeEnum::EdgeManage(EdgeManageNode::Alter(node)))
            }
            AlterTarget::Sequence { .. } => Err(PlannerError::UnsupportedOperation(
                "ALTER SEQUENCE planning not yet implemented".to_string(),
            )),
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
                if tag_names.len() > 1 {
                    return Err(PlannerError::UnsupportedOperation(
                        "DROP TAG with multiple names is not yet supported; drop one tag per statement"
                            .to_string(),
                    ));
                }
                let node = crate::planning::plan::core::nodes::DropTagNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    tag_names[0].clone(),
                )
                .with_if_exists(if_exists);
                Ok(PlanNodeEnum::TagManage(TagManageNode::Drop(node)))
            }
            DropTarget::Edges(edge_names) if !edge_names.is_empty() => {
                if edge_names.len() > 1 {
                    return Err(PlannerError::UnsupportedOperation(
                        "DROP EDGE with multiple names is not yet supported; drop one edge type per statement"
                            .to_string(),
                    ));
                }
                let node = crate::planning::plan::core::nodes::DropEdgeNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    edge_names[0].clone(),
                )
                .with_if_exists(if_exists);
                Ok(PlanNodeEnum::EdgeManage(EdgeManageNode::Drop(node)))
            }
            DropTarget::Space(space_name) => {
                let node = crate::planning::plan::core::nodes::DropSpaceNode::new(
                    next_node_id(),
                    space_name.clone(),
                );
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
                );
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
                );
                Ok(PlanNodeEnum::IndexManage(IndexManageNode::DropEdgeIndex(
                    node,
                )))
            }
            DropTarget::Tags(_) => {
                let node = crate::planning::plan::core::nodes::DropTagNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    String::new(),
                )
                .with_if_exists(if_exists);
                Ok(PlanNodeEnum::TagManage(TagManageNode::Drop(node)))
            }
            DropTarget::Edges(_) => {
                let node = crate::planning::plan::core::nodes::DropEdgeNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    String::new(),
                )
                .with_if_exists(if_exists);
                Ok(PlanNodeEnum::EdgeManage(EdgeManageNode::Drop(node)))
            }
            DropTarget::Sequence(_) => Err(PlannerError::UnsupportedOperation(
                "DROP SEQUENCE planning not yet implemented".to_string(),
            )),
        }
    }
}
