//! Maintenance Operation Planner
//! Handling query planning related to maintenance tasks (such as SUBMIT JOB, etc.)

use crate::core::types::PropertyDef;
use crate::query::parser::ast::{AlterTarget, CreateTarget, IndexType, ShowTarget, Stmt};
use crate::query::planning::plan::core::nodes::management::edge_nodes::EdgeAlterInfo;
use crate::query::planning::plan::core::nodes::management::index_nodes::IndexManageInfo;
use crate::query::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, IndexManageNode, SpaceManageNode, TagManageNode,
};
use crate::query::planning::plan::core::nodes::management::space_nodes::{
    CreateSpaceNode, SpaceManageInfo,
};
use crate::query::planning::plan::core::nodes::management::tag_nodes::TagAlterInfo;
use crate::query::planning::plan::core::nodes::{
    AlterEdgeNode, AlterTagNode, CreateEdgeNode, CreateTagNode, EdgeManageInfo, ShowCreateEdgeNode,
    ShowCreateIndexNode, ShowCreateSpaceNode, ShowCreateTagNode, ShowEdgesNode, ShowIndexesNode,
    ShowTagsNode, TagManageInfo,
};
use crate::query::planning::plan::core::{
    node_id_generator::next_node_id, AlterSpaceNode, ClearSpaceNode, PlanNodeEnum, ShowSpacesNode,
    ShowStatsNode, ShowStatsType, ShowUsersNode,
};
use crate::query::planning::plan::SubPlan;
use crate::query::planning::plan::{BeginTransactionNode, CommitNode, RollbackNode};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct MaintainPlanner;

impl MaintainPlanner {
    pub fn new() -> Self {
        Self
    }

    fn current_space(&self, validated: &ValidatedStatement) -> String {
        validated
            .validation_info
            .semantic_info
            .space_name
            .clone()
            .unwrap_or_default()
    }

    fn plan_show(
        &self,
        show_stmt: &crate::query::parser::ast::ShowStmt,
        validated: &ValidatedStatement,
    ) -> PlanNodeEnum {
        match show_stmt.target {
            ShowTarget::Stats => {
                let stats_node = ShowStatsNode::new(next_node_id(), ShowStatsType::Storage);
                PlanNodeEnum::ShowStats(stats_node)
            }
            ShowTarget::Tags => {
                let show_tags_node =
                    ShowTagsNode::new(next_node_id(), self.current_space(validated));
                PlanNodeEnum::TagManage(TagManageNode::Show(show_tags_node))
            }
            ShowTarget::Edges => {
                let show_edges_node =
                    ShowEdgesNode::new(next_node_id(), self.current_space(validated));
                PlanNodeEnum::EdgeManage(EdgeManageNode::Show(show_edges_node))
            }
            ShowTarget::Spaces => {
                let show_spaces_node = ShowSpacesNode::new(next_node_id());
                PlanNodeEnum::SpaceManage(SpaceManageNode::Show(show_spaces_node))
            }
            ShowTarget::Users => {
                let show_users_node = ShowUsersNode::new(next_node_id());
                PlanNodeEnum::UserManage(crate::query::planning::plan::core::nodes::management::manage_node_enums::UserManageNode::ShowUsers(show_users_node))
            }
            ShowTarget::Roles => {
                let show_roles_node = crate::query::planning::plan::core::nodes::ShowRolesNode::new(
                    next_node_id(),
                    self.current_space(validated),
                );
                PlanNodeEnum::UserManage(crate::query::planning::plan::core::nodes::management::manage_node_enums::UserManageNode::ShowRoles(show_roles_node))
            }
            ShowTarget::Indexes => {
                let show_indexes_node =
                    ShowIndexesNode::new(next_node_id(), self.current_space(validated));
                PlanNodeEnum::IndexManage(IndexManageNode::ShowIndexes(show_indexes_node))
            }
            ShowTarget::Tag(_) => {
                let show_tags_node =
                    ShowTagsNode::new(next_node_id(), self.current_space(validated));
                PlanNodeEnum::TagManage(TagManageNode::Show(show_tags_node))
            }
            ShowTarget::Edge(_) => {
                let show_edges_node =
                    ShowEdgesNode::new(next_node_id(), self.current_space(validated));
                PlanNodeEnum::EdgeManage(EdgeManageNode::Show(show_edges_node))
            }
            ShowTarget::Index(_) => {
                let show_indexes_node =
                    ShowIndexesNode::new(next_node_id(), self.current_space(validated));
                PlanNodeEnum::IndexManage(IndexManageNode::ShowIndexes(show_indexes_node))
            }
        }
    }

