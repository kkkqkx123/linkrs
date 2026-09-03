//! Immutable configuration for fulltext search operators.

use graphdb_core::types::expr::Expression;

/// Fulltext index DDL command payload.
#[derive(Debug, Clone)]
pub enum FulltextManageCommand {
    Create {
        index_name: String,
        schema_name: String,
        fields: Vec<String>,
        space_id: u64,
    },
    Drop {
        index_name: String,
        if_exists: bool,
    },
    Alter {
        index_name: String,
    },
    Show {
        pattern: Option<String>,
        from_schema: Option<String>,
    },
    Describe {
        index_name: String,
    },
}

impl FulltextManageCommand {
    /// Whether the command mutates stored state.
    pub fn is_write(&self) -> bool {
        match self {
            Self::Create { .. } | Self::Drop { .. } | Self::Alter { .. } => true,
            Self::Show { .. } | Self::Describe { .. } => false,
        }
    }
}

/// Immutable config for fulltext search operators.
#[derive(Debug, Clone)]
pub enum FulltextSpec {
    FulltextManage {
        space_name: String,
        command: FulltextManageCommand,
    },
    FulltextSearch {
        space_name: String,
        space_id: u64,
        index_name: String,
        search_query: String,
        tag_name: String,
        field_name: String,
    },
    FulltextLookup {
        space_name: String,
        space_id: u64,
        index_name: String,
        search_query: String,
        tag_name: String,
        field_name: String,
    },
    MatchFulltext {
        space_name: String,
        match_expr: Expression,
        match_field: Option<String>,
        tag_name: String,
        field_name: String,
    },
}
