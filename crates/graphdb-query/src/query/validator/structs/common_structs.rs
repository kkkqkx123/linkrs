//! General data structures

use crate::core::DataType;
use crate::query::validator::error::ValidationError;
use crate::query::validator::strategies::helpers::ExpressionValidationContext;
use crate::query::validator::structs::{AliasType, QueryPart};
use crate::query::validator::validator_trait::ColumnDef;
use std::collections::HashMap;

/// Verify the implementation of the context.
#[derive(Debug, Clone)]
pub struct ValidationContextImpl {
    pub query_parts: Vec<QueryPart>,
    pub errors: Vec<ValidationError>,
    pub aliases: std::collections::HashMap<String, AliasType>,
    /// Variable definition: Variable name -> Column definition
    pub variables: std::collections::HashMap<String, Vec<ColumnDef>>,
}

impl Default for ValidationContextImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationContextImpl {
    pub fn new() -> Self {
        Self {
            query_parts: Vec::new(),
            errors: Vec::new(),
            aliases: std::collections::HashMap::new(),
            variables: std::collections::HashMap::new(),
        }
    }

    pub fn add_error(&mut self, error: ValidationError) {
        self.errors.push(error);
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn get_errors(&self) -> &[ValidationError] {
        &self.errors
    }

    /// Check whether the variable exists.
    pub fn exists_var(&self, name: &str) -> bool {
        self.variables.contains_key(name)
    }

    /// Obtain the column definitions of the variables
    pub fn get_var(&self, name: &str) -> Vec<ColumnDef> {
        self.variables.get(name).cloned().unwrap_or_default()
    }

    /// Registering variables
    pub fn register_variable(&mut self, name: String, cols: Vec<ColumnDef>) {
        self.variables.insert(name, cols);
    }
}

/// Cypher clause types
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
    /// Obtain a string representation of the clause type.
    pub fn as_str(&self) -> &'static str {
        match self {
            CypherClauseKind::Match => "MATCH",
            CypherClauseKind::Where => "WHERE",
            CypherClauseKind::Return => "RETURN",
            CypherClauseKind::With => "WITH",
            CypherClauseKind::Unwind => "UNWIND",
            CypherClauseKind::Yield => "YIELD",
            CypherClauseKind::OrderBy => "ORDER BY",
            CypherClauseKind::Pagination => "PAGINATION",
        }
    }
}

// Implement the ExpressionValidationContext trait for ValidationContextImpl
impl ExpressionValidationContext for ValidationContextImpl {
    fn get_aliases(&self) -> &HashMap<String, AliasType> {
        &self.aliases
    }

    fn get_variable_types(&self) -> Option<&HashMap<String, DataType>> {
        None
    }
}