    fn plan_show_create(
        &self,
        show_create_stmt: &crate::query::parser::ast::stmt::ShowCreateStmt,
        validated: &ValidatedStatement,
    ) -> PlanNodeEnum {
        let current_space = self.current_space(validated);
        match &show_create_stmt.target {
            crate::query::parser::ast::stmt::ShowCreateTarget::Tag(tag_name) => {
                let node = ShowCreateTagNode::new(next_node_id(), current_space, tag_name.clone());
                PlanNodeEnum::TagManage(TagManageNode::ShowCreate(node))
            }
            crate::query::parser::ast::stmt::ShowCreateTarget::Space(space_name) => {
                let node = ShowCreateSpaceNode::new(next_node_id(), space_name.clone());
                PlanNodeEnum::SpaceManage(SpaceManageNode::ShowCreate(node))
            }
            crate::query::parser::ast::stmt::ShowCreateTarget::Edge(edge_name) => {
                let node =
                    ShowCreateEdgeNode::new(next_node_id(), current_space, edge_name.clone());
                PlanNodeEnum::EdgeManage(EdgeManageNode::ShowCreate(node))
            }
            crate::query::parser::ast::stmt::ShowCreateTarget::Index(index_name) => {
                let node =
                    ShowCreateIndexNode::new(next_node_id(), current_space, index_name.clone());
                PlanNodeEnum::IndexManage(IndexManageNode::ShowCreateIndex(node))
            }
        }
    }

