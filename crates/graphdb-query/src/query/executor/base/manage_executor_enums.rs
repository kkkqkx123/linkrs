//! Management executor sub-enum definitions
//!
//! Groups management executors into category-based sub-enums to reduce
//! the number of variants in ExecutorEnum from 90+ to ~50.
//!
//! # Categories
//! - SpaceManageExecutor: Space DDL executors (CREATE/DROP/DESC/SHOW/ALTER/CLEAR SPACE)
//! - TagManageExecutor: Tag DDL executors (CREATE/ALTER/DESC/DROP/SHOW TAG)
//! - EdgeManageExecutor: Edge DDL executors (CREATE/ALTER/DESC/DROP/SHOW EDGE)
//! - IndexManageExecutor: Index DDL executors (CREATE/DROP/DESC/SHOW/REBUILD INDEX)
//! - UserManageExecutor: User DDL executors (CREATE/ALTER/DROP USER, GRANT/REVOKE ROLE)
//! - FulltextManageExecutor: Fulltext index DDL executors
//! - VectorManageExecutor: Vector index DDL executors

use crate::query::executor::admin::edge::{
    AlterEdgeExecutor, CreateEdgeExecutor, DescEdgeExecutor, DropEdgeExecutor, ShowEdgesExecutor,
};
#[cfg(feature = "fulltext-search")]
use crate::query::executor::admin::index::{
    AlterFulltextIndexExecutor, CreateFulltextIndexExecutor, DescribeFulltextIndexExecutor,
    DropFulltextIndexExecutor, ShowFulltextIndexExecutor,
};
use crate::query::executor::admin::index::{
    CreateEdgeIndexExecutor, CreateTagIndexExecutor, DescEdgeIndexExecutor, DescTagIndexExecutor,
    DropEdgeIndexExecutor, DropTagIndexExecutor, RebuildEdgeIndexExecutor, RebuildTagIndexExecutor,
    ShowEdgeIndexesExecutor, ShowTagIndexesExecutor,
};
use crate::query::executor::admin::space::{
    AlterSpaceExecutor, ClearSpaceExecutor, CreateSpaceExecutor, DescSpaceExecutor,
    DropSpaceExecutor, ShowSpacesExecutor, SwitchSpaceExecutor,
};
use crate::query::executor::admin::tag::{
    AlterTagExecutor, CreateTagExecutor, DescTagExecutor, DropTagExecutor, ShowCreateTagExecutor,
    ShowTagsExecutor,
};
use crate::query::executor::admin::user::{
    AlterUserExecutor, ChangePasswordExecutor, CreateUserExecutor, DescribeUserExecutor,
    DropUserExecutor, GrantRoleExecutor, RevokeRoleExecutor,
};
#[cfg(feature = "qdrant")]
use crate::query::executor::data_access::{CreateVectorIndexExecutor, DropVectorIndexExecutor};
use crate::storage::StorageClient;
macro_rules! define_manage_executor_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $variant:ident($executor_type:ty, $type_id:expr, $type_name:expr) ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[allow(clippy::large_enum_variant)]
        pub enum $name<S: StorageClient + Send + 'static> {
            $( $variant($executor_type), )*
        }

        impl<S: StorageClient + Send + 'static> crate::query::executor::base::Executor<S> for $name<S> {
            fn execute(&mut self) -> crate::core::error::DBResult<crate::query::executor::base::ExecutionResult> {
                match self {
                    $( $name::$variant(exec) => exec.execute(), )*
                }
            }

            fn open(&mut self) -> crate::core::error::DBResult<()> {
                match self {
                    $( $name::$variant(exec) => exec.open(), )*
                }
            }

            fn close(&mut self) -> crate::core::error::DBResult<()> {
                match self {
                    $( $name::$variant(exec) => exec.close(), )*
                }
            }

            fn is_open(&self) -> bool {
                match self {
                    $( $name::$variant(exec) => exec.is_open(), )*
                }
            }

            fn id(&self) -> i64 {
                match self {
                    $( $name::$variant(exec) => exec.id(), )*
                }
            }

            fn name(&self) -> &str {
                match self {
                    $( $name::$variant(exec) => exec.name(), )*
                }
            }

            fn description(&self) -> &str {
                match self {
                    $( $name::$variant(exec) => exec.description(), )*
                }
            }

            fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
                match self {
                    $( $name::$variant(exec) => exec.stats(), )*
                }
            }

            fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
                match self {
                    $( $name::$variant(exec) => exec.stats_mut(), )*
                }
            }
        }

        impl<S: StorageClient + Send + 'static> crate::query::core::NodeType for $name<S> {
            fn node_type_id(&self) -> &'static str {
                match self {
                    $( $name::$variant(_) => $type_id, )*
                }
            }

            fn node_type_name(&self) -> &'static str {
                match self {
                    $( $name::$variant(_) => $type_name, )*
                }
            }

            fn category(&self) -> crate::query::core::NodeCategory {
                crate::query::core::NodeCategory::Admin
            }
        }
    };
}

