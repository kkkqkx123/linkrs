//! Immutable configuration for DDL operators and schema/user manage payloads.

use graphdb_core::metadata::MacroParamDef;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::user::PasswordInfo;
use graphdb_core::types::PropertyDef;

/// Migrate action kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateAction {
    MigrateSpace,
}

/// Space DDL command payload.
#[derive(Debug, Clone)]
pub enum SpaceManageCommand {
    Create {
        space_name: String,
        vid_type: String,
    },
    Drop {
        space_name: String,
    },
    Desc {
        space_name: String,
    },
    Show,
    ShowCreate {
        space_name: String,
    },
    Switch {
        space_name: String,
    },
    Alter {
        space_name: String,
    },
    Clear {
        space_name: String,
    },
    CommentOn {
        space_name: String,
        comment: String,
    },
    Checkpoint,
    ExportDatabase {
        path: String,
    },
    ImportDatabase {
        path: String,
    },
    AttachDatabase {
        alias: String,
        path: String,
        db_type: Option<String>,
    },
    DetachDatabase {
        alias: String,
    },
}

/// Tag DDL command payload.
#[derive(Debug, Clone)]
pub enum TagManageCommand {
    Create {
        tag_name: String,
        properties: Vec<PropertyDef>,
        if_not_exists: bool,
    },
    Alter {
        tag_name: String,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
        changes: Vec<PropertyRename>,
    },
    Rename {
        old_name: String,
        new_name: String,
    },
    Desc {
        tag_name: String,
    },
    Drop {
        tag_name: String,
        if_exists: bool,
    },
    Show,
    ShowCreate {
        tag_name: String,
    },
}

/// Edge DDL command payload.
#[derive(Debug, Clone)]
pub enum EdgeManageCommand {
    Create {
        edge_name: String,
        properties: Vec<PropertyDef>,
        src_tag_name: Option<String>,
        dst_tag_name: Option<String>,
        if_not_exists: bool,
    },
    Alter {
        edge_name: String,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    },
    Rename {
        old_name: String,
        new_name: String,
    },
    Desc {
        edge_name: String,
    },
    Drop {
        edge_name: String,
        if_exists: bool,
    },
    Show,
    ShowCreate {
        edge_name: String,
    },
}

/// Sequence DDL command payload.
#[derive(Debug, Clone)]
pub enum SequenceManageCommand {
    Create {
        seq_name: String,
        start: Option<i64>,
        increment: Option<i64>,
        min_value: Option<i64>,
        max_value: Option<i64>,
        cycle: bool,
        if_not_exists: bool,
    },
    Alter {
        seq_name: String,
        increment: Option<i64>,
        min_value: Option<i64>,
        max_value: Option<i64>,
        cycle: Option<bool>,
    },
    Drop {
        seq_name: String,
        if_exists: bool,
    },
}

/// Macro DDL command payload.
///
/// The body is the parsed macro template; the binder expands macro calls
/// before planning, so the executor only stores and lists definitions.
#[derive(Debug, Clone)]
pub enum MacroManageCommand {
    Create {
        macro_name: String,
        params: Vec<MacroParamDef>,
        body: Expression,
        if_not_exists: bool,
    },
    Drop {
        macro_name: String,
        if_exists: bool,
    },
}

/// Type DDL command payload.
#[derive(Debug, Clone)]
pub enum TypeManageCommand {
    Create {
        type_name: String,
        underlying_type: String,
        if_not_exists: bool,
    },
    Drop {
        type_name: String,
        if_exists: bool,
    },
}

/// Index DDL command payload.
#[derive(Debug, Clone)]
pub enum IndexManageCommand {
    CreateTagIndex {
        index_name: String,
        target_name: String,
        properties: Vec<String>,
    },
    DropTagIndex {
        index_name: String,
    },
    DescTagIndex {
        index_name: String,
    },
    ShowTagIndexes,
    RebuildTagIndex {
        index_name: String,
    },
    CreateEdgeIndex {
        index_name: String,
        target_name: String,
        properties: Vec<String>,
    },
    DropEdgeIndex {
        index_name: String,
    },
    DescEdgeIndex {
        index_name: String,
    },
    ShowEdgeIndexes,
    RebuildEdgeIndex {
        index_name: String,
    },
    ShowIndexes,
    ShowCreateIndex {
        index_name: String,
    },
}