    fn plan_create(
        &self,
        create_stmt: &crate::query::parser::ast::CreateStmt,
        validated: &ValidatedStatement,
    ) -> Result<Option<PlanNodeEnum>, PlannerError> {
        match &create_stmt.target {
            CreateTarget::Index {
                index_type,
                name,
                on,
                properties,
            } => {
                let space_name = self.current_space(validated);
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
                        let node =
                            crate::query::planning::plan::core::nodes::CreateTagIndexNode::new(
                                next_node_id(),
                                index_info,
                            );
                        PlanNodeEnum::IndexManage(IndexManageNode::CreateTagIndex(node))
                    }
                    IndexType::Edge => {
                        let node =
                            crate::query::planning::plan::core::nodes::CreateEdgeIndexNode::new(
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
                let space_name = self.current_space(validated);
                let tag_info = TagManageInfo::new(space_name, name.clone())
                    .with_properties(properties.clone())
                    .with_if_not_exists(create_stmt.if_not_exists);
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
                let space_name = self.current_space(validated);
                let mut edge_info = EdgeManageInfo::new(space_name, name.clone())
                    .with_properties(properties.clone())
                    .with_if_not_exists(create_stmt.if_not_exists);
                if let (Some(src), Some(dst)) = (src_tag, dst_tag) {
                    edge_info = edge_info.with_src_dst_tags(src.clone(), dst.clone());
                }
                let node = CreateEdgeNode::new(next_node_id(), edge_info);
                Ok(Some(PlanNodeEnum::EdgeManage(EdgeManageNode::Create(node))))
            }
            CreateTarget::Node { .. } | CreateTarget::Edge { .. } | CreateTarget::Path { .. } => {
                Ok(None)
            }
        }
    }

    fn plan_alter(
        &self,
        alter_stmt: &crate::query::parser::ast::AlterStmt,
        validated: &ValidatedStatement,
    ) -> PlanNodeEnum {
        match &alter_stmt.target {
            AlterTarget::Space {
                space_name,
                comment,
            } => {
                let options = comment
                    .as_ref()
                    .map(|c| {
                        vec![
                            crate::query::planning::plan::core::nodes::SpaceAlterOption::Comment(
                                c.clone(),
                            ),
                        ]
                    })
                    .unwrap_or_default();
                let node = AlterSpaceNode::new(next_node_id(), space_name.clone(), options);
                PlanNodeEnum::SpaceManage(SpaceManageNode::Alter(node))
            }
            AlterTarget::Tag {
                tag_name,
                additions,
                deletions,
                changes,
            } => {
                let current_space = self.current_space(validated);
                let alter_info = TagAlterInfo::new(current_space, tag_name.clone())
                    .with_additions(additions.clone())
                    .with_deletions(deletions.clone())
                    .with_changes(changes.clone());

                let node = AlterTagNode::new(next_node_id(), alter_info);
                PlanNodeEnum::TagManage(TagManageNode::Alter(node))
            }
            AlterTarget::Edge {
                edge_name,
                additions,
                deletions,
                changes,
            } => {
                let current_space = self.current_space(validated);
                let mut alter_info = EdgeAlterInfo::new(current_space, edge_name.clone())
                    .with_additions(additions.clone())
                    .with_deletions(deletions.clone());

                for change in changes {
                    let prop = PropertyDef::new(change.new_name.clone(), change.data_type.clone());
                    alter_info.additions.push(prop);
                    alter_info.deletions.push(change.old_name.clone());
                }

                let node = AlterEdgeNode::new(next_node_id(), alter_info);
                PlanNodeEnum::EdgeManage(EdgeManageNode::Alter(node))
            }
        }
    }

    fn plan_desc(
        &self,
        desc_stmt: &crate::query::parser::ast::stmt::DescStmt,
        validated: &ValidatedStatement,
    ) -> PlanNodeEnum {
        let current_space = self.current_space(validated);

        match &desc_stmt.target {
            crate::query::parser::ast::stmt::DescTarget::Tag {
                space_name,
                tag_name,
            } => {
                let effective_space = if space_name.is_empty() {
                    current_space
                } else {
                    space_name.clone()
                };
                let node = crate::query::planning::plan::core::nodes::DescTagNode::new(
                    next_node_id(),
                    effective_space,
                    tag_name.clone(),
                );
                PlanNodeEnum::TagManage(TagManageNode::Desc(node))
            }
            crate::query::parser::ast::stmt::DescTarget::Edge {
                space_name,
                edge_name,
            } => {
                let effective_space = if space_name.is_empty() {
                    current_space
                } else {
                    space_name.clone()
                };
                let node = crate::query::planning::plan::core::nodes::DescEdgeNode::new(
                    next_node_id(),
                    effective_space,
                    edge_name.clone(),
                );
                PlanNodeEnum::EdgeManage(EdgeManageNode::Desc(node))
            }
            crate::query::parser::ast::stmt::DescTarget::Space(space_name) => {
                let node = crate::query::planning::plan::core::nodes::DescSpaceNode::new(
                    next_node_id(),
                    space_name.clone(),
                );
                PlanNodeEnum::SpaceManage(SpaceManageNode::Desc(node))
            }
        }
    }

    fn plan_drop(
        &self,
        drop_stmt: &crate::query::parser::ast::DropStmt,
        validated: &ValidatedStatement,
    ) -> PlanNodeEnum {
        use crate::query::parser::ast::stmt::DropTarget;
        let current_space = self.current_space(validated);

        match &drop_stmt.target {
            DropTarget::Tags(tag_names) if !tag_names.is_empty() => {
                let node = crate::query::planning::plan::core::nodes::DropTagNode::new(
                    next_node_id(),
                    current_space,
                    tag_names[0].clone(),
                )
                .with_if_exists(drop_stmt.if_exists);
                PlanNodeEnum::TagManage(TagManageNode::Drop(node))
            }
            DropTarget::Edges(edge_names) if !edge_names.is_empty() => {
                let node = crate::query::planning::plan::core::nodes::DropEdgeNode::new(
                    next_node_id(),
                    current_space,
                    edge_names[0].clone(),
                )
                .with_if_exists(drop_stmt.if_exists);
                PlanNodeEnum::EdgeManage(EdgeManageNode::Drop(node))
            }
            DropTarget::Space(space_name) => {
                let node = crate::query::planning::plan::core::nodes::DropSpaceNode::new(
                    next_node_id(),
                    space_name.clone(),
                );
                PlanNodeEnum::SpaceManage(SpaceManageNode::Drop(node))
            }
            DropTarget::TagIndex {
                space_name,
                index_name,
            } => {
                let resolved_space = if space_name.is_empty() {
                    current_space
                } else {
                    space_name.clone()
                };
                let node = crate::query::planning::plan::core::nodes::DropTagIndexNode::new(
                    next_node_id(),
                    resolved_space,
                    index_name.clone(),
                );
                PlanNodeEnum::IndexManage(IndexManageNode::DropTagIndex(node))
            }
            DropTarget::EdgeIndex {
                space_name,
                index_name,
            } => {
                let resolved_space = if space_name.is_empty() {
                    current_space
                } else {
                    space_name.clone()
                };
                let node = crate::query::planning::plan::core::nodes::DropEdgeIndexNode::new(
                    next_node_id(),
                    resolved_space,
                    index_name.clone(),
                );
                PlanNodeEnum::IndexManage(IndexManageNode::DropEdgeIndex(node))
            }
            DropTarget::Tags(_) => {
                let node = crate::query::planning::plan::core::nodes::DropTagNode::new(
                    next_node_id(),
                    current_space,
                    String::new(),
                )
                .with_if_exists(drop_stmt.if_exists);
                PlanNodeEnum::TagManage(TagManageNode::Drop(node))
            }
            DropTarget::Edges(_) => {
                let node = crate::query::planning::plan::core::nodes::DropEdgeNode::new(
                    next_node_id(),
                    current_space,
                    String::new(),
                )
                .with_if_exists(drop_stmt.if_exists);
                PlanNodeEnum::EdgeManage(EdgeManageNode::Drop(node))
            }
        }
    }
}

impl Planner for MaintainPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let stmt = validated.stmt();

