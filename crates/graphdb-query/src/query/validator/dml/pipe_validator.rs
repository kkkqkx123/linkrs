//! Pipeline Operation Validator
//! Verify the compatibility of the queries before and after being connected by the pipeline operator `|`.
//!
//! This document has been restructured according to the new trait + enumeration validator framework.
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. All original functions have been retained.
//! Left-side output verification
//! Input validation on the right side
//! Column compatibility check
//! Pipeline connection verification
//! Type matching verification
//! 3. Use QueryContext to manage the context in a unified manner.

use crate::query::parser::ast::stmt::Ast;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

/// Column information definition
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub type_: ValueType,
    pub alias: Option<String>,
}

impl ColumnInfo {
    /// Create new column information
    pub fn new(name: String, type_: ValueType) -> Self {
        Self {
            name,
            type_,
            alias: None,
        }
    }

    /// Create column information with aliases
    pub fn with_alias(name: String, type_: ValueType, alias: String) -> Self {
        Self {
            name,
            type_,
            alias: Some(alias),
        }
    }
}

/// Pipe Validator – New System Implementation
///
/// Functionality integrity assurance:
/// 1. Complete validation lifecycle
/// 2. Management of input/output columns
/// 3. Expression property tracing
/// 4. Verification of pipeline connection compatibility
/// 5. Column type matching check
#[derive(Debug)]
pub struct PipeValidator {
    // The output column of the query on the left side
    left_output_cols: Vec<ColumnInfo>,
    // The input column for the search on the right side
    right_input_cols: Vec<ColumnInfo>,
    // Column definition (for the trait interface)
    inputs: Vec<ColumnDef>,
    // Column definition (the output of the pipeline operation corresponds to the output of the query on the right side)
    outputs: Vec<ColumnDef>,
    // Expression properties
    expr_props: ExpressionProps,
    // User-defined variables
    user_defined_vars: Vec<String>,
    // List of validation errors
    validation_errors: Vec<ValidationError>,
}

impl PipeValidator {
    /// Create a new instance of the validator.
    pub fn new() -> Self {
        Self {
            left_output_cols: Vec::new(),
            right_input_cols: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validation_errors: Vec::new(),
        }
    }

    /// Set the left output column
    pub fn set_left_output(&mut self, cols: Vec<ColumnInfo>) {
        self.left_output_cols = cols;
        // Synchronize with the inputs.
        self.inputs = self
            .left_output_cols
            .iter()
            .map(|col| ColumnDef {
                name: col.name.clone(),
                type_: col.type_.clone(),
            })
            .collect();
    }

    /// Set the input column on the right side
    pub fn set_right_input(&mut self, cols: Vec<ColumnInfo>) {
        self.right_input_cols = cols;
    }

    /// Add the left output column
    pub fn add_left_output(&mut self, col: ColumnInfo) {
        self.left_output_cols.push(col.clone());
        self.inputs.push(ColumnDef {
            name: col.name,
            type_: col.type_,
        });
    }

    /// Add an input column on the right side
    pub fn add_right_input(&mut self, col: ColumnInfo) {
        self.right_input_cols.push(col);
    }

    /// Get the output column on the left side.
    pub fn left_output_cols(&self) -> &[ColumnInfo] {
        &self.left_output_cols
    }

    /// Obtain the data from the input column on the right side.
    pub fn right_input_cols(&self) -> &[ColumnInfo] {
        &self.right_input_cols
    }

    /// Clear the verification errors.
    fn clear_errors(&mut self) {
        self.validation_errors.clear();
    }

    /// Perform validation (in the traditional way, while maintaining backward compatibility).
    pub fn validate_pipe(&mut self) -> Result<(), ValidationError> {
        self.clear_errors();
        self.validate_impl()?;
        Ok(())
    }

    fn validate_impl(&mut self) -> Result<(), ValidationError> {
        self.validate_left_output()?;
        self.validate_right_input()?;
        self.validate_compatibility()?;
        self.validate_pipe_connection()?;
        Ok(())
    }

