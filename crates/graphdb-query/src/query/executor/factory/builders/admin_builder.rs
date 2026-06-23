//! Management Executor Builder
//!
//! Responsible for creating and managing various types of executors (space management, tag management, edge management, index management, user management).

use crate::core::error::QueryError;
use crate::core::types::index::IndexConfig;
use crate::core::types::IndexField;
use crate::core::RoleType;
use crate::core::Value;
use crate::query::executor::admin::query_management::show_stats::ShowStatsType as ExecutorShowStatsType;
use crate::query::executor::admin::space::alter_space::SpaceAlterOption as ExecutorSpaceAlterOption;
use crate::query::executor::admin::{
    AlterEdgeExecutor, AlterSpaceExecutor, AlterTagExecutor, AlterUserExecutor,
    ChangePasswordExecutor, ClearSpaceExecutor, CreateEdgeExecutor, CreateEdgeIndexExecutor,
    CreateSpaceExecutor, CreateTagExecutor, CreateTagIndexExecutor, CreateUserExecutor,
    DescEdgeExecutor, DescEdgeIndexExecutor, DescSpaceExecutor, DescTagExecutor,
    DescTagIndexExecutor, DescribeUserExecutor, DropEdgeExecutor, DropEdgeIndexExecutor,
    DropSpaceExecutor, DropTagExecutor, DropTagIndexExecutor, DropUserExecutor, GrantRoleExecutor,
    RebuildEdgeIndexExecutor, RebuildTagIndexExecutor, RevokeRoleExecutor, ShowCreateTagExecutor,
    ShowEdgeIndexesExecutor, ShowEdgesExecutor, ShowSpacesExecutor, ShowStatsExecutor,
    ShowTagIndexesExecutor, ShowTagsExecutor, SwitchSpaceExecutor,
};
use crate::query::executor::base::{
    EdgeManageExecutor, ExecutionContext, ExecutorEnum, IndexManageExecutor, SpaceManageExecutor,
    TagManageExecutor, UserManageExecutor,
};
use crate::query::executor::utils::PassThroughExecutor;

use crate::query::planning::plan::core::nodes::management::{
    AlterEdgeNode, AlterSpaceNode, AlterTagNode, AlterUserNode, ChangePasswordNode, ClearSpaceNode,
    CreateEdgeIndexNode, CreateEdgeNode, CreateSpaceNode, CreateTagIndexNode, CreateTagNode,
    CreateUserNode, DescEdgeIndexNode, DescEdgeNode, DescSpaceNode, DescTagIndexNode, DescTagNode,
    DescribeUserNode, DropEdgeIndexNode, DropEdgeNode, DropSpaceNode, DropTagIndexNode,
    DropTagNode, DropUserNode, GrantRoleNode, RebuildEdgeIndexNode, RebuildTagIndexNode,
    RevokeRoleNode, ShowCreateEdgeNode, ShowCreateIndexNode, ShowCreateSpaceNode,
    ShowCreateTagNode, ShowEdgeIndexesNode, ShowEdgesNode, ShowIndexesNode, ShowRolesNode,
    ShowSpacesNode, ShowStatsNode, ShowTagIndexesNode, ShowTagsNode, ShowUsersNode,
    SwitchSpaceNode,
};
use crate::storage::StorageClient;
use parking_lot::RwLock;
use std::sync::Arc;

