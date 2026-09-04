//! Maintenance Operation Planner
//! Handling query planning related to maintenance tasks (such as SUBMIT JOB, etc.)

use crate::binder::BoundStatement;
use crate::parser::ast::{AlterTarget, CreateTarget, IndexType, ShowTarget, Stmt};
use crate::planning::plan::core::nodes::management::edge_nodes::EdgeAlterInfo;
use crate::planning::plan::core::nodes::management::index_nodes::IndexManageInfo;
use crate::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, IndexManageNode, SpaceManageNode, TagManageNode,
};
use crate::planning::plan::core::nodes::management::space_nodes::{
    CreateSpaceNode, SpaceManageInfo,
};
use crate::planning::plan::core::nodes::management::tag_nodes::TagAlterInfo;
use crate::planning::plan::core::nodes::{
    AlterEdgeNode, AlterTagNode, CreateEdgeNode, CreateTagNode, EdgeManageInfo, ShowCreateEdgeNode,
    ShowCreateIndexNode, ShowCreateSpaceNode, ShowCreateTagNode, ShowEdgesNode, ShowIndexesNode,
    ShowTagsNode, TagManageInfo,
};
use crate::planning::plan::core::{
    node_id_generator::next_node_id, AlterSpaceNode, ClearSpaceNode, PlanNodeEnum, ShowSpacesNode,
    ShowStatsNode, ShowStatsType, ShowUsersNode,
};
use crate::planning::plan::SubPlan;
use crate::planning::plan::{
    BeginTransactionNode, CommitNode, ReleaseSavepointNode, RollbackNode, SavepointNode,
};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use graphdb_core::types::PropertyDef;
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

    fn plan_show(&self, target: &ShowTarget, current_space: &str) -> PlanNodeEnum {
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
            ShowTarget::Users => {
                let show_users_node = ShowUsersNode::new(next_node_id());
                PlanNodeEnum::UserManage(crate::planning::plan::core::nodes::management::manage_node_enums::UserManageNode::ShowUsers(show_users_node))
            }
            ShowTarget::Roles => {
                let show_roles_node = crate::planning::plan::core::nodes::ShowRolesNode::new(
                    next_node_id(),
                    current_space.to_string(),
                );
                PlanNodeEnum::UserManage(crate::planning::plan::core::nodes::management::manage_node_enums::UserManageNode::ShowRoles(show_roles_node))
            }
            ShowTarget::Indexes => {
                let show_indexes_node =
                    ShowIndexesNode::new(next_node_id(), current_space.to_string());
                PlanNodeEnum::IndexManage(IndexManageNode::ShowIndexes(show_indexes_node))
            }
            ShowTarget::Tag(_) => {
                let show_tags_node = ShowTagsNode::new(next_node_id(), current_space.to_string());
                PlanNodeEnum::TagManage(TagManageNode::Show(show_tags_node))
            }
            ShowTarget::Edge(_) => {
                let show_edges_node = ShowEdgesNode::new(next_node_id(), current_space.to_string());
                PlanNodeEnum::EdgeManage(EdgeManageNode::Show(show_edges_node))
            }
            ShowTarget::Index(_) => {
                let show_indexes_node =
                    ShowIndexesNode::new(next_node_id(), current_space.to_string());
                PlanNodeEnum::IndexManage(IndexManageNode::ShowIndexes(show_indexes_node))
            }
        }
    }

    fn plan_show_create(
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

    fn plan_create(
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
                // Sequence creation will be handled by the executor in S-7
                Ok(None)
            }
        }
    }

    fn plan_alter(&self, target: &AlterTarget, current_space: &str) -> PlanNodeEnum {
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
                PlanNodeEnum::SpaceManage(SpaceManageNode::Alter(node))
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
                PlanNodeEnum::TagManage(TagManageNode::Alter(node))
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
                PlanNodeEnum::EdgeManage(EdgeManageNode::Alter(node))
            }
            AlterTarget::Sequence { .. } => {
                // Sequence alteration will be handled by the executor in S-7
                // Return a placeholder for now
                unreachable!("ALTER SEQUENCE planning not yet implemented")
            }
        }
    }

    fn plan_desc(
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

    fn plan_drop(
        &self,
        target: &crate::parser::ast::stmt::DropTarget,
        if_exists: bool,
        current_space: &str,
    ) -> PlanNodeEnum {
        use crate::parser::ast::stmt::DropTarget;

        match target {
            DropTarget::Tags(tag_names) if !tag_names.is_empty() => {
                let node = crate::planning::plan::core::nodes::DropTagNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    tag_names[0].clone(),
                )
                .with_if_exists(if_exists);
                PlanNodeEnum::TagManage(TagManageNode::Drop(node))
            }
            DropTarget::Edges(edge_names) if !edge_names.is_empty() => {
                let node = crate::planning::plan::core::nodes::DropEdgeNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    edge_names[0].clone(),
                )
                .with_if_exists(if_exists);
                PlanNodeEnum::EdgeManage(EdgeManageNode::Drop(node))
            }
            DropTarget::Space(space_name) => {
                let node = crate::planning::plan::core::nodes::DropSpaceNode::new(
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
                    current_space.to_string()
                } else {
                    space_name.clone()
                };
                let node = crate::planning::plan::core::nodes::DropTagIndexNode::new(
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
                    current_space.to_string()
                } else {
                    space_name.clone()
                };
                let node = crate::planning::plan::core::nodes::DropEdgeIndexNode::new(
                    next_node_id(),
                    resolved_space,
                    index_name.clone(),
                );
                PlanNodeEnum::IndexManage(IndexManageNode::DropEdgeIndex(node))
            }
            DropTarget::Tags(_) => {
                let node = crate::planning::plan::core::nodes::DropTagNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    String::new(),
                )
                .with_if_exists(if_exists);
                PlanNodeEnum::TagManage(TagManageNode::Drop(node))
            }
            DropTarget::Edges(_) => {
                let node = crate::planning::plan::core::nodes::DropEdgeNode::new(
                    next_node_id(),
                    current_space.to_string(),
                    String::new(),
                )
                .with_if_exists(if_exists);
                PlanNodeEnum::EdgeManage(EdgeManageNode::Drop(node))
            }
            DropTarget::Sequence(_) => {
                // Sequence dropping will be handled by the executor in S-7
                unreachable!("DROP SEQUENCE planning not yet implemented")
            }
        }
    }

    fn plan_migrate_plan(
        &self,
        stmt: &crate::parser::ast::MigratePlanStmt,
        _validated: &ValidatedStatement,
    ) -> PlanNodeEnum {
        let _ = stmt;
        let node = crate::planning::plan::core::nodes::ShowStatsNode::new(
            next_node_id(),
            crate::planning::plan::core::nodes::ShowStatsType::Storage,
        );
        PlanNodeEnum::ShowStats(node)
    }

    fn plan_migrate_execute(
        &self,
        stmt: &crate::parser::ast::MigrateExecuteStmt,
        _validated: &ValidatedStatement,
    ) -> PlanNodeEnum {
        let _ = stmt;
        let node = crate::planning::plan::core::nodes::ShowStatsNode::new(
            next_node_id(),
            crate::planning::plan::core::nodes::ShowStatsType::Storage,
        );
        PlanNodeEnum::ShowStats(node)
    }

    fn plan_migrate_rollback(
        &self,
        stmt: &crate::parser::ast::MigrateRollbackStmt,
        _validated: &ValidatedStatement,
    ) -> PlanNodeEnum {
        let _ = stmt;
        let node = crate::planning::plan::core::nodes::ShowStatsNode::new(
            next_node_id(),
            crate::planning::plan::core::nodes::ShowStatsType::Storage,
        );
        PlanNodeEnum::ShowStats(node)
    }
}

