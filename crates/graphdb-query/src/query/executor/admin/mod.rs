//! Managing actuator modules
//!
//! Provide database management functions, including space management, label management, edge type management, index management, data change, user management, query management and so on.
//! Simplified for single-node deployments, removing distribution-related functionality.

pub mod analyze;
pub mod edge;
pub mod index;
pub mod query_management;
pub mod space;
pub mod tag;
pub mod user;

pub use self::space::{
    AlterSpaceExecutor, ClearSpaceExecutor, CreateSpaceExecutor, DescSpaceExecutor,
    DropSpaceExecutor, ShowSpacesExecutor, SwitchSpaceExecutor,
};

pub use self::tag::{
    AlterTagExecutor, CreateTagExecutor, DescTagExecutor, DropTagExecutor, ShowCreateTagExecutor,
    ShowTagsExecutor,
};

pub use self::tag::alter_tag::{AlterTagInfo, AlterTagItem, AlterTagOp};

pub use self::edge::{
    AlterEdgeExecutor, CreateEdgeExecutor, DescEdgeExecutor, DropEdgeExecutor, ShowEdgesExecutor,
};

pub use self::edge::alter_edge::{AlterEdgeInfo, AlterEdgeItem, AlterEdgeOp};

pub use self::index::{
    CreateEdgeIndexExecutor, CreateTagIndexExecutor, DescEdgeIndexExecutor, DescTagIndexExecutor,
    DropEdgeIndexExecutor, DropTagIndexExecutor, RebuildEdgeIndexExecutor, RebuildTagIndexExecutor,
    ShowEdgeIndexesExecutor, ShowTagIndexStatusExecutor, ShowTagIndexesExecutor,
};

#[cfg(feature = "fulltext-search")]
pub use self::index::{
    AlterFulltextIndexExecutor, CreateFulltextIndexConfig, CreateFulltextIndexExecutor,
    DescribeFulltextIndexExecutor, DropFulltextIndexExecutor, ShowFulltextIndexExecutor,
};

pub use self::user::{
    AlterUserExecutor, ChangePasswordExecutor, CreateUserExecutor, DescribeUserExecutor,
    DropUserExecutor, GrantRoleExecutor, RevokeRoleExecutor,
};

pub use self::query_management::ShowStatsExecutor;

pub use self::analyze::{AnalyzeExecutor, AnalyzeTarget};

pub use crate::core::types::PasswordInfo;
