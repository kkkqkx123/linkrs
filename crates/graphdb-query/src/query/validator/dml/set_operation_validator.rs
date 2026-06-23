//! Set operation statement validator
//! Corresponding to the functionality of NebulaGraph SetValidator
//! Verify set operation statements such as UNION, UNION ALL, INTERSECT, and MINUS.
//!
//! Design Principles:
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. Verify the compatibility of the number of columns and data types in the left and right subqueries.
//! 3. Support for various types of set operations

use crate::query::parser::ast::stmt::{Ast, SetOperationStmt, SetOperationType};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_enum::Validator;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

/// Verified set operation information
#[derive(Debug, Clone)]
pub struct ValidatedSetOperation {
    pub op_type: SetOperationType,
    pub left_outputs: Vec<ColumnDef>,
    pub right_outputs: Vec<ColumnDef>,
    pub output_col_names: Vec<String>,
}

/// Set operation validator
#[derive(Debug)]
pub struct SetOperationValidator {
    op_type: SetOperationType,
    left_validator: Option<Box<Validator>>,
    right_validator: Option<Box<Validator>>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl SetOperationValidator {
    pub fn new() -> Self {
        Self {
            op_type: SetOperationType::Union,
            left_validator: None,
            right_validator: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &SetOperationStmt) -> Result<(), ValidationError> {
        self.op_type = stmt.op_type.clone();

        // Create a left subquery validator
        self.left_validator = Some(Box::new(
            Validator::create_from_stmt(&stmt.left).ok_or_else(|| {
                ValidationError::new(
                    "Failed to create validator for left statement".to_string(),
                    ValidationErrorType::SemanticError,
                )
            })?,
        ));

        // Create a right subquery validator
        self.right_validator = Some(Box::new(
            Validator::create_from_stmt(&stmt.right).ok_or_else(|| {
                ValidationError::new(
                    "Failed to create validator for right statement".to_string(),
                    ValidationErrorType::SemanticError,
                )
            })?,
        ));

        Ok(())
    }

    fn validate_compatibility(
        &self,
        left_outputs: &[ColumnDef],
        right_outputs: &[ColumnDef],
    ) -> Result<(), ValidationError> {
        // Verify that the number of columns is the same.
        if left_outputs.len() != right_outputs.len() {
            return Err(ValidationError::new(
                format!(
                    "Set operation requires same number of columns: left has {}, right has {}",
                    left_outputs.len(),
                    right_outputs.len()
                ),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify column name compatibility (optional): You can require that the column names be the same or allow for automatic mapping.
        for (i, (left, right)) in left_outputs.iter().zip(right_outputs.iter()).enumerate() {
            // Verify the compatibility of data types
            if !Self::types_compatible(&left.type_, &right.type_) {
                return Err(ValidationError::new(
                    format!(
                        "Type mismatch at column {}: left is {:?}, right is {:?}",
                        i + 1,
                        left.type_,
                        right.type_
                    ),
                    ValidationErrorType::TypeError,
                ));
            }
        }

        Ok(())
    }

    fn types_compatible(left: &ValueType, right: &ValueType) -> bool {
        // Compatibility with the same type
        if left == right {
            return true;
        }

        // The “Unknown” type is compatible with any type.
        if matches!(left, ValueType::Unknown) || matches!(right, ValueType::Unknown) {
            return true;
        }

        // The Null type is compatible with any type.
        if matches!(left, ValueType::Null) || matches!(right, ValueType::Null) {
            return true;
        }

        // Compatibility between numeric data types
        let left_is_numeric = matches!(left, ValueType::Int | ValueType::Float);
        let right_is_numeric = matches!(right, ValueType::Int | ValueType::Float);
        if left_is_numeric && right_is_numeric {
            return true;
        }

        false
    }

    fn merge_outputs(&mut self, left_outputs: &[ColumnDef], right_outputs: &[ColumnDef]) {
        // The output column of the set operation uses the column names from the left subquery.
        // However, the type must be the compatible version of the type in question.
        self.outputs = left_outputs
            .iter()
            .zip(right_outputs.iter())
            .map(|(left, right)| ColumnDef {
                name: left.name.clone(),
                type_: Self::merge_types(&left.type_, &right.type_),
            })
            .collect();
    }

    fn merge_types(left: &ValueType, right: &ValueType) -> ValueType {
        if left == right {
            return left.clone();
        }

        // If one is “Unknown”, use the other one.
        if matches!(left, ValueType::Unknown) {
            return right.clone();
        }
        if matches!(right, ValueType::Unknown) {
            return left.clone();
        }

        // The numeric types have been merged into a single `Float` type.
        let left_is_numeric = matches!(left, ValueType::Int | ValueType::Float);
        let right_is_numeric = matches!(right, ValueType::Int | ValueType::Float);
        if left_is_numeric && right_is_numeric {
            return ValueType::Float;
        }

        // The default response is “Unknown”.
        ValueType::Unknown
    }

    /// Obtain the operation type
    pub fn op_type(&self) -> &SetOperationType {
        &self.op_type
    }

    /// Obtain the left subquery validator.
    pub fn left_validator(&self) -> Option<&Validator> {
        self.left_validator.as_deref()
    }

    /// Obtain the right subquery validator.
    pub fn right_validator(&self) -> Option<&Validator> {
        self.right_validator.as_deref()
    }

    pub fn validated_result(&self) -> ValidatedSetOperation {
        ValidatedSetOperation {
            op_type: self.op_type.clone(),
            left_outputs: self
                .left_validator
                .as_ref()
                .map(|v| v.get_outputs().to_vec())
                .unwrap_or_default(),
            right_outputs: self
                .right_validator
                .as_ref()
                .map(|v| v.get_outputs().to_vec())
                .unwrap_or_default(),
            output_col_names: self.outputs.iter().map(|c| c.name.clone()).collect(),
        }
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for SetOperationValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let set_op_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::SetOperation(set_op_stmt) => set_op_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected SET OPERATION statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // Extract the left and right subquery statements.
        let left_stmt = *set_op_stmt.left.clone();
        let right_stmt = *set_op_stmt.right.clone();

        self.validate_impl(set_op_stmt)?;

        // Verify the left and right subqueries
        let left_outputs = if let Some(ref mut left) = self.left_validator {
            let result = left.validate(
                Arc::new(Ast::new(left_stmt, ast.expr_context.clone())),
                qctx.clone(),
            );
            if result.success {
                result.outputs
            } else {
                return Err(result.errors.first().cloned().unwrap_or_else(|| {
                    ValidationError::new(
                        "Left subquery validation failed".to_string(),
                        ValidationErrorType::SemanticError,
                    )
                }));
            }
        } else {
            Vec::new()
        };

        let right_outputs = if let Some(ref mut right) = self.right_validator {
            let result = right.validate(
                Arc::new(Ast::new(right_stmt, ast.expr_context.clone())),
                qctx.clone(),
            );
            if result.success {
                result.outputs
            } else {
                return Err(result.errors.first().cloned().unwrap_or_else(|| {
                    ValidationError::new(
                        "Right subquery validation failed".to_string(),
                        ValidationErrorType::SemanticError,
                    )
                }));
            }
        } else {
            Vec::new()
        };

        // Verify compatibility
        self.validate_compatibility(&left_outputs, &right_outputs)?;

        // Merge the output columns
        self.merge_outputs(&left_outputs, &right_outputs);

        // Collecting user-defined variables
        if let Some(ref left) = self.left_validator {
            for var in left.get_user_defined_vars() {
                if !self.user_defined_vars.contains(&var.to_string()) {
                    self.user_defined_vars.push(var.to_string());
                }
            }
        }
        if let Some(ref right) = self.right_validator {
            for var in right.get_user_defined_vars() {
                if !self.user_defined_vars.contains(&var.to_string()) {
                    self.user_defined_vars.push(var.to_string());
                }
            }
        }

        let info = ValidationInfo::new();

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Set
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // Whether a set operation is a global statement depends on the left and right subqueries.
        let left_global = self
            .left_validator
            .as_ref()
            .map(|v| v.get_type().is_global_statement())
            .unwrap_or(false);
        let right_global = self
            .right_validator
            .as_ref()
            .map(|v| v.get_type().is_global_statement())
            .unwrap_or(false);
        left_global && right_global
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for SetOperationValidator {
    fn default() -> Self {
        Self::new()
    }
}
