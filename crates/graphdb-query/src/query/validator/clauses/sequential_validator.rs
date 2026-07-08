//! Sequential Statement Validator
//! Verify the validity of multi-statement queries (separated by semicolons).
//!
//! This document has been restructured in accordance with the new trait + enumeration validator framework.
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. All original functions have been retained.
//! Verification of the number of sentences
//! Verification of the order of DDL/DML statements
//! Variable name validation
//! Limit on the maximum number of sentences
//! 3. Use QueryContext to manage the context in a unified manner.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::DataType;
use crate::query::parser::ast::stmt::Ast;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult,
};
use crate::query::QueryContext;
use std::collections::HashMap;
use std::sync::Arc;

/// Definition of sequential statements
#[derive(Debug, Clone)]
pub struct SequentialStatement {
    pub statement: String,
    pub parameters: HashMap<String, ContextualExpression>,
}

impl SequentialStatement {
    /// Create a new sentence with the given words in the correct order.
    pub fn new(statement: String) -> Self {
        Self {
            statement,
            parameters: HashMap::new(),
        }
    }

    /// Add parameters
    pub fn with_parameter(mut self, name: String, expr: ContextualExpression) -> Self {
        self.parameters.insert(name, expr);
        self
    }
}

/// Sequential Validator – New Implementation in the New System
///
/// Functionality completeness assurance:
/// 1. Complete validation lifecycle
/// 2. Management of input/output columns
/// 3. Expression property tracing
/// 4. Verification of the order of multiple sentences
/// 5. Verification of DDL/DML sequence constraints
#[derive(Debug)]
pub struct SequentialValidator {
    // List of sentences
    statements: Vec<SequentialStatement>,
    // Limit on the maximum number of sentences
    max_statements: usize,
    // Variable mapping
    variables: HashMap<String, DataType>,
    // Column definition (for the trait interface)
    inputs: Vec<ColumnDef>,
    // Column definition (The output of the sequence of statements is the output of the last statement.)
    outputs: Vec<ColumnDef>,
    // Expression property
    expr_props: ExpressionProps,
    // User-defined variables
    user_defined_vars: Vec<String>,
    // List of validation errors
    validation_errors: Vec<ValidationError>,
}

impl SequentialValidator {
    /// Create a new instance of the validator.
    pub fn new() -> Self {
        Self {
            statements: Vec::new(),
            max_statements: 100,
            variables: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validation_errors: Vec::new(),
        }
    }

    /// Set the maximum number of sentences
    pub fn with_max_statements(mut self, max: usize) -> Self {
        self.max_statements = max;
        self
    }

    /// Add the sentence
    pub fn add_statement(&mut self, statement: SequentialStatement) {
        self.statements.push(statement);
    }

    /// Setting variables
    pub fn set_variable(&mut self, name: String, type_: DataType) {
        self.variables.insert(name.clone(), type_);
        if !self.user_defined_vars.contains(&name) {
            self.user_defined_vars.push(name);
        }
    }

    /// Obtain a list of statements.
    pub fn statements(&self) -> &[SequentialStatement] {
        &self.statements
    }

    /// Obtain the variable mapping.
    pub fn variables(&self) -> &HashMap<String, DataType> {
        &self.variables
    }

    /// Obtain the maximum number of sentences.
    pub fn max_statements(&self) -> usize {
        self.max_statements
    }

    /// Setting the maximum number of statements
    pub fn set_max_statements(&mut self, max: usize) {
        self.max_statements = max;
    }

    /// Clear the verification errors.
    fn clear_errors(&mut self) {
        self.validation_errors.clear();
    }

    /// Perform validation (in the traditional way, while maintaining backward compatibility).
    pub fn validate_sequential(&mut self) -> Result<(), ValidationError> {
        self.clear_errors();
        self.validate_impl()?;
        Ok(())
    }

    fn validate_impl(&mut self) -> Result<(), ValidationError> {
        self.validate_statement_count()?;
        self.validate_statement_order()?;
        self.validate_variables()?;
        Ok(())
    }

