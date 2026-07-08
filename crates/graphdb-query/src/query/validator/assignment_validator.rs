//! Variable assignment statement validator
//! Corresponding to the functionality of NebulaGraph AssignmentValidator
//! Verify the validity of variable assignment statements, such as $var = GO FROM ...
//!
//! Design Principles:
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. When an assignment statement wraps other statements, it is necessary to recursively verify the internal statements.
//! 3. Variable name validation (must start with $)

use std::sync::Arc;

use crate::query::parser::ast::stmt::{AssignmentStmt, Ast};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_enum::Validator;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult,
};
use crate::query::QueryContext;

/// Verified assignment information
#[derive(Debug, Clone)]
pub struct ValidatedAssignment {
    pub variable: String,
    pub inner_statement_type: String,
}

/// Assignment Statement Validator
#[derive(Debug)]
pub struct AssignmentValidator {
    variable: String,
    inner_validator: Option<Box<Validator>>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl AssignmentValidator {
    pub fn new() -> Self {
        Self {
            variable: String::new(),
            inner_validator: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &AssignmentStmt) -> Result<(), ValidationError> {
        // Verify the variable names.
        self.variable = stmt.variable.clone();
        self.validate_variable_name(&self.variable)?;

        // Create an internal statement validator.
        self.inner_validator = Some(Box::new(
            Validator::create_from_stmt(&stmt.statement).ok_or_else(|| {
                ValidationError::new(
                    "Failed to create validator for inner statement".to_string(),
                    ValidationErrorType::SemanticError,
                )
            })?,
        ));

        Ok(())
    }

    fn validate_variable_name(&self, name: &str) -> Result<(), ValidationError> {
        // The variable name cannot be empty.
        if name.is_empty() {
            return Err(ValidationError::new(
                "Variable name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Variable names must start with a letter or an underscore (_).
        let first_char = name
            .chars()
            .next()
            .expect("The variable name is verified to be non-null");
        if !first_char.is_ascii_alphabetic() && first_char != '_' {
            return Err(ValidationError::new(
                format!(
                    "Variable name '{}' must start with a letter or underscore",
                    name
                ),
                ValidationErrorType::SemanticError,
            ));
        }

        // Variable names can only contain letters, digits, and underscores (_).
        for (i, c) in name.chars().enumerate() {
            if i > 0 && !c.is_ascii_alphanumeric() && c != '_' {
                return Err(ValidationError::new(
                    format!(
                        "Variable name '{}' contains invalid character '{}'",
                        name, c
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        Ok(())
    }

    /// Obtain the variable name
    pub fn variable(&self) -> &str {
        &self.variable
    }

    /// Obtain the internal validator.
    pub fn inner_validator(&self) -> Option<&Validator> {
        self.inner_validator.as_deref()
    }

    pub fn validated_result(&self) -> ValidatedAssignment {
        ValidatedAssignment {
            variable: self.variable.clone(),
            inner_statement_type: self
                .inner_validator
                .as_ref()
                .map(|v| v.get_type().as_str().to_string())
                .unwrap_or_default(),
        }
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
/// The internal statement validation directly calls the `validate` method, passing in `stmt` and `qctx` as parameters.
impl StatementValidator for AssignmentValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let assignment_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Assignment(assignment_stmt) => assignment_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected ASSIGNMENT statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // Extract the internal statements (before moving).
        let inner_stmt = *assignment_stmt.statement.clone();

        self.validate_impl(assignment_stmt)?;

        // Verify the internal statements.
        if let Some(ref mut inner) = self.inner_validator {
            let result = inner.validate(
                Arc::new(Ast::new(inner_stmt, ast.expr_context.clone())),
                qctx,
            );

            if result.success {
                // The output of the assignment statement is the same as the output of the statements inside it.
                self.inputs = result.inputs.clone();
                self.outputs = result.outputs.clone();

                // Add a variable to the list of user-defined variables.
                if !self.user_defined_vars.contains(&self.variable) {
                    self.user_defined_vars.push(self.variable.clone());
                }
            } else {
                return Err(result.errors.first().cloned().unwrap_or_else(|| {
                    ValidationError::new(
                        "Internal statement validation failed".to_string(),
                        ValidationErrorType::SemanticError,
                    )
                }));
            }
        }

        let mut info = ValidationInfo::new();

        info.add_alias(
            self.variable.clone(),
            crate::query::validator::structs::AliasType::Variable,
        );

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Assignment
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        self.inner_validator
            .as_ref()
            .map(|v| v.get_type().is_global_statement())
            .unwrap_or(false)
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for AssignmentValidator {
    fn default() -> Self {
        Self::new()
    }
}
