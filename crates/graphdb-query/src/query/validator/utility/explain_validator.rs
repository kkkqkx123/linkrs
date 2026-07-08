//! Statement Validator Explanation/Profile:
//! Corresponding to the functionality of NebulaGraph ExplainValidator
//! Verify the EXPLAIN and PROFILE statements
//!
//! Design principles:
//! The StatementValidator trait has been implemented, unifying the interface.
//! 2. EXPLAIN/PROFILE: These functions are used to analyze the structure of SQL queries. When applied to a query, they “package” other related statements and perform recursive verification of the internal components of the query. In other words, they break down the query into its individual components (such as SELECT, INSERT, UPDATE, etc.) and then check the syntax, semantics, and performance characteristics of each component. This process helps to identify potential issues or optimizations that can improve the overall performance of the query.
//! 3. Support for multiple output formats (row, dot).

use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::query::parser::ast::stmt::{Ast, ExplainFormat, ExplainStmt, ProfileStmt};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_enum::Validator;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Verified EXPLAIN information
#[derive(Debug, Clone)]
pub struct ValidatedExplain {
    pub format: ExplainFormat,
    pub inner_statement_type: String,
}

/// EXPLAIN Statement Validator
#[derive(Debug)]
pub struct ExplainValidator {
    format: ExplainFormat,
    inner_validator: Option<Box<Validator>>,
    schema_manager: Option<Arc<SchemaManager>>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl ExplainValidator {
    pub fn new() -> Self {
        Self {
            format: ExplainFormat::Table,
            inner_validator: None,
            schema_manager: None,
            inputs: Vec::new(),
            outputs: vec![
                ColumnDef {
                    name: "id".to_string(),
                    type_: ValueType::Int,
                },
                ColumnDef {
                    name: "name".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "dependencies".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "profiling_data".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "operator info".to_string(),
                    type_: ValueType::String,
                },
            ],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_manager = Some(schema_manager);
    }

    fn validate_impl(&mut self, stmt: &ExplainStmt) -> Result<(), ValidationError> {
        self.format = stmt.format.clone();

        let mut inner_validator =
            Validator::create_from_stmt(&stmt.statement).ok_or_else(|| {
                ValidationError::new(
                    "Failed to create validator for inner statement".to_string(),
                    ValidationErrorType::SemanticError,
                )
            })?;

        if let Some(ref sm) = self.schema_manager {
            inner_validator.set_schema_manager(sm.clone());
        }

        self.inner_validator = Some(Box::new(inner_validator));

        Ok(())
    }

    /// Obtain the internal validator.
    pub fn inner_validator(&self) -> Option<&Validator> {
        self.inner_validator.as_deref()
    }

    /// Obtain the format type
    pub fn format(&self) -> &ExplainFormat {
        &self.format
    }

    pub fn validated_result(&self) -> ValidatedExplain {
        ValidatedExplain {
            format: self.format.clone(),
            inner_statement_type: self
                .inner_validator
                .as_ref()
                .map(|v| v.as_ref().statement_type().as_str().to_string())
                .unwrap_or_default(),
        }
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
/// The internal statement validation directly calls the `validate` method, passing in `stmt` and `qctx` as arguments.
impl StatementValidator for ExplainValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let explain_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Explain(explain_stmt) => explain_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected EXPLAIN statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // Extract the internal statements (before moving).
        let inner_stmt = *explain_stmt.statement.clone();

        self.validate_impl(explain_stmt)?;

        // Verify the internal statements.
        if let Some(ref mut inner) = self.inner_validator {
            let result = inner.validate(
                Arc::new(Ast::new(inner_stmt, ast.expr_context.clone())),
                qctx,
            );
            if !result.success {
                return Err(result.errors.first().cloned().unwrap_or_else(|| {
                    ValidationError::new(
                        "Internal statement validation failed".to_string(),
                        ValidationErrorType::SemanticError,
                    )
                }));
            }
        }

        let info = ValidationInfo::new();

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Explain
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

impl Default for ExplainValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// PROFILE Statement Validator
/// PROFILE is similar to EXPLAIN, but it actually executes the code and collects performance data.
#[derive(Debug)]
pub struct ProfileValidator {
    format: ExplainFormat,
    inner_validator: Option<Box<Validator>>,
    schema_manager: Option<Arc<SchemaManager>>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl ProfileValidator {
    pub fn new() -> Self {
        Self {
            format: ExplainFormat::Table,
            inner_validator: None,
            schema_manager: None,
            inputs: Vec::new(),
            outputs: vec![
                ColumnDef {
                    name: "id".to_string(),
                    type_: ValueType::Int,
                },
                ColumnDef {
                    name: "name".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "dependencies".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "profiling_data".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "operator info".to_string(),
                    type_: ValueType::String,
                },
            ],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_manager = Some(schema_manager);
    }

    fn validate_impl(&mut self, stmt: &ProfileStmt) -> Result<(), ValidationError> {
        self.format = stmt.format.clone();

        let mut inner_validator =
            Validator::create_from_stmt(&stmt.statement).ok_or_else(|| {
                ValidationError::new(
                    "Failed to create validator for inner statement".to_string(),
                    ValidationErrorType::SemanticError,
                )
            })?;

        if let Some(ref sm) = self.schema_manager {
            inner_validator.set_schema_manager(sm.clone());
        }

        self.inner_validator = Some(Box::new(inner_validator));

        Ok(())
    }

    /// Obtain the internal validator.
    pub fn inner_validator(&self) -> Option<&Validator> {
        self.inner_validator.as_deref()
    }

    /// Obtain the format type
    pub fn format(&self) -> &ExplainFormat {
        &self.format
    }

    pub fn validated_result(&self) -> ValidatedExplain {
        ValidatedExplain {
            format: self.format.clone(),
            inner_statement_type: self
                .inner_validator
                .as_ref()
                .map(|v| v.as_ref().statement_type().as_str().to_string())
                .unwrap_or_default(),
        }
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
/// The internal statement validation directly calls the `validate` method, passing in `stmt` and `qctx` as arguments.
impl StatementValidator for ProfileValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let profile_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Profile(profile_stmt) => profile_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected PROFILE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // Extract the internal statements (before moving).
        let inner_stmt = *profile_stmt.statement.clone();

        self.validate_impl(profile_stmt)?;

        // Verify the internal statements.
        if let Some(ref mut inner) = self.inner_validator {
            let result = inner.validate(
                Arc::new(Ast::new(inner_stmt, ast.expr_context.clone())),
                qctx,
            );
            if !result.success {
                return Err(result.errors.first().cloned().unwrap_or_else(|| {
                    ValidationError::new(
                        "Internal statement validation failed".to_string(),
                        ValidationErrorType::SemanticError,
                    )
                }));
            }
        }

        let info = ValidationInfo::new();

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Profile
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

impl Default for ProfileValidator {
    fn default() -> Self {
        Self::new()
    }
}
