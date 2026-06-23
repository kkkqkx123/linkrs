//! Management node sub-enum definitions
//!
//! Groups management plan nodes into category-based sub-enums to reduce
//! the number of variants in PlanNodeEnum from 90+ to ~50.
//!
//! # Categories
//! - SpaceManageNode: Space DDL operations (CREATE/DROP/DESC/SHOW/ALTER/CLEAR SPACE)
//! - TagManageNode: Tag DDL operations (CREATE/ALTER/DESC/DROP/SHOW TAG)
//! - EdgeManageNode: Edge DDL operations (CREATE/ALTER/DESC/DROP/SHOW EDGE)
//! - IndexManageNode: Index DDL operations (CREATE/DROP/DESC/SHOW/REBUILD INDEX)
//! - UserManageNode: User DDL operations (CREATE/ALTER/DROP USER, GRANT/REVOKE ROLE)
//! - FulltextManageNode: Fulltext index DDL operations
//! - VectorManageNode: Vector index DDL operations

use crate::query::core::{NodeCategory, NodeType, NodeTypeMapping};
use crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{PlanNode, ZeroInputNode};
use crate::query::planning::plan::core::nodes::management::edge_nodes::{
    AlterEdgeNode, CreateEdgeNode, DescEdgeNode, DropEdgeNode, ShowCreateEdgeNode, ShowEdgesNode,
};
use crate::query::planning::plan::core::nodes::management::index_nodes::{
    CreateEdgeIndexNode, CreateTagIndexNode, DescEdgeIndexNode, DescTagIndexNode,
    DropEdgeIndexNode, DropTagIndexNode, RebuildEdgeIndexNode, RebuildTagIndexNode,
    ShowCreateIndexNode, ShowEdgeIndexesNode, ShowIndexesNode, ShowTagIndexesNode,
};
use crate::query::planning::plan::core::nodes::management::space_nodes::{
    AlterSpaceNode, ClearSpaceNode, CreateSpaceNode, DescSpaceNode, DropSpaceNode,
    ShowCreateSpaceNode, ShowSpacesNode, SwitchSpaceNode,
};
use crate::query::planning::plan::core::nodes::management::tag_nodes::{
    AlterTagNode, CreateTagNode, DescTagNode, DropTagNode, ShowCreateTagNode, ShowTagsNode,
};
use crate::query::planning::plan::core::nodes::management::user_nodes::{
    AlterUserNode, ChangePasswordNode, CreateUserNode, DescribeUserNode, DropUserNode,
    GrantRoleNode, RevokeRoleNode, ShowRolesNode, ShowUsersNode,
};
use crate::query::planning::plan::core::nodes::search::fulltext::management::{
    AlterFulltextIndexNode, CreateFulltextIndexNode, DescribeFulltextIndexNode,
    DropFulltextIndexNode, ShowFulltextIndexNode,
};
use crate::query::planning::plan::core::nodes::search::vector::management::{
    CreateVectorIndexNode, DropVectorIndexNode,
};

