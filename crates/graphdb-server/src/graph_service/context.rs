use std::collections::HashMap;
use std::sync::Arc;

use crate::query::parser::ast::stmt::Ast;

/// Common execution context shared by all query/command execution paths.
///
/// Groups the per-statement parameters that flow through the execution
/// pipeline, eliminating repetitive argument lists and reducing the risk of
/// parameter-ordering mistakes.
pub struct QueryExecutionContext<'a> {
    /// The raw statement text.
    pub stmt: &'a str,
    /// Pre-parsed AST (available for command statements from the
    /// classification pass; `None` for regular statements).
    pub parsed_ast: Option<Arc<Ast>>,
    /// Target space id (0 when no space is selected).
    pub space_id: i64,
    /// Client-supplied query parameters (`@name` references).
    pub parameters: Option<HashMap<String, graphdb_core::Value>>,
    /// Client-supplied session variables (`$name` references).
    pub session_variables: Option<HashMap<String, graphdb_core::Value>>,
}
