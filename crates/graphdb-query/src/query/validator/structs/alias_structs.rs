//! Alias-related data structures

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression;
use crate::core::types::OrderDirection;
use crate::core::YieldColumn;
use crate::query::validator::{MatchClauseContext, Path};
use std::collections::HashMap;

/// Alias types in Cypher queries
#[derive(Debug, Clone, PartialEq)]
pub enum AliasType {
    Node,
    Edge,
    NodeList,
    EdgeList,
    Path,
    Variable,
    Runtime,
    CTE,
    Expression,
}

/// Boundary clause context (With or Unwind)
#[derive(Debug, Clone)]
pub enum BoundaryClauseContext {
    With(WithClauseData),
    Unwind(UnwindClauseData),
}

/// Data from the WITH clause
#[derive(Debug, Clone)]
pub struct WithClauseData {
    pub yield_clause: YieldClauseData,
    pub where_clause: Option<WhereClauseData>,
    pub pagination: Option<PaginationData>,
    pub order_by: Option<OrderByData>,
    pub distinct: bool,
}

/// UNWIND clause data
#[derive(Debug, Clone)]
pub struct UnwindClauseData {
    pub alias: String,
    pub unwind_expression: Expression,
    pub paths: Vec<Path>,
}

/// Data from the Yield clause
#[derive(Debug, Clone)]
pub struct YieldClauseData {
    pub yield_columns: Vec<YieldColumn>,
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

/// Data from the WHERE clause
#[derive(Debug, Clone)]
pub struct WhereClauseData {
    pub filter: Option<ContextualExpression>,
}

/// Paginated data
#[derive(Debug, Clone)]
pub struct PaginationData {
    pub skip: Option<usize>,
    pub limit: Option<usize>,
}

/// Sorting data
#[derive(Debug, Clone)]
pub struct OrderByData {
    pub items: Vec<OrderByItem>,
}

/// Sorting items
#[derive(Debug, Clone)]
pub struct OrderByItem {
    pub expression: ContextualExpression,
    pub direction: OrderDirection,
}

/// Query the structure of the part.
#[derive(Debug, Clone)]
pub struct QueryPart {
    pub matchs: Vec<MatchClauseContext>,
    pub boundary: Option<BoundaryClauseContext>,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
    pub paths: Vec<Path>,
}