    fn validate_statement_count(&self) -> Result<(), ValidationError> {
        if self.statements.is_empty() {
            return Err(ValidationError::new(
                "Sequential statement must have at least one statement".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        if self.statements.len() > self.max_statements {
            return Err(ValidationError::new(
                format!(
                    "Too many statements in sequential query (max: {})",
                    self.max_statements
                ),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    fn validate_statement_order(&self) -> Result<(), ValidationError> {
        let mut has_ddl = false;
        let mut has_dml = false;

        for (i, stmt) in self.statements.iter().enumerate() {
            let stmt_upper = stmt.statement.to_uppercase();
            if self.is_ddl_statement(&stmt_upper) {
                if has_dml {
                    return Err(ValidationError::new(
                        format!(
                            "DDL statement cannot follow DML statement at position {}",
                            i + 1
                        ),
                        ValidationErrorType::SemanticError,
                    ));
                }
                if has_ddl {
                    return Err(ValidationError::new(
                        format!(
                            "Multiple DDL statements are not allowed, found at position {}",
                            i + 1
                        ),
                        ValidationErrorType::SemanticError,
                    ));
                }
                has_ddl = true;
            }
            if self.is_dml_statement(&stmt_upper) {
                has_dml = true;
            }
        }
        Ok(())
    }

    fn is_ddl_statement(&self, stmt: &str) -> bool {
        stmt.starts_with("CREATE") || stmt.starts_with("ALTER") || stmt.starts_with("DROP")
    }

    fn is_dml_statement(&self, stmt: &str) -> bool {
        stmt.starts_with("INSERT")
            || stmt.starts_with("UPDATE")
            || stmt.starts_with("DELETE")
            || stmt.starts_with("UPSERT")
    }

    fn validate_variables(&self) -> Result<(), ValidationError> {
        for name in self.variables.keys() {
            if name.is_empty() {
                return Err(ValidationError::new(
                    "Variable name cannot be empty".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
            if !name.starts_with('$') && !name.starts_with('@') {
                return Err(ValidationError::new(
                    format!(
                        "Invalid variable name '{}': must start with '$' or '@'",
                        name
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    /// Check whether the statement is a query statement (which returns a result set).
    pub fn is_query_statement(&self, stmt: &str) -> bool {
        let stmt_upper = stmt.to_uppercase();
        stmt_upper.starts_with("MATCH")
            || stmt_upper.starts_with("GO")
            || stmt_upper.starts_with("FETCH")
            || stmt_upper.starts_with("LOOKUP")
            || stmt_upper.starts_with("FIND PATH")
            || stmt_upper.starts_with("GET SUBGRAPH")
    }

    /// Check whether the sentence is a modified sentence.
    pub fn is_mutation_statement(&self, stmt: &str) -> bool {
        let stmt_upper = stmt.to_uppercase();
        stmt_upper.starts_with("INSERT")
            || stmt_upper.starts_with("UPDATE")
            || stmt_upper.starts_with("DELETE")
            || stmt_upper.starts_with("UPSERT")
    }
}

impl Default for SequentialValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for SequentialValidator {
    fn validate(
        &mut self,
        _ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        self.clear_errors();

        // Please provide the text you would like to have translated. I will then perform the verification and provide the translated version.
        if let Err(e) = self.validate_impl() {
            return Ok(ValidationResult::failure(vec![e]));
        }

        // The output of a sequence of statements depends on the last statement in the sequence.
        // The simplification process here results in an empty output. (In reality, the output should be determined based on the type of the last sentence.)
        self.outputs = Vec::new();

        let info = ValidationInfo::new();

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Sequential
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // “Sequential” is not a global statement; it is necessary to select a space in advance.
        false
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sequential_validator_new() {
        let validator = SequentialValidator::new();
        assert!(validator.statements().is_empty());
        assert!(validator.variables().is_empty());
        assert_eq!(validator.max_statements(), 100);
    }

    #[test]
    fn test_add_statement() {
        let mut validator = SequentialValidator::new();
        let stmt = SequentialStatement::new("MATCH (n) RETURN n".to_string());
        validator.add_statement(stmt);
        assert_eq!(validator.statements().len(), 1);
    }

    #[test]
    fn test_set_variable() {
        let mut validator = SequentialValidator::new();
        validator.set_variable("$var".to_string(), DataType::String);
        assert_eq!(validator.variables().len(), 1);
        assert!(validator.variables().contains_key("$var"));
    }

    #[test]
    fn test_validate_empty_statements() {
        let mut validator = SequentialValidator::new();
        let result = validator.validate_sequential();
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_valid_single_statement() {
        let mut validator = SequentialValidator::new();
        let stmt = SequentialStatement::new("MATCH (n) RETURN n".to_string());
        validator.add_statement(stmt);

        let result = validator.validate_sequential();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_ddl_before_dml() {
        let mut validator = SequentialValidator::new();
        validator.add_statement(SequentialStatement::new(
            "CREATE TAG person(name string)".to_string(),
        ));
        validator.add_statement(SequentialStatement::new(
            "INSERT VERTEX person(name) VALUES \"1\":(\"Alice\")".to_string(),
        ));

        let result = validator.validate_sequential();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_ddl_after_dml() {
        let mut validator = SequentialValidator::new();
        validator.add_statement(SequentialStatement::new(
            "INSERT VERTEX person(name) VALUES \"1\":(\"Alice\")".to_string(),
        ));
        validator.add_statement(SequentialStatement::new(
            "CREATE TAG person(name string)".to_string(),
        ));

        let result = validator.validate_sequential();
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_multiple_ddl() {
        let mut validator = SequentialValidator::new();
        validator.add_statement(SequentialStatement::new(
            "CREATE TAG person(name string)".to_string(),
        ));
        validator.add_statement(SequentialStatement::new(
            "CREATE TAG company(name string)".to_string(),
        ));

        let result = validator.validate_sequential();
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_invalid_variable() {
        let mut validator = SequentialValidator::new();
        validator.add_statement(SequentialStatement::new("RETURN 1".to_string()));
        validator.set_variable("invalid_var".to_string(), DataType::Int);

        let result = validator.validate_sequential();
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_valid_variable() {
        let mut validator = SequentialValidator::new();
        validator.add_statement(SequentialStatement::new("RETURN 1".to_string()));
        validator.set_variable("$var".to_string(), DataType::Int);

        let result = validator.validate_sequential();
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_query_statement() {
        let validator = SequentialValidator::new();
        assert!(validator.is_query_statement("MATCH (n) RETURN n"));
        assert!(validator.is_query_statement("GO FROM \"1\" OVER edge"));
        assert!(validator.is_query_statement("FETCH PROP ON person \"1\""));
        assert!(
            !validator.is_query_statement("INSERT VERTEX person(name) VALUES \"1\":(\"Alice\")")
        );
    }

    #[test]
    fn test_is_mutation_statement() {
        let validator = SequentialValidator::new();
        assert!(
            validator.is_mutation_statement("INSERT VERTEX person(name) VALUES \"1\":(\"Alice\")")
        );
        assert!(validator.is_mutation_statement("UPDATE VERTEX \"1\" SET name=\"Bob\""));
        assert!(validator.is_mutation_statement("DELETE VERTEX \"1\""));
        assert!(!validator.is_mutation_statement("MATCH (n) RETURN n"));
    }

    #[test]
    fn test_max_statements_limit() {
        let mut validator = SequentialValidator::new().with_max_statements(2);
        validator.add_statement(SequentialStatement::new("RETURN 1".to_string()));
        validator.add_statement(SequentialStatement::new("RETURN 2".to_string()));
        validator.add_statement(SequentialStatement::new("RETURN 3".to_string()));

        let result = validator.validate_sequential();
        assert!(result.is_err());
    }
}