macro_rules! define_manage_node_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident as $enum_variant:ident {
            $( $variant:ident($node_type:ty, $node_name:expr, $type_id:expr, $type_name:expr, $executor_type:expr) ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        #[allow(clippy::large_enum_variant)]
        pub enum $name {
            $( $variant($node_type), )*
        }

        impl $name {
            pub fn node_name(&self) -> &'static str {
                match self {
                    $( $name::$variant(_) => $node_name, )*
                }
            }

            pub fn node_id(&self) -> i64 {
                match self {
                    $( $name::$variant(n) => n.id(), )*
                }
            }

            pub fn output_var(&self) -> Option<&str> {
                match self {
                    $( $name::$variant(n) => n.output_var(), )*
                }
            }

            pub fn col_names(&self) -> &[String] {
                match self {
                    $( $name::$variant(n) => n.col_names(), )*
                }
            }

            pub fn set_output_var(&mut self, var: String) {
                match self {
                    $( $name::$variant(n) => n.set_output_var(var), )*
                }
            }

            pub fn set_col_names(&mut self, names: Vec<String>) {
                match self {
                    $( $name::$variant(n) => n.set_col_names(names), )*
                }
            }
        }

        impl PlanNode for $name {
            fn id(&self) -> i64 {
                self.node_id()
            }

            fn name(&self) -> &'static str {
                self.node_name()
            }

            fn category(&self) -> PlanNodeCategory {
                PlanNodeCategory::Management
            }

            fn output_var(&self) -> Option<&str> {
                self.output_var()
            }

            fn col_names(&self) -> &[String] {
                self.col_names()
            }

            fn set_output_var(&mut self, var: String) {
                self.set_output_var(var);
            }

            fn set_col_names(&mut self, names: Vec<String>) {
                self.set_col_names(names);
            }

            fn into_enum(
                self,
            ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self)
            }
        }

        impl ZeroInputNode for $name {}

        impl MemoryEstimatable for $name {
            fn estimate_memory(&self) -> usize {
                match self {
                    $( $name::$variant(n) => n.estimate_memory(), )*
                }
            }
        }

        impl NodeType for $name {
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

            fn category(&self) -> NodeCategory {
                NodeCategory::Admin
            }
        }

        impl NodeTypeMapping for $name {
            fn corresponding_executor_type(&self) -> Option<&'static str> {
                match self {
                    $( $name::$variant(_) => Some($executor_type), )*
                }
            }
        }
    };
}

define_manage_node_enum! {
    /// Space management sub-enum
    pub enum SpaceManageNode as SpaceManage {
        Create(CreateSpaceNode, "CreateSpace", "create_space", "Create Space", "create_space"),
        Drop(DropSpaceNode, "DropSpace", "drop_space", "Drop Space", "drop_space"),
        Desc(DescSpaceNode, "DescSpace", "desc_space", "Describe Space", "desc_space"),
        Show(ShowSpacesNode, "ShowSpaces", "show_spaces", "Show Spaces", "show_spaces"),
        ShowCreate(ShowCreateSpaceNode, "ShowCreateSpace", "show_create_space", "Show Create Space", "show_create_space"),
        Switch(SwitchSpaceNode, "SwitchSpace", "switch_space", "Switch Space", "switch_space"),
        Alter(AlterSpaceNode, "AlterSpace", "alter_space", "Alter Space", "alter_space"),
        Clear(ClearSpaceNode, "ClearSpace", "clear_space", "Clear Space", "clear_space"),
    }
}

define_manage_node_enum! {
    /// Tag management sub-enum
    pub enum TagManageNode as TagManage {
        Create(CreateTagNode, "CreateTag", "create_tag", "Create Tag", "create_tag"),
        Alter(AlterTagNode, "AlterTag", "alter_tag", "Alter Tag", "alter_tag"),
        Desc(DescTagNode, "DescTag", "desc_tag", "Describe Tag", "desc_tag"),
        Drop(DropTagNode, "DropTag", "drop_tag", "Drop Tag", "drop_tag"),
        Show(ShowTagsNode, "ShowTags", "show_tags", "Show Tags", "show_tags"),
        ShowCreate(ShowCreateTagNode, "ShowCreateTag", "show_create_tag", "Show Create Tag", "show_create_tag"),
    }
}

define_manage_node_enum! {
    /// Edge management sub-enum
    pub enum EdgeManageNode as EdgeManage {
        Create(CreateEdgeNode, "CreateEdge", "create_edge", "Create Edge", "create_edge"),
        Alter(AlterEdgeNode, "AlterEdge", "alter_edge", "Alter Edge", "alter_edge"),
        Desc(DescEdgeNode, "DescEdge", "desc_edge", "Describe Edge", "desc_edge"),
        Drop(DropEdgeNode, "DropEdge", "drop_edge", "Drop Edge", "drop_edge"),
        Show(ShowEdgesNode, "ShowEdges", "show_edges", "Show Edges", "show_edges"),
        ShowCreate(ShowCreateEdgeNode, "ShowCreateEdge", "show_create_edge", "Show Create Edge", "show_create_edge"),
    }
}

