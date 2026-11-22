//! Immutable configuration for fulltext search operators.

use crate::parser::ast::fulltext::{
    AlterIndexAction, FulltextQueryExpr, IndexFieldDef, IndexOptions,
};
use linkrs_core::types::expr::Expression;
use linkrs_core::types::ReadYourWritesConfig;

/// Fulltext index DDL command payload.
#[derive(Debug, Clone)]
pub enum FulltextManageCommand {
    Create {
        index_name: String,
        schema_name: String,
        fields: Vec<IndexFieldDef>,
        options: IndexOptions,
        if_not_exists: bool,
        space_id: u64,
    },
    Drop {
        index_name: String,
        if_exists: bool,
        /// Space the statement runs in; index names are only unique inside
        /// one space, so an unscoped drop could destroy a foreign catalog.
        space_id: u64,
    },
    Alter {
        index_name: String,
        actions: Vec<AlterIndexAction>,
        space_id: u64,
    },
    Show {
        pattern: Option<String>,
        from_schema: Option<String>,
        space_id: u64,
    },
    Describe {
        index_name: String,
        space_id: u64,
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
        structured_query: FulltextQueryExpr,
        tag_name: String,
        field_name: String,
        limit: Option<usize>,
        /// Read-your-writes consistency config; `None` = eventual.
        ryw_config: Option<ReadYourWritesConfig>,
    },
    FulltextLookup {
        space_name: String,
        space_id: u64,
        index_name: String,
        structured_query: FulltextQueryExpr,
        tag_name: String,
        field_name: String,
        limit: Option<usize>,
        /// Read-your-writes consistency config; `None` = eventual.
        ryw_config: Option<ReadYourWritesConfig>,
    },
    MatchFulltext {
        space_name: String,
        space_id: u64,
        match_expr: Expression,
        match_field: Option<String>,
        /// Structured query built from the fulltext match condition.
        structured_query: FulltextQueryExpr,
        tag_name: String,
        field_name: String,
        limit: Option<usize>,
        /// Read-your-writes consistency config; `None` = eventual.
        ryw_config: Option<ReadYourWritesConfig>,
    },
}

impl FulltextSpec {
    /// Return the RYW consistency config, if any.
    pub fn ryw_config(&self) -> Option<ReadYourWritesConfig> {
        match self {
            Self::FulltextManage { .. } => None,
            Self::FulltextSearch { ryw_config, .. }
            | Self::FulltextLookup { ryw_config, .. }
            | Self::MatchFulltext { ryw_config, .. } => *ryw_config,
        }
    }
}