    fn validate_left_output(&self) -> Result<(), ValidationError> {
        for col in &self.left_output_cols {
            if col.name.is_empty() {
                return Err(ValidationError::new(
                    "Pipe left side has empty column name".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    fn validate_right_input(&self) -> Result<(), ValidationError> {
        for col in &self.right_input_cols {
            if col.name.is_empty() {
                return Err(ValidationError::new(
                    "Pipe right side has empty column reference".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    fn validate_compatibility(&self) -> Result<(), ValidationError> {
        if self.left_output_cols.is_empty() && !self.right_input_cols.is_empty() {
            return Err(ValidationError::new(
                "Pipe left side has no output columns but right side requires input".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        for right_col in &self.right_input_cols {
            let mut found = false;
            for left_col in &self.left_output_cols {
                if right_col.name == left_col.name {
                    if right_col.type_ != left_col.type_ && left_col.type_ != ValueType::Unknown {
                        return Err(ValidationError::new(
                            format!(
                                "Column type mismatch for '{}': left output is {:?}, right input requires {:?}",
                                right_col.name, left_col.type_, right_col.type_
                            ),
                            ValidationErrorType::TypeError,
                        ));
                    }
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(ValidationError::new(
                    format!(
                        "Column '{}' referenced in pipe right side not found in left output",
                        right_col.name
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    fn validate_pipe_connection(&self) -> Result<(), ValidationError> {
        if self.left_output_cols.is_empty() && self.right_input_cols.is_empty() {
            return Ok(());
        }

        if !self.right_input_cols.is_empty() && self.left_output_cols.is_empty() {
            return Err(ValidationError::new(
                "Pipe requires input from previous query but previous query has no output"
                    .to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Verify pipeline compatibility (static method, for easy direct use)
    pub fn validate_pipe_compatibility(
        left_outputs: &[ColumnInfo],
        right_inputs: &[ColumnInfo],
    ) -> Result<(), ValidationError> {
        let mut validator = Self::new();
        validator.set_left_output(left_outputs.to_vec());
        validator.set_right_input(right_inputs.to_vec());
        validator.validate_pipe()
    }
}

impl Default for PipeValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for PipeValidator {
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

        // The output of the pipeline operation corresponds to the output of the query on the right side.
        // If the input column on the right side is not available, then the content from the output column on the left side should be displayed.
        self.outputs = if self.right_input_cols.is_empty() {
            self.inputs.clone()
        } else {
            self.right_input_cols
                .iter()
                .map(|col| ColumnDef {
                    name: col.name.clone(),
                    type_: col.type_.clone(),
                })
                .collect()
        };

        let info = ValidationInfo::new();

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Pipe
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `PIPE` statement is not a global statement; therefore, the relevant memory space must be selected in advance.
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
    fn test_pipe_validator_new() {
        let validator = PipeValidator::new();
        assert!(validator.left_output_cols().is_empty());
        assert!(validator.right_input_cols().is_empty());
    }

    #[test]
    fn test_set_left_output() {
        let mut validator = PipeValidator::new();
        let cols = vec![
            ColumnInfo::new("col1".to_string(), ValueType::String),
            ColumnInfo::new("col2".to_string(), ValueType::Int),
        ];
        validator.set_left_output(cols);
        assert_eq!(validator.left_output_cols().len(), 2);
    }

    #[test]
    fn test_validate_empty_columns() {
        let mut validator = PipeValidator::new();
        // Empty pipes are allowed.
        let result = validator.validate_pipe();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_compatible_columns() {
        let mut validator = PipeValidator::new();
        let left_cols = vec![
            ColumnInfo::new("name".to_string(), ValueType::String),
            ColumnInfo::new("age".to_string(), ValueType::Int),
        ];
        let right_cols = vec![ColumnInfo::new("name".to_string(), ValueType::String)];
        validator.set_left_output(left_cols);
        validator.set_right_input(right_cols);

        let result = validator.validate_pipe();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_incompatible_type() {
        let mut validator = PipeValidator::new();
        let left_cols = vec![ColumnInfo::new("age".to_string(), ValueType::Int)];
        let right_cols = vec![ColumnInfo::new("age".to_string(), ValueType::String)];
        validator.set_left_output(left_cols);
        validator.set_right_input(right_cols);

        let result = validator.validate_pipe();
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_missing_column() {
        let mut validator = PipeValidator::new();
        let left_cols = vec![ColumnInfo::new("name".to_string(), ValueType::String)];
        let right_cols = vec![ColumnInfo::new("age".to_string(), ValueType::Int)];
        validator.set_left_output(left_cols);
        validator.set_right_input(right_cols);

        let result = validator.validate_pipe();
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_static_method() {
        let left_cols = vec![ColumnInfo::new("name".to_string(), ValueType::String)];
        let right_cols = vec![ColumnInfo::new("name".to_string(), ValueType::String)];

        let result = PipeValidator::validate_pipe_compatibility(&left_cols, &right_cols);
        assert!(result.is_ok());
    }
}