define_manage_node_enum! {
    /// Index management sub-enum
    pub enum IndexManageNode as IndexManage {
        CreateTagIndex(CreateTagIndexNode, "CreateTagIndex", "create_tag_index", "Create Tag Index", "create_tag_index"),
        DropTagIndex(DropTagIndexNode, "DropTagIndex", "drop_tag_index", "Drop Tag Index", "drop_tag_index"),
        DescTagIndex(DescTagIndexNode, "DescTagIndex", "desc_tag_index", "Describe Tag Index", "desc_tag_index"),
        ShowTagIndexes(ShowTagIndexesNode, "ShowTagIndexes", "show_tag_indexes", "Show Tag Indexes", "show_tag_indexes"),
        RebuildTagIndex(RebuildTagIndexNode, "RebuildTagIndex", "rebuild_tag_index", "Rebuild Tag Index", "rebuild_tag_index"),
        CreateEdgeIndex(CreateEdgeIndexNode, "CreateEdgeIndex", "create_edge_index", "Create Edge Index", "create_edge_index"),
        DropEdgeIndex(DropEdgeIndexNode, "DropEdgeIndex", "drop_edge_index", "Drop Edge Index", "drop_edge_index"),
        DescEdgeIndex(DescEdgeIndexNode, "DescEdgeIndex", "desc_edge_index", "Describe Edge Index", "desc_edge_index"),
        ShowEdgeIndexes(ShowEdgeIndexesNode, "ShowEdgeIndexes", "show_edge_indexes", "Show Edge Indexes", "show_edge_indexes"),
        RebuildEdgeIndex(RebuildEdgeIndexNode, "RebuildEdgeIndex", "rebuild_edge_index", "Rebuild Edge Index", "rebuild_edge_index"),
        ShowIndexes(ShowIndexesNode, "ShowIndexes", "show_indexes", "Show Indexes", "show_indexes"),
        ShowCreateIndex(ShowCreateIndexNode, "ShowCreateIndex", "show_create_index", "Show Create Index", "show_create_index"),
    }
}

define_manage_node_enum! {
    /// User management sub-enum
    pub enum UserManageNode as UserManage {
        Create(CreateUserNode, "CreateUser", "create_user", "Create User", "create_user"),
        Alter(AlterUserNode, "AlterUser", "alter_user", "Alter User", "alter_user"),
        Drop(DropUserNode, "DropUser", "drop_user", "Drop User", "drop_user"),
        ChangePassword(ChangePasswordNode, "ChangePassword", "change_password", "Change Password", "change_password"),
        GrantRole(GrantRoleNode, "GrantRole", "grant_role", "Grant Role", "grant_role"),
        RevokeRole(RevokeRoleNode, "RevokeRole", "revoke_role", "Revoke Role", "revoke_role"),
        DescribeUser(DescribeUserNode, "DescribeUser", "describe_user", "Describe User", "describe_user"),
        ShowRoles(ShowRolesNode, "ShowRoles", "show_roles", "Show Roles", "show_roles"),
        ShowUsers(ShowUsersNode, "ShowUsers", "show_users", "Show Users", "show_users"),
    }
}

define_manage_node_enum! {
    /// Fulltext index management sub-enum
    pub enum FulltextManageNode as FulltextManage {
        Create(CreateFulltextIndexNode, "CreateFulltextIndex", "create_fulltext_index", "Create Fulltext Index", "create_fulltext_index"),
        Drop(DropFulltextIndexNode, "DropFulltextIndex", "drop_fulltext_index", "Drop Fulltext Index", "drop_fulltext_index"),
        Alter(AlterFulltextIndexNode, "AlterFulltextIndex", "alter_fulltext_index", "Alter Fulltext Index", "alter_fulltext_index"),
        Show(ShowFulltextIndexNode, "ShowFulltextIndex", "show_fulltext_index", "Show Fulltext Index", "show_fulltext_index"),
        Describe(DescribeFulltextIndexNode, "DescribeFulltextIndex", "describe_fulltext_index", "Describe Fulltext Index", "describe_fulltext_index"),
    }
}

define_manage_node_enum! {
    /// Vector index management sub-enum
    pub enum VectorManageNode as VectorManage {
        Create(CreateVectorIndexNode, "CreateVectorIndex", "create_vector_index", "Create Vector Index", "create_vector_index"),
        Drop(DropVectorIndexNode, "DropVectorIndex", "drop_vector_index", "Drop Vector Index", "drop_vector_index"),
    }
}