        let final_node = match stmt {
            Stmt::Show(show_stmt) => self.plan_show(show_stmt, validated),

            Stmt::ShowCreate(show_create_stmt) => {
                self.plan_show_create(show_create_stmt, validated)
            }

            Stmt::Create(create_stmt) => {
                if let Some(node) = self.plan_create(create_stmt, validated)? {
                    return Ok(SubPlan::from_single_node(node));
                }
                return Err(PlannerError::UnsupportedOperation(
                    "Create Node/Edge/Path is not supported by MaintainPlanner".to_string(),
                ));
            }

            Stmt::Alter(alter_stmt) => self.plan_alter(alter_stmt, validated),

            Stmt::ClearSpace(clear_stmt) => {
                let node = ClearSpaceNode::new(next_node_id(), clear_stmt.space_name.clone());
                PlanNodeEnum::SpaceManage(SpaceManageNode::Clear(node))
            }

            Stmt::Desc(desc_stmt) => self.plan_desc(desc_stmt, validated),

            Stmt::BeginTransaction(_) => {
                let node = BeginTransactionNode::new(next_node_id());
                PlanNodeEnum::BeginTransaction(node)
            }

            Stmt::CommitTransaction(_) => {
                let node = CommitNode::new(next_node_id());
                PlanNodeEnum::Commit(node)
            }

            Stmt::RollbackTransaction(_) => {
                let node = RollbackNode::new(next_node_id());
                PlanNodeEnum::Rollback(node)
            }

            Stmt::Drop(drop_stmt) => self.plan_drop(drop_stmt, validated),

            _ => {
                return Err(PlannerError::UnsupportedOperation(format!(
                    "Statement {:?} is not supported by MaintainPlanner",
                    stmt
                )));
            }
        };

        let sub_plan = SubPlan::from_single_node(final_node);
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(
            stmt,
            Stmt::Show(_)
                | Stmt::ShowCreate(_)
                | Stmt::Create(_)
                | Stmt::Alter(_)
                | Stmt::ClearSpace(_)
                | Stmt::Desc(_)
                | Stmt::Drop(_)
                | Stmt::BeginTransaction(_)
                | Stmt::CommitTransaction(_)
                | Stmt::RollbackTransaction(_)
        )
    }
}

impl Default for MaintainPlanner {
    fn default() -> Self {
        Self::new()
    }
}
