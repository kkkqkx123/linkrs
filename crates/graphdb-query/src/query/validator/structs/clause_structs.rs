//! Data structures related to clauses

use super::alias_structs::AliasType;
use super::path_structs::Path;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::OrderDirection;
use crate::core::DataType;
use crate::core::Expression;
use crate::core::YieldColumn;
use crate::query::validator::error::ValidationError;
use crate::query::validator::strategies::helpers::ExpressionValidationContext;
use crate::query::validator::QueryPart;
use std::collections::HashMap;

/// Context of the “Match” clause
#[derive(Debug, Clone)]
pub struct MatchClauseContext {
    pub paths: Vec<Path>,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
    pub where_clause: Option<WhereClauseContext>,
    pub is_optional: bool,
    pub skip: Option<ContextualExpression>,
    pub limit: Option<ContextualExpression>,
    pub query_parts: Vec<QueryPart>,
    pub errors: Vec<ValidationError>,
}

/// Context of the WHERE clause
#[derive(Debug, Clone)]
pub struct WhereClauseContext {
    pub filter: Option<ContextualExpression>,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
    pub paths: Vec<Path>,
    pub query_parts: Vec<QueryPart>,
    pub errors: Vec<ValidationError>,
}

/// Context of the RETURN clause
#[derive(Debug, Clone)]
pub struct ReturnClauseContext {
    pub yield_clause: YieldClauseContext,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
    pub pagination: Option<PaginationContext>,
    pub order_by: Option<OrderByClauseContext>,
    pub distinct: bool,
    pub query_parts: Vec<QueryPart>,
    pub errors: Vec<ValidationError>,
}

/// Context of the WITH clause
#[derive(Debug, Clone)]
pub struct WithClauseContext {
    pub yield_clause: YieldClauseContext,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
    pub where_clause: Option<WhereClauseContext>,
    pub pagination: Option<PaginationContext>,
    pub order_by: Option<OrderByClauseContext>,
    pub distinct: bool,
    pub query_parts: Vec<QueryPart>,
    pub errors: Vec<ValidationError>,
}

/// Context of the UNWIND clause
#[derive(Debug, Clone)]
pub struct UnwindClauseContext {
    pub alias: String,
    pub unwind_expression: Expression,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
    pub paths: Vec<Path>, // The paths that may be contained in the Unwind clause
    pub query_parts: Vec<QueryPart>,
    pub errors: Vec<ValidationError>,
}

/// Context of the Yield clause
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
    pub paths: Vec<Path>,
    pub query_parts: Vec<QueryPart>,
    pub errors: Vec<ValidationError>,
    pub filter_condition: Option<ContextualExpression>,
    pub skip: Option<usize>,
    pub limit: Option<usize>,
}

/// Pagination context
#[derive(Debug, Clone)]
pub struct PaginationContext {
    pub skip: i64,
    pub limit: i64,
}

/// Context of the sorting clause
#[derive(Debug, Clone)]
pub struct OrderByClauseContext {
    pub indexed_order_factors: Vec<(usize, OrderDirection)>,
}

// Implement the ExpressionValidationContext trait for various context types.
impl ExpressionValidationContext for MatchClauseContext {
    fn get_aliases(&self) -> &HashMap<String, AliasType> {
        &self.aliases_available
    }

    fn get_variable_types(&self) -> Option<&HashMap<String, DataType>> {
        None
    }
}

impl ExpressionValidationContext for WhereClauseContext {
    fn get_aliases(&self) -> &HashMap<String, AliasType> {
        &self.aliases_available
    }

    fn get_variable_types(&self) -> Option<&HashMap<String, DataType>> {
        None
    }
}

impl ExpressionValidationContext for ReturnClauseContext {
    fn get_aliases(&self) -> &HashMap<String, AliasType> {
        &self.aliases_available
    }

    fn get_variable_types(&self) -> Option<&HashMap<String, DataType>> {
        None
    }
}

impl ExpressionValidationContext for WithClauseContext {
    fn get_aliases(&self) -> &HashMap<String, AliasType> {
        &self.aliases_available
    }

    fn get_variable_types(&self) -> Option<&HashMap<String, DataType>> {
        None
    }
}

impl ExpressionValidationContext for UnwindClauseContext {
    fn get_aliases(&self) -> &HashMap<String, AliasType> {
        &self.aliases_available
    }

    fn get_variable_types(&self) -> Option<&HashMap<String, DataType>> {
        None
    }
}

impl ExpressionValidationContext for YieldClauseContext {
    fn get_aliases(&self) -> &HashMap<String, AliasType> {
        &self.aliases_available
    }

    fn get_variable_types(&self) -> Option<&HashMap<String, DataType>> {
        None
    }
}