/// User DDL command payload.
#[derive(Debug, Clone)]
pub enum UserManageCommand {
    Create {
        username: String,
        password: String,
        role: String,
    },
    Alter {
        username: String,
        new_password: Option<String>,
        new_role: Option<String>,
        is_locked: Option<bool>,
    },
    Drop {
        username: String,
        if_exists: bool,
    },
    ChangePassword {
        password_info: PasswordInfo,
    },
    GrantRole {
        username: String,
        space_name: String,
        role: String,
    },
    RevokeRole {
        username: String,
        space_name: String,
    },
    ShowUsers,
    ShowRoles,
    DescribeUser {
        username: String,
    },
}

/// Property rename within ALTER TAG (executor consumes only old/new names).
#[derive(Debug, Clone)]
pub struct PropertyRename {
    pub old_name: String,
    pub new_name: String,
}

/// Immutable config for DDL operators.
#[derive(Debug, Clone)]
pub enum DdlSpec {
    SpaceManage {
        command: SpaceManageCommand,
    },
    TagManage {
        space_name: String,
        command: TagManageCommand,
    },
    EdgeManage {
        space_name: String,
        command: EdgeManageCommand,
    },
    IndexManage {
        space_name: String,
        command: IndexManageCommand,
    },
    DeleteIndex {
        space_name: String,
        index_name: String,
    },
    UserManage {
        command: UserManageCommand,
    },
    ShowStats {
        space_name: String,
    },
    ShowConfigs {
        space_name: String,
    },
    ShowQueries {
        space_name: String,
    },
    ShowSessions {
        space_name: String,
    },
    ShowFunctions {
        space_name: String,
    },
    ShowGraphs {
        space_name: String,
    },
    ShowMacros {
        space_name: String,
    },
    ShowAttachedDatabases {
        space_name: String,
    },
    ShowExtensions {
        space_name: String,
    },
    LoadFrom {
        space_name: String,
        source_kind: String,
        source_value: String,
        func_name: Option<String>,
        func_args_json: Option<String>,
        options: Vec<(String, String)>,
        col_names: Vec<String>,
    },
    InQueryCall {
        space_name: String,
        func_name: String,
        args_json: String,
        yield_items: Vec<(String, String)>,
        col_names: Vec<String>,
    },
    Analyze {
        space_name: String,
    },
    Migrate {
        space_name: String,
        action: MigrateAction,
        migration_data: Option<String>,
    },
    MigratePlan {
        space_name: String,
        label: String,
        is_edge: bool,
        from_version: u64,
        to_version: u64,
    },
    MigrateRun {
        plan_json: String,
    },
    MigrateRollback {
        plan_json: String,
    },
    SequenceManage {
        command: SequenceManageCommand,
    },
    MacroManage {
        command: MacroManageCommand,
    },
    TypeManage {
        command: TypeManageCommand,
    },
}

impl SpaceManageCommand {
    /// Whether the command mutates stored state.
    ///
    /// Describe/show variants only read schema metadata; switch only flips
    /// session-level space routing.
    pub fn is_write(&self) -> bool {
        match self {
            Self::Create { .. }
            | Self::Drop { .. }
            | Self::Alter { .. }
            | Self::Clear { .. }
            | Self::CommentOn { .. }
            | Self::Checkpoint
            | Self::ExportDatabase { .. }
            | Self::ImportDatabase { .. } => true,
            // Attach/Detach only mutate the in-memory attached-database
            // catalog, not storage, so they must not demand a write
            // transaction scope.
            Self::AttachDatabase { .. } | Self::DetachDatabase { .. } => false,
            Self::Desc { .. } | Self::Show | Self::ShowCreate { .. } | Self::Switch { .. } => false,
        }
    }
}