impl Planner for MaintainPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let validated = ctx.validated;
        let space = qctx
            .space_name()
            .or_else(|| qctx.request_context().space_name.clone())
            .unwrap_or_default();

        let final_node = match bound {
            BoundStatement::Show(s) => self.plan_show(&s.target, &space),
            BoundStatement::ShowCreate(s) => self.plan_show_create(&s.target, &space),
            BoundStatement::Drop(s) => self.plan_drop(&s.target, s.if_exists, &space),
            BoundStatement::Alter(s) => self.plan_alter(&s.target, &space),
            BoundStatement::Desc(s) => self.plan_desc(&s.target, &space),
            BoundStatement::ClearSpace(s) => {
                let node = ClearSpaceNode::new(next_node_id(), s.space_name.clone());
                PlanNodeEnum::SpaceManage(SpaceManageNode::Clear(node))
            }
            BoundStatement::BeginTransaction(_) => {
                let node = BeginTransactionNode::new(next_node_id());
                PlanNodeEnum::BeginTransaction(node)
            }
            BoundStatement::Commit(_) => {
                let node = CommitNode::new(next_node_id());
                PlanNodeEnum::Commit(node)
            }
            BoundStatement::Rollback(r) => {
                let mut node = RollbackNode::new(next_node_id());
                if let Some(savepoint) = &r.savepoint_name {
                    node = node.with_savepoint(savepoint.clone());
                }
                PlanNodeEnum::Rollback(node)
            }
            BoundStatement::Savepoint(s) => {
                let node = SavepointNode::new(next_node_id(), s.name.clone());
                PlanNodeEnum::Savepoint(node)
            }
            BoundStatement::ReleaseSavepoint(s) => {
                let node = ReleaseSavepointNode::new(next_node_id(), s.name.clone());
                PlanNodeEnum::ReleaseSavepoint(node)
            }
            // Schema-level CREATE, Migrate, session/management statements and
            // any other legacy AST-only statement still use the AST path.
            _ => return self.transform(validated, qctx),
        };

        Ok(SubPlan::from_single_node(final_node))
    }

    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let stmt = validated.stmt();
        let current_space = self.current_space(validated);

        let final_node = match stmt {
            Stmt::Show(show_stmt) => self.plan_show(&show_stmt.target, &current_space),

            Stmt::ShowCreate(show_create_stmt) => {
                self.plan_show_create(&show_create_stmt.target, &current_space)
            }

            Stmt::Create(create_stmt) => {
                if let Some(node) = self.plan_create(
                    &create_stmt.target,
                    create_stmt.if_not_exists,
                    &current_space,
                )? {
                    return Ok(SubPlan::from_single_node(node));
                }
                return Err(PlannerError::UnsupportedOperation(
                    "Create Node/Edge/Path is not supported by MaintainPlanner".to_string(),
                ));
            }

            Stmt::Alter(alter_stmt) => self.plan_alter(&alter_stmt.target, &current_space),

            Stmt::ClearSpace(clear_stmt) => {
                let node = ClearSpaceNode::new(next_node_id(), clear_stmt.space_name.clone());
                PlanNodeEnum::SpaceManage(SpaceManageNode::Clear(node))
            }

            Stmt::Desc(desc_stmt) => self.plan_desc(&desc_stmt.target, &current_space),

            Stmt::ShowConfigs(show_configs_stmt) => {
                let node = crate::planning::plan::core::nodes::ShowConfigsNode::new(
                    next_node_id(),
                    show_configs_stmt.module.clone(),
                );
                PlanNodeEnum::ShowConfigs(node)
            }

            Stmt::ShowQueries(_) => {
                let node = crate::planning::plan::core::nodes::ShowQueriesNode::new(next_node_id());
                PlanNodeEnum::ShowQueries(node)
            }

            Stmt::ShowSessions(_) => {
                let node =
                    crate::planning::plan::core::nodes::ShowSessionsNode::new(next_node_id());
                PlanNodeEnum::ShowSessions(node)
            }

            Stmt::BeginTransaction(_) => {
                let node = BeginTransactionNode::new(next_node_id());
                PlanNodeEnum::BeginTransaction(node)
            }

            Stmt::CommitTransaction(_) => {
                let node = CommitNode::new(next_node_id());
                PlanNodeEnum::Commit(node)
            }

            Stmt::RollbackTransaction(rollback_stmt) => {
                let mut node = RollbackNode::new(next_node_id());
                if let Some(savepoint) = &rollback_stmt.savepoint_name {
                    node = node.with_savepoint(savepoint.clone());
                }
                PlanNodeEnum::Rollback(node)
            }

            Stmt::Savepoint(savepoint_stmt) => {
                let node = SavepointNode::new(next_node_id(), savepoint_stmt.name.clone());
                PlanNodeEnum::Savepoint(node)
            }

            Stmt::ReleaseSavepoint(release_stmt) => {
                let node = ReleaseSavepointNode::new(next_node_id(), release_stmt.name.clone());
                PlanNodeEnum::ReleaseSavepoint(node)
            }

            Stmt::Drop(drop_stmt) => {
                self.plan_drop(&drop_stmt.target, drop_stmt.if_exists, &current_space)
            }

            Stmt::Migrate(m) => match m {
                crate::parser::ast::MigrateStmt::Plan(p) => self.plan_migrate_plan(p, validated),
                crate::parser::ast::MigrateStmt::Execute(e) => {
                    self.plan_migrate_execute(e, validated)
                }
                crate::parser::ast::MigrateStmt::Rollback(r) => {
                    self.plan_migrate_rollback(r, validated)
                }
            },

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
                | Stmt::ShowConfigs(_)
                | Stmt::ShowQueries(_)
                | Stmt::ShowSessions(_)
                | Stmt::BeginTransaction(_)
                | Stmt::CommitTransaction(_)
                | Stmt::RollbackTransaction(_)
                | Stmt::Savepoint(_)
                | Stmt::ReleaseSavepoint(_)
                | Stmt::Migrate(_)
        )
    }
}

impl Default for MaintainPlanner {
    fn default() -> Self {
        Self::new()
    }
}