/// Management Executor Builder
pub struct AdminBuilder<S: StorageClient + Send + 'static> {
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageClient + Send + 'static> AdminBuilder<S> {
    /// Create a new management executor builder.
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    // Space Management Executor

    /// Building the CreateSpace executor
    pub fn build_create_space(
        node: &CreateSpaceNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::query::executor::admin::space::create_space::ExecutorSpaceInfo;
        let space_info = ExecutorSpaceInfo::new(node.info().space_name.clone())
            .with_vid_type(node.info().vid_type.clone());
        let executor = CreateSpaceExecutor::new(
            node.id(),
            storage,
            space_info,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::SpaceManage(SpaceManageExecutor::Create(
            executor,
        )))
    }

    /// Building the DropSpace executor
    pub fn build_drop_space(
        node: &DropSpaceNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = DropSpaceExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::SpaceManage(SpaceManageExecutor::Drop(
            executor,
        )))
    }

    /// Building the DescSpace executor
    pub fn build_desc_space(
        node: &DescSpaceNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = DescSpaceExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::SpaceManage(SpaceManageExecutor::Desc(
            executor,
        )))
    }

    /// Building the ShowSpaces executor
    pub fn build_show_spaces(
        _node: &ShowSpacesNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            ShowSpacesExecutor::new(_node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::SpaceManage(SpaceManageExecutor::Show(
            executor,
        )))
    }

    // Tag Management Executor

    /// Building the CreateTag executor
    pub fn build_create_tag(
        node: &CreateTagNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::query::executor::admin::tag::create_tag::ExecutorTagInfo;
        let tag_info =
            ExecutorTagInfo::new(node.info().space_name.clone(), node.info().tag_name.clone())
                .with_properties(node.info().properties.clone());

        let executor = if node.info().if_not_exists {
            CreateTagExecutor::with_if_not_exists(
                node.id(),
                storage,
                tag_info,
                context.expression_context().clone(),
            )
        } else {
            CreateTagExecutor::new(
                node.id(),
                storage,
                tag_info,
                context.expression_context().clone(),
            )
        };
        Ok(ExecutorEnum::TagManage(TagManageExecutor::Create(executor)))
    }

    /// Building the AlterTag executor
    pub fn build_alter_tag(
        node: &AlterTagNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::query::executor::admin::tag::alter_tag::{AlterTagInfo, AlterTagItem};
        let mut items = Vec::new();
        for prop in node.info().additions.iter() {
            items.push(AlterTagItem::add_property(prop.clone()));
        }
        for prop_name in node.info().deletions.iter() {
            items.push(AlterTagItem::drop_property(prop_name.clone()));
        }
        for change in node.info().changes.iter() {
            items.push(AlterTagItem::change_property(
                change.old_name.clone(),
                change.new_name.clone(),
                change.data_type.clone(),
            ));
        }
        let alter_info =
            AlterTagInfo::new(node.info().space_name.clone(), node.info().tag_name.clone())
                .with_items(items);
        let executor = AlterTagExecutor::new(
            node.id(),
            storage,
            alter_info,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::TagManage(TagManageExecutor::Alter(executor)))
    }

    /// Building the DescTag executor
    pub fn build_desc_tag(
        node: &DescTagNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = DescTagExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            node.tag_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::TagManage(TagManageExecutor::Desc(executor)))
    }

    /// Building the DropTag executor
    pub fn build_drop_tag(
        node: &DropTagNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = if node.if_exists() {
            DropTagExecutor::with_if_exists(
                node.id(),
                storage,
                node.space_name().to_string(),
                node.tag_name().to_string(),
                context.expression_context().clone(),
            )
        } else {
            DropTagExecutor::new(
                node.id(),
                storage,
                node.space_name().to_string(),
                node.tag_name().to_string(),
                context.expression_context().clone(),
            )
        };
        Ok(ExecutorEnum::TagManage(TagManageExecutor::Drop(executor)))
    }

    /// Constructing the ShowTags executor
    pub fn build_show_tags(
        node: &ShowTagsNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = ShowTagsExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::TagManage(TagManageExecutor::Show(executor)))
    }

    /// Building the ShowCreateTag executor
    pub fn build_show_create_tag(
        node: &ShowCreateTagNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = ShowCreateTagExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            node.tag_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::TagManage(TagManageExecutor::ShowCreate(
            executor,
        )))
    }

    //  Ellison Type Management Executor ============

    /// Building the CreateEdge executor
    pub fn build_create_edge(
        node: &CreateEdgeNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::query::executor::admin::edge::create_edge::ExecutorEdgeInfo;
        let src_tag = node.info().src_tag_name.clone().unwrap_or_default();
        let dst_tag = node.info().dst_tag_name.clone().unwrap_or_default();
        let edge_info = ExecutorEdgeInfo {
            space_name: node.info().space_name.clone(),
            edge_name: node.info().edge_name.clone(),
            src_tag_name: src_tag,
            dst_tag_name: dst_tag,
            properties: node.info().properties.clone(),
            comment: None,
        };

        let executor = if node.info().if_not_exists {
            CreateEdgeExecutor::with_if_not_exists(
                node.id(),
                storage,
                edge_info,
                context.expression_context().clone(),
            )
        } else {
            CreateEdgeExecutor::new(
                node.id(),
                storage,
                edge_info,
                context.expression_context().clone(),
            )
        };
        Ok(ExecutorEnum::EdgeManage(EdgeManageExecutor::Create(
            executor,
        )))
    }

    /// Building the AlterEdge executor
    pub fn build_alter_edge(
        node: &AlterEdgeNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::query::executor::admin::edge::alter_edge::{AlterEdgeInfo, AlterEdgeItem};
        let mut items = Vec::new();
        for prop in node.info().additions.iter() {
            items.push(AlterEdgeItem::add_property(prop.clone()));
        }
        for prop_name in node.info().deletions.iter() {
            items.push(AlterEdgeItem::drop_property(prop_name.clone()));
        }
        let alter_info = AlterEdgeInfo::new(
            node.info().space_name.clone(),
            node.info().edge_name.clone(),
        )
        .with_items(items);
        let executor = AlterEdgeExecutor::new(
            node.id(),
            storage,
            alter_info,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::EdgeManage(EdgeManageExecutor::Alter(
            executor,
        )))
    }

    /// Building the DescEdge executor
    pub fn build_desc_edge(
        node: &DescEdgeNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = DescEdgeExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            node.edge_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::EdgeManage(EdgeManageExecutor::Desc(executor)))
    }

    /// Building the DropEdge executor
    pub fn build_drop_edge(
        node: &DropEdgeNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = if node.if_exists() {
            DropEdgeExecutor::with_if_exists(
                node.id(),
                storage,
                node.space_name().to_string(),
                node.edge_name().to_string(),
                context.expression_context().clone(),
            )
        } else {
            DropEdgeExecutor::new(
                node.id(),
                storage,
                node.space_name().to_string(),
                node.edge_name().to_string(),
                context.expression_context().clone(),
            )
        };
        Ok(ExecutorEnum::EdgeManage(EdgeManageExecutor::Drop(executor)))
    }

    /// Constructing the ShowEdges executor
    pub fn build_show_edges(
        node: &ShowEdgesNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = ShowEdgesExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::EdgeManage(EdgeManageExecutor::Show(executor)))
    }

    // Tag Index Management Executor

    /// Construct the CreateTagIndex executor.
    pub fn build_create_tag_index(
        node: &CreateTagIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::core::types::{Index, IndexType};
        let fields = node
            .info()
            .properties
            .iter()
            .map(|prop| IndexField::new(prop.clone(), Value::String("string".to_string()), false))
            .collect();
        let index = Index::new(IndexConfig {
            id: 0,
            name: node.info().index_name.clone(),
            space_id: 0,
            schema_name: node.info().target_name.clone(),
            fields,
            properties: node.info().properties.clone(),
            index_type: IndexType::TagIndex,
            is_unique: false,
            partial_condition: None,
        });
        let executor = CreateTagIndexExecutor::new(
            node.id(),
            storage,
            node.info().space_name.clone(),
            index,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::IndexManage(
            IndexManageExecutor::CreateTagIndex(executor),
        ))
    }

    /// Building the DropTagIndex executor
    pub fn build_drop_tag_index(
        node: &DropTagIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = DropTagIndexExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            node.index_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::IndexManage(
            IndexManageExecutor::DropTagIndex(executor),
        ))
    }

    /// Constructing the DescTagIndex executor
    pub fn build_desc_tag_index(
        node: &DescTagIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = DescTagIndexExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            node.index_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::IndexManage(
            IndexManageExecutor::DescTagIndex(executor),
        ))
    }

    /// Constructing the ShowTagIndexes executor
    pub fn build_show_tag_indexes(
        node: &ShowTagIndexesNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = ShowTagIndexesExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::IndexManage(
            IndexManageExecutor::ShowTagIndexes(executor),
        ))
    }

    /// Constructing the RebuildTagIndex executor
    pub fn build_rebuild_tag_index(
        node: &RebuildTagIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = RebuildTagIndexExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            node.index_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::IndexManage(
            IndexManageExecutor::RebuildTagIndex(executor),
        ))
    }

    // ========== Side Index Management Executor ----------

    /// Build the CreateEdgeIndex executor.
    pub fn build_create_edge_index(
        node: &CreateEdgeIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            CreateEdgeIndexExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::IndexManage(
            IndexManageExecutor::CreateEdgeIndex(executor),
        ))
    }

    /// Constructing the DropEdgeIndex executor
    pub fn build_drop_edge_index(
        node: &DropEdgeIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            DropEdgeIndexExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::IndexManage(
            IndexManageExecutor::DropEdgeIndex(executor),
        ))
    }

    /// Constructing the DescEdgeIndex executor
    pub fn build_desc_edge_index(
        node: &DescEdgeIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            DescEdgeIndexExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::IndexManage(
            IndexManageExecutor::DescEdgeIndex(executor),
        ))
    }

    /// Constructing the ShowEdgeIndexes executor
    pub fn build_show_edge_indexes(
        node: &ShowEdgeIndexesNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            ShowEdgeIndexesExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::IndexManage(
            IndexManageExecutor::ShowEdgeIndexes(executor),
        ))
    }

    /// Constructing the RebuildEdgeIndex executor
    pub fn build_rebuild_edge_index(
        node: &RebuildEdgeIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            RebuildEdgeIndexExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::IndexManage(
            IndexManageExecutor::RebuildEdgeIndex(executor),
        ))
    }

    // >User Management Executor==========

    /// Constructing the CreateUser executor
    pub fn build_create_user(
        node: &CreateUserNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::core::types::UserInfo;
        let user_info = UserInfo::new(node.username().to_string(), node.password().to_string())
            .map_err(|e| {
                QueryError::execution(format!("Failed to create user information: {}", e))
            })?;
        let executor = if node.if_not_exists() {
            CreateUserExecutor::with_if_not_exists(
                node.id(),
                storage,
                user_info,
                context.expression_context().clone(),
            )
        } else {
            CreateUserExecutor::new(
                node.id(),
                storage,
                user_info,
                context.expression_context().clone(),
            )
        };
        Ok(ExecutorEnum::UserManage(UserManageExecutor::Create(
            executor,
        )))
    }

    /// Constructing the AlterUser executor
    pub fn build_alter_user(
        node: &AlterUserNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::core::types::UserAlterInfo;
        // AlterUserNode 使用 username() 方法
        // The `AlterUserExecutor::new` method requires a `UserAlterInfo` object.
        let alter_info = UserAlterInfo::new(node.username().to_string());
        let executor = AlterUserExecutor::new(
            node.id(),
            storage,
            alter_info,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::UserManage(UserManageExecutor::Alter(
            executor,
        )))
    }

    /// Building the DropUser executor
    pub fn build_drop_user(
        node: &DropUserNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = if node.if_exists() {
            DropUserExecutor::with_if_exists(
                node.id(),
                storage,
                node.username().to_string(),
                context.expression_context().clone(),
            )
        } else {
            DropUserExecutor::new(
                node.id(),
                storage,
                node.username().to_string(),
                context.expression_context().clone(),
            )
        };
        Ok(ExecutorEnum::UserManage(UserManageExecutor::Drop(executor)))
    }

    /// Constructing the ChangePassword executor
    pub fn build_change_password(
        node: &ChangePasswordNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // ChangePasswordNode 使用 password_info() 方法获取 PasswordInfo
        let password_info = node.password_info();
        let username = password_info.username.clone().unwrap_or_default();
        let executor = ChangePasswordExecutor::new(
            node.id(),
            storage,
            Some(username),
            password_info.old_password.clone(),
            password_info.new_password.clone(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::UserManage(
            UserManageExecutor::ChangePassword(executor),
        ))
    }

    /// Building the GrantRole executor
    pub fn build_grant_role(
        node: &GrantRoleNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let role = match node.role() {
            "admin" => RoleType::Admin,
            "dba" => RoleType::Dba,
            "user" => RoleType::User,
            "guest" => RoleType::Guest,
            _ => RoleType::User,
        };
        let executor = GrantRoleExecutor::new(
            node.id(),
            storage,
            node.username().to_string(),
            node.space_name().to_string(),
            role,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::UserManage(UserManageExecutor::GrantRole(
            executor,
        )))
    }

    /// Building the RevokeRole executor
    pub fn build_revoke_role(
        node: &RevokeRoleNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = RevokeRoleExecutor::new(
            node.id(),
            storage,
            node.username().to_string(),
            node.space_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::UserManage(UserManageExecutor::RevokeRole(
            executor,
        )))
    }

    /// Building the SwitchSpace executor
    pub fn build_switch_space(
        node: &SwitchSpaceNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = SwitchSpaceExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::SpaceManage(SpaceManageExecutor::Switch(
            executor,
        )))
    }

    /// Building the AlterSpace executor
    pub fn build_alter_space(
        node: &AlterSpaceNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let options: Vec<ExecutorSpaceAlterOption> = node
            .options()
            .iter()
            .map(|opt| match opt {
                crate::query::planning::plan::core::nodes::SpaceAlterOption::Comment(c) => {
                    ExecutorSpaceAlterOption::Comment(c.clone())
                }
            })
            .collect();
        let executor = AlterSpaceExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            options,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::SpaceManage(SpaceManageExecutor::Alter(
            executor,
        )))
    }

    /// Building the ClearSpace executor
    pub fn build_clear_space(
        node: &ClearSpaceNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = ClearSpaceExecutor::new(
            node.id(),
            storage,
            node.space_name().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::SpaceManage(SpaceManageExecutor::Clear(
            executor,
        )))
    }

    /// Building the ShowStats executor
    pub fn build_show_stats(
        node: &ShowStatsNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let stats_type = match node.stats_type() {
            crate::query::planning::plan::core::nodes::ShowStatsType::Storage => {
                ExecutorShowStatsType::Storage
            }
            crate::query::planning::plan::core::nodes::ShowStatsType::Space => {
                ExecutorShowStatsType::Space
            }
        };
        let executor = ShowStatsExecutor::new(
            node.id(),
            storage,
            stats_type,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::ShowStats(executor))
    }

    pub fn build_show_create_space(
        node: &ShowCreateSpaceNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            PassThroughExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::PassThrough(executor))
    }

    pub fn build_show_create_edge(
        node: &ShowCreateEdgeNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            PassThroughExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::PassThrough(executor))
    }

    pub fn build_show_indexes(
        node: &ShowIndexesNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            PassThroughExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::PassThrough(executor))
    }

    pub fn build_show_create_index(
        node: &ShowCreateIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            PassThroughExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::PassThrough(executor))
    }

    pub fn build_describe_user(
        node: &DescribeUserNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = DescribeUserExecutor::new(
            node.id(),
            storage,
            node.username().to_string(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::UserManage(UserManageExecutor::Describe(
            executor,
        )))
    }

    pub fn build_show_users(
        node: &ShowUsersNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            PassThroughExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::PassThrough(executor))
    }

    pub fn build_show_roles(
        node: &ShowRolesNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            PassThroughExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::PassThrough(executor))
    }
}

impl<S: StorageClient + 'static> Default for AdminBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}