impl TagManageCommand {
    /// Whether the command mutates stored state.
    pub fn is_write(&self) -> bool {
        match self {
            Self::Create { .. } | Self::Alter { .. } | Self::Rename { .. } | Self::Drop { .. } => {
                true
            }
            Self::Desc { .. } | Self::Show | Self::ShowCreate { .. } => false,
        }
    }
}

impl EdgeManageCommand {
    /// Whether the command mutates stored state.
    pub fn is_write(&self) -> bool {
        match self {
            Self::Create { .. } | Self::Alter { .. } | Self::Rename { .. } | Self::Drop { .. } => {
                true
            }
            Self::Desc { .. } | Self::Show | Self::ShowCreate { .. } => false,
        }
    }
}

impl SequenceManageCommand {
    /// Whether the command mutates stored state.
    pub fn is_write(&self) -> bool {
        match self {
            Self::Create { .. } | Self::Alter { .. } | Self::Drop { .. } => true,
        }
    }
}

impl MacroManageCommand {
    /// Whether the command mutates stored state.
    pub fn is_write(&self) -> bool {
        match self {
            Self::Create { .. } | Self::Drop { .. } => true,
        }
    }
}

impl TypeManageCommand {
    /// Whether the command mutates stored state.
    pub fn is_write(&self) -> bool {
        match self {
            Self::Create { .. } | Self::Drop { .. } => true,
        }
    }
}

impl IndexManageCommand {
    /// Whether the command mutates stored state.
    ///
    /// Describe/show variants only read index metadata.
    pub fn is_write(&self) -> bool {
        match self {
            Self::CreateTagIndex { .. }
            | Self::DropTagIndex { .. }
            | Self::RebuildTagIndex { .. }
            | Self::CreateEdgeIndex { .. }
            | Self::DropEdgeIndex { .. }
            | Self::RebuildEdgeIndex { .. } => true,
            Self::DescTagIndex { .. }
            | Self::ShowTagIndexes
            | Self::DescEdgeIndex { .. }
            | Self::ShowEdgeIndexes
            | Self::ShowIndexes
            | Self::ShowCreateIndex { .. } => false,
        }
    }
}

impl UserManageCommand {
    /// Whether the command mutates the user/privilege store.
    ///
    /// Show/describe variants only read through the auth reader.
    pub fn is_write(&self) -> bool {
        match self {
            Self::Create { .. }
            | Self::Alter { .. }
            | Self::Drop { .. }
            | Self::ChangePassword { .. }
            | Self::GrantRole { .. }
            | Self::RevokeRole { .. } => true,
            Self::ShowUsers | Self::ShowRoles | Self::DescribeUser { .. } => false,
        }
    }
}

impl DdlSpec {
    /// Whether the spec mutates stored state.
    ///
    /// Read-only variants (describe/show) run on the statement-level read
    /// snapshot and must not require a write transaction scope.
    pub fn is_write(&self) -> bool {
        match self {
            Self::SpaceManage { command } => command.is_write(),
            Self::TagManage { command, .. } => command.is_write(),
            Self::EdgeManage { command, .. } => command.is_write(),
            Self::IndexManage { command, .. } => command.is_write(),
            Self::DeleteIndex { .. } => true,
            Self::UserManage { command } => command.is_write(),
            Self::ShowStats { .. }
            | Self::ShowConfigs { .. }
            | Self::ShowQueries { .. }
            | Self::ShowSessions { .. }
            | Self::ShowFunctions { .. }
            | Self::ShowGraphs { .. }
            | Self::ShowMacros { .. }
            | Self::ShowAttachedDatabases { .. }
            | Self::ShowExtensions { .. }
            | Self::LoadFrom { .. }
            | Self::InQueryCall { .. }
            | Self::Analyze { .. } => false,
            Self::Migrate { .. }
            | Self::MigratePlan { .. }
            | Self::MigrateRun { .. }
            | Self::MigrateRollback { .. } => true,
            Self::SequenceManage { command } => command.is_write(),
            Self::MacroManage { command } => command.is_write(),
            Self::TypeManage { command } => command.is_write(),
        }
    }
}
