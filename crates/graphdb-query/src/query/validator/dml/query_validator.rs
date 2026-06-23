//! Query Statement Validator
//! Used to validate the top-level query statement (QueryStmt)
//! The “Query” statement is a wrapper that contains the actual query statement.

use crate::query::parser::ast::stmt::{Ast, QueryStmt};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_enum::Validator;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult,
};
use crate::query::QueryContext;
use std::sync::Arc;

/// Query Statement Validator
#[derive(Debug)]
pub struct QueryValidator {
    inner_validator: Option<Box<crate::query::validator::validator_enum::Validator>>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl QueryValidator {
    /// Create a new Query validator.
    pub fn new() -> Self {
        Self {
            inner_validator: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &QueryStmt) -> Result<(), ValidationError> {
        if stmt.statements.is_empty() {
            return Err(ValidationError::new(
                "Query must contain at least one statement".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        let first_stmt = &stmt.statements[0];
        let validator = Validator::create_from_stmt(first_stmt).ok_or_else(|| {
            ValidationError::new(
                format!(
                    "Unsupported statement type in QUERY: {:?}",
                    first_stmt.kind()
                ),
                ValidationErrorType::SemanticError,
            )
        })?;

        self.inner_validator = Some(Box::new(validator));
        self.setup_outputs();

        Ok(())
    }

    fn setup_outputs(&mut self) {
        // The output of the query statement is the same as the output of the internal statements.
        // Copy from the internal validator after verification.
        if let Some(ref inner) = self.inner_validator {
            self.outputs = inner.get_outputs().to_vec();
        }
    }
}

impl Default for QueryValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as arguments.
impl StatementValidator for QueryValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let query_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Query(query_stmt) => query_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected QUERY statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // Verify the implementation (before executing the query_stmt on the mobile device).
        self.validate_impl(query_stmt)?;

        // Extract the first sentence.
        let first_stmt = query_stmt
            .statements
            .first()
            .ok_or_else(|| {
                ValidationError::new(
                    "Query must contain at least one statement".to_string(),
                    ValidationErrorType::SemanticError,
                )
            })?
            .clone();

        let inner = self
            .inner_validator
            .as_mut()
            .expect("inner_validator should be set after validate_impl");

        let result = inner.validate(
            Arc::new(Ast::new(first_stmt, ast.expr_context.clone())),
            qctx.clone(),
        );

        if result.success {
            self.inputs = result.inputs;
            self.outputs = result.outputs;

            let mut info = ValidationInfo::new();
            info.semantic_info.query_type = Some("Query".to_string());
            info.semantic_info.query_complexity = Some(self.inputs.len());

            Ok(ValidationResult::success_with_info(info))
        } else {
            Err(result.errors.into_iter().next().unwrap_or_else(|| {
                ValidationError::new(
                    "Internal validation failed".to_string(),
                    ValidationErrorType::SemanticError,
                )
            }))
        }
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Query
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        if let Some(ref inner) = self.inner_validator {
            inner.get_type().is_global_statement()
        } else {
            false
        }
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
    use crate::query::parser::ast::{QueryStmt, Span, Stmt};

    #[test]
    fn test_query_validator_new() {
        let validator = QueryValidator::new();
        assert_eq!(validator.statement_type(), StatementType::Query);
    }

    #[test]
    fn test_query_validator_with_match() {
        use crate::query::parser::ast::MatchStmt;

        let mut validator = QueryValidator::new();
        let query_stmt = QueryStmt {
            span: Span::default(),
            statements: vec![Stmt::Match(MatchStmt {
                span: Span::default(),
                patterns: vec![],
                where_clause: None,
                return_clause: None,
                order_by: None,
                limit: None,
                skip: None,
                optional: false,
                delete_clause: None,
            })],
        };

        // The verification implementation should successfully create the internal validator.
        assert!(validator.validate_impl(&query_stmt).is_ok());
        assert!(validator.inner_validator.is_some());
    }
}
