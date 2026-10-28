//! Clause contexts produced by the binder and consumed by planners.
//!
//! The bound statement is the single source of truth; these contexts carry
//! resolved clause inputs for planning.

use std::collections::HashMap;

use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::types::semantic::AliasType;

use linkrs_core::types::OrderDirection;
use linkrs_core::YieldColumn;

/// Cypher clause types.
#[derive(Debug, Clone, PartialEq, Eq, Copy, Hash)]
pub enum CypherClauseKind {
    Match,
    Where,
    Return,
    With,
    Unwind,
    Yield,
    OrderBy,
    Pagination,
}

impl CypherClauseKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Match => "MATCH",
            Self::Where => "WHERE",
            Self::Return => "RETURN",
            Self::With => "WITH",
            Self::Unwind => "UNWIND",
            Self::Yield => "YIELD",
            Self::OrderBy => "ORDER BY",
            Self::Pagination => "PAGINATION",
        }
    }
}

// Clause contexts consumed by planners.

#[derive(Debug, Clone)]
pub struct WhereClauseContext {
    pub filter: Option<ContextualExpression>,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
}

#[derive(Debug, Clone)]
pub struct OrderByClauseContext {
    pub indexed_order_factors: Vec<(usize, OrderDirection)>,
}

#[derive(Debug, Clone)]
pub struct PaginationContext {
    pub skip: i64,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub struct YieldClauseContext {
    pub yield_columns: Vec<YieldColumn>,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
    pub distinct: bool,
    pub has_agg: bool,
    pub group_keys: Vec<ContextualExpression>,
    pub group_items: Vec<ContextualExpression>,
    pub need_gen_project: bool,
    pub agg_output_column_names: Vec<String>,
    pub proj_output_column_names: Vec<String>,
    pub filter_condition: Option<ContextualExpression>,
    pub skip: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct WithClauseContext {
    pub yield_clause: YieldClauseContext,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
    pub where_clause: Option<WhereClauseContext>,
    pub pagination: Option<PaginationContext>,
    pub order_by: Option<OrderByClauseContext>,
    pub distinct: bool,
}