define_manage_executor_enum! {
    /// Space management executor sub-enum
    pub enum SpaceManageExecutor {
        Create(CreateSpaceExecutor<S>, "create_space", "Create Space"),
        Drop(DropSpaceExecutor<S>, "drop_space", "Drop Space"),
        Desc(DescSpaceExecutor<S>, "desc_space", "Describe Space"),
        Show(ShowSpacesExecutor<S>, "show_spaces", "Show Spaces"),
        Switch(SwitchSpaceExecutor<S>, "switch_space", "Switch Space"),
        Alter(AlterSpaceExecutor<S>, "alter_space", "Alter Space"),
        Clear(ClearSpaceExecutor<S>, "clear_space", "Clear Space"),
    }
}

define_manage_executor_enum! {
    /// Tag management executor sub-enum
    pub enum TagManageExecutor {
        Create(CreateTagExecutor<S>, "create_tag", "Create Tag"),
        Alter(AlterTagExecutor<S>, "alter_tag", "Alter Tag"),
        Desc(DescTagExecutor<S>, "desc_tag", "Describe Tag"),
        Drop(DropTagExecutor<S>, "drop_tag", "Drop Tag"),
        Show(ShowTagsExecutor<S>, "show_tags", "Show Tags"),
        ShowCreate(ShowCreateTagExecutor<S>, "show_create_tag", "Show Create Tag"),
    }
}

define_manage_executor_enum! {
    /// Edge management executor sub-enum
    pub enum EdgeManageExecutor {
        Create(CreateEdgeExecutor<S>, "create_edge", "Create Edge"),
        Alter(AlterEdgeExecutor<S>, "alter_edge", "Alter Edge"),
        Desc(DescEdgeExecutor<S>, "desc_edge", "Describe Edge"),
        Drop(DropEdgeExecutor<S>, "drop_edge", "Drop Edge"),
        Show(ShowEdgesExecutor<S>, "show_edges", "Show Edges"),
    }
}

define_manage_executor_enum! {
    /// Index management executor sub-enum
    pub enum IndexManageExecutor {
        CreateTagIndex(CreateTagIndexExecutor<S>, "create_tag_index", "Create Tag Index"),
        DropTagIndex(DropTagIndexExecutor<S>, "drop_tag_index", "Drop Tag Index"),
        DescTagIndex(DescTagIndexExecutor<S>, "desc_tag_index", "Describe Tag Index"),
        ShowTagIndexes(ShowTagIndexesExecutor<S>, "show_tag_indexes", "Show Tag Indexes"),
        RebuildTagIndex(RebuildTagIndexExecutor<S>, "rebuild_tag_index", "Rebuild Tag Index"),
        CreateEdgeIndex(CreateEdgeIndexExecutor<S>, "create_edge_index", "Create Edge Index"),
        DropEdgeIndex(DropEdgeIndexExecutor<S>, "drop_edge_index", "Drop Edge Index"),
        DescEdgeIndex(DescEdgeIndexExecutor<S>, "desc_edge_index", "Describe Edge Index"),
        ShowEdgeIndexes(ShowEdgeIndexesExecutor<S>, "show_edge_indexes", "Show Edge Indexes"),
        RebuildEdgeIndex(RebuildEdgeIndexExecutor<S>, "rebuild_edge_index", "Rebuild Edge Index"),
    }
}

define_manage_executor_enum! {
    /// User management executor sub-enum
    pub enum UserManageExecutor {
        Create(CreateUserExecutor<S>, "create_user", "Create User"),
        Alter(AlterUserExecutor<S>, "alter_user", "Alter User"),
        Drop(DropUserExecutor<S>, "drop_user", "Drop User"),
        ChangePassword(ChangePasswordExecutor<S>, "change_password", "Change Password"),
        GrantRole(GrantRoleExecutor<S>, "grant_role", "Grant Role"),
        RevokeRole(RevokeRoleExecutor<S>, "revoke_role", "Revoke Role"),
        Describe(DescribeUserExecutor<S>, "describe_user", "Describe User"),
    }
}

#[cfg(feature = "fulltext-search")]
define_manage_executor_enum! {
    /// Fulltext index management executor sub-enum
    pub enum FulltextManageExecutor {
        Create(CreateFulltextIndexExecutor<S>, "create_fulltext_index", "Create Fulltext Index"),
        Drop(DropFulltextIndexExecutor<S>, "drop_fulltext_index", "Drop Fulltext Index"),
        Alter(AlterFulltextIndexExecutor<S>, "alter_fulltext_index", "Alter Fulltext Index"),
        Show(ShowFulltextIndexExecutor<S>, "show_fulltext_index", "Show Fulltext Index"),
        Describe(DescribeFulltextIndexExecutor<S>, "describe_fulltext_index", "Describe Fulltext Index"),
    }
}

#[cfg(feature = "qdrant")]
define_manage_executor_enum! {
    /// Vector index management executor sub-enum
    pub enum VectorManageExecutor {
        Create(CreateVectorIndexExecutor<S>, "create_vector_index", "Create Vector Index"),
        Drop(DropVectorIndexExecutor<S>, "drop_vector_index", "Drop Vector Index"),
    }
}
