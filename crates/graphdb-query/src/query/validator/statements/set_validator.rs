//! SET/GET/SHOW statement validator – New system version
//! Verify the validity of SET/GET/SHOW statements
//!
//! This document has been restructured in accordance with the new trait + enumeration validator framework.
//! - The StatementValidator trait has been implemented to unify the interface.
//! - 2. All original functions have been retained.
//! - SET variable validation
//! - SET Tag/Edge property validation
//! - SET priority verification
//! - Expression validation
//! - 3. Use AstContext to manage contexts in a unified manner.

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::Expression;
use crate::query::parser::ast::stmt::Ast;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::structs::AliasType;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Types of SET statements
#[derive(Debug, Clone, PartialEq)]
pub enum SetStatementType {
    SetVariable,
    SetTag,
    SetEdge,
    SetPriority,
}

/// SET item definition
#[derive(Debug, Clone)]
pub struct SetItem {
    pub statement_type: SetStatementType,
    pub target: ContextualExpression,
    pub value: ContextualExpression,
}

impl SetItem {
    /// Create a new SET item.
    pub fn new(
        statement_type: SetStatementType,
        target: ContextualExpression,
        value: ContextualExpression,
    ) -> Self {
        Self {
            statement_type,
            target,
            value,
        }
    }
}

/// Verified SET information
#[derive(Debug, Clone)]
pub struct ValidatedSet {
    pub items: Vec<ValidatedSetItem>,
    pub variables: HashMap<String, ContextualExpression>,
}

/// Verified SET items
#[derive(Debug, Clone)]
pub struct ValidatedSetItem {
    pub statement_type: SetStatementType,
    pub target: ContextualExpression,
    pub value: ContextualExpression,
}

/// SET Validator – New Implementation of the System
///
/// Functionality integrity assurance:
/// 1. Complete validation lifecycle
/// 2. Management of input/output columns
/// 3. Expression property tracking
/// 4. Variable Management
#[derive(Debug)]
pub struct SetValidator {
    // List of SET items
    set_items: Vec<SetItem>,
    // Variable mapping
    variables: HashMap<String, ContextualExpression>,
    // Column definition
    inputs: Vec<ColumnDef>,
    // Column definition
    outputs: Vec<ColumnDef>,
    // Expression properties
    expr_props: ExpressionProps,
    // User-defined variables
    user_defined_vars: Vec<String>,
    // List of validation errors
    validation_errors: Vec<ValidationError>,
    // Cache validation results
    validated_result: Option<ValidatedSet>,
}

impl SetValidator {
    /// Create a new instance of the validator.
    pub fn new() -> Self {
        Self {
            set_items: Vec::new(),
            variables: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validation_errors: Vec::new(),
            validated_result: None,
        }
    }

    /// Obtain the verification results.
    pub fn validated_result(&self) -> Option<&ValidatedSet> {
        self.validated_result.as_ref()
    }

    /// Retrieve the list of verification errors.
    pub fn validation_errors(&self) -> &[ValidationError] {
        &self.validation_errors
    }

    /// Add validation errors.
    fn add_error(&mut self, error: ValidationError) {
        self.validation_errors.push(error);
    }

    /// Clear the verification errors.
    fn clear_errors(&mut self) {
        self.validation_errors.clear();
    }

    /// Check for any validation errors.
    fn has_errors(&self) -> bool {
        !self.validation_errors.is_empty()
    }

    /// Add a SET item
    pub fn add_set_item(&mut self, item: SetItem) {
        self.set_items.push(item);
    }

    /// Setting variables
    pub fn set_variable(&mut self, name: String, value: ContextualExpression) {
        self.variables.insert(name.clone(), value);
        if !self.user_defined_vars.contains(&name) {
            self.user_defined_vars.push(name);
        }
    }

    /// Obtain a list of SET items
    pub fn set_items(&self) -> &[SetItem] {
        &self.set_items
    }

    /// Obtain the variable mapping.
    pub fn variables(&self) -> &HashMap<String, ContextualExpression> {
        &self.variables
    }

    /// Verify the SET statement (traditional method, maintaining backward compatibility)
    pub fn validate_set(&mut self) -> Result<ValidatedSet, ValidationError> {
        let mut validated_items = Vec::new();

        for item in &self.set_items {
            self.validate_set_item(item)?;
            validated_items.push(ValidatedSetItem {
                statement_type: item.statement_type.clone(),
                target: item.target.clone(),
                value: item.value.clone(),
            });
        }

        self.validate_variables()?;

        let result = ValidatedSet {
            items: validated_items,
            variables: self.variables.clone(),
        };

        self.validated_result = Some(result.clone());
        Ok(result)
    }

    /// Verify a single item in a SET
    fn validate_set_item(&self, item: &SetItem) -> Result<(), ValidationError> {
        match item.statement_type {
            SetStatementType::SetVariable => {
                self.validate_set_variable(&item.target, &item.value)?;
            }
            SetStatementType::SetTag => {
                self.validate_set_tag(&item.target, &item.value)?;
            }
            SetStatementType::SetEdge => {
                self.validate_set_edge(&item.target, &item.value)?;
            }
            SetStatementType::SetPriority => {
                self.validate_set_priority(&item.value)?;
            }
        }
        Ok(())
    }

    /// Verify the SET variable
    fn validate_set_variable(
        &self,
        target: &ContextualExpression,
        _value: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        if target.expression().is_none() {
            return Err(ValidationError::new(
                "SET Target expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if !target.is_variable() {
            return Err(ValidationError::new(
                "SET variable must target a variable".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if let Some(name) = target.as_variable() {
            if name.is_empty() {
                return Err(ValidationError::new(
                    "The variable name cannot be null".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
            if !name.starts_with('$') {
                return Err(ValidationError::new(
                    format!("The variable name '{}' must start with '$'.", name),
                    ValidationErrorType::SemanticError,
                ));
            }
            Ok(())
        } else {
            Err(ValidationError::new(
                "SET variable must target a variable".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Verify the SET Tag
    fn validate_set_tag(
        &self,
        target: &ContextualExpression,
        _value: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        if target.expression().is_none() {
            return Err(ValidationError::new(
                "SET Tag Target expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if !target.is_property() {
            return Err(ValidationError::new(
                "The SET Tag must target an attribute expression.".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Verify the SET Edge configuration.
    fn validate_set_edge(
        &self,
        target: &ContextualExpression,
        _value: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        if target.expression().is_none() {
            return Err(ValidationError::new(
                "SET Edge Target expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if !target.is_property() {
            return Err(ValidationError::new(
                "SET Edge must target an attribute expression".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Verify the SET priority
    fn validate_set_priority(&self, value: &ContextualExpression) -> Result<(), ValidationError> {
        let expr_meta = match value.expression() {
            Some(m) => m,
            None => {
                return Err(ValidationError::new(
                    "SET Invalid priority expression".to_string(),
                    ValidationErrorType::SemanticError,
                ))
            }
        };
        let expr = expr_meta.inner();

        match expr {
            Expression::Literal(lit) => {
                if let crate::core::Value::Int(n) = lit {
                    if *n < 0 {
                        return Err(ValidationError::new(
                            "Priority cannot be negative".to_string(),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                    Ok(())
                } else {
                    Err(ValidationError::new(
                        "Priority must be an integer".to_string(),
                        ValidationErrorType::TypeError,
                    ))
                }
            }
            _ => Err(ValidationError::new(
                "Priority must be an integer literal".to_string(),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Verify the variable
    fn validate_variables(&self) -> Result<(), ValidationError> {
        for (name, value) in &self.variables {
            if name.is_empty() {
                return Err(ValidationError::new(
                    "The variable name cannot be null".to_string(),
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
            // Verify the variable value expression
            if let Some(expr_meta) = value.expression() {
                self.validate_expression(expr_meta.inner())?;
            }
        }
        Ok(())
    }

    /// Verify the expression
    fn validate_expression(&self, expression: &Expression) -> Result<(), ValidationError> {
        match expression {
            Expression::Binary { left, right, .. } => {
                self.validate_expression(left)?;
                self.validate_expression(right)?;
            }
            Expression::Unary { operand, .. } => {
                self.validate_expression(operand)?;
            }
            Expression::Function { args, .. } => {
                for arg in args {
                    self.validate_expression(arg)?;
                }
            }
            Expression::List(items) => {
                for item in items {
                    self.validate_expression(item)?;
                }
            }
            Expression::Map(pairs) => {
                for (_, value) in pairs {
                    self.validate_expression(value)?;
                }
            }
            Expression::Case {
                conditions,
                default,
                ..
            } => {
                for (condition, expr) in conditions {
                    self.validate_expression(condition)?;
                    self.validate_expression(expr)?;
                }
                if let Some(default_expr) = default {
                    self.validate_expression(default_expr)?;
                }
            }
            Expression::TypeCast { expression, .. } => {
                self.validate_expression(expression)?;
            }
            Expression::Subscript { collection, index } => {
                self.validate_expression(collection)?;
                self.validate_expression(index)?;
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                self.validate_expression(collection)?;
                if let Some(start_expr) = start {
                    self.validate_expression(start_expr)?;
                }
                if let Some(end_expr) = end {
                    self.validate_expression(end_expr)?;
                }
            }
            Expression::Path(items) => {
                for item in items {
                    self.validate_expression(item)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Verify the specific sentence.
    ///
    /// # Refactoring Changes
    /// Remove the AstContext parameter.
    /// - Receive the Arc<QueryContext> parameter
    fn validate_impl(&mut self, _qctx: Arc<QueryContext>) -> Result<(), ValidationError> {
        // Performing the SET validation
        self.validate_set()?;

        // The output of the SET statement is the value assigned to the variable.
        self.outputs.clear();
        for name in self.variables.keys() {
            self.outputs.push(ColumnDef {
                name: name.clone(),
                type_: ValueType::Unknown,
            });
        }

        Ok(())
    }
}

impl Default for SetValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Reconfiguration changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
/// Remove operations related to AstContext.
impl StatementValidator for SetValidator {
    fn validate(
        &mut self,
        _ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        // Clear the previous state.
        self.outputs.clear();
        self.inputs.clear();
        self.expr_props = ExpressionProps::default();
        self.clear_errors();

        // Perform the specific validation logic.
        if let Err(e) = self.validate_impl(qctx) {
            self.add_error(e);
        }

        // If there are any validation errors, return a failure result.
        if self.has_errors() {
            let errors = self.validation_errors.clone();
            return Ok(ValidationResult::failure(errors));
        }

        let mut info = ValidationInfo::new();

        for item in &self.set_items {
            if let Some(expr_meta) = item.target.expression() {
                let expr = expr_meta.inner();
                if let Expression::Variable(var_name) = expr {
                    info.add_alias(var_name.clone(), AliasType::Variable);
                }
            }
        }

        // Return the successful verification result.
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
        // The `SET` statement is a global statement; therefore, there is no need to pre-select a specific scope (i.e., a specific database or table) for its execution.
        true
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
    use crate::core::types::expr::contextual::ContextualExpression;
    use crate::core::Expression;
    use crate::core::Value;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;

    fn create_contextual_expr(expr: Expression) -> ContextualExpression {
        let ctx = std::sync::Arc::new(ExpressionAnalysisContext::new());
        let meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    #[test]
    fn test_set_validator_new() {
        let validator = SetValidator::new();
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());
        assert!(validator.validated_result().is_none());
        assert!(validator.validation_errors().is_empty());
    }

    #[test]
    fn test_set_validator_default() {
        let validator: SetValidator = Default::default();
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());
    }

    #[test]
    fn test_statement_type() {
        let validator = SetValidator::new();
        assert_eq!(validator.statement_type(), StatementType::Set);
    }

    #[test]
    fn test_set_variable_validation() {
        let mut validator = SetValidator::new();

        // Testing the effectiveness of the variable settings
        let item = SetItem::new(
            SetStatementType::SetVariable,
            create_contextual_expr(Expression::Variable("$var".to_string())),
            create_contextual_expr(Expression::Literal(Value::Int(42))),
        );
        validator.add_set_item(item);

        let result = validator.validate_set();
        assert!(result.is_ok());
    }

    #[test]
    fn test_set_variable_invalid_name() {
        let mut validator = SetValidator::new();

        // Test invalid variable names (those that do not start with $).
        let item = SetItem::new(
            SetStatementType::SetVariable,
            create_contextual_expr(Expression::Variable("var".to_string())),
            create_contextual_expr(Expression::Literal(Value::Int(42))),
        );
        validator.add_set_item(item);

        let result = validator.validate_set();
        assert!(result.is_err());
    }

    #[test]
    fn test_set_priority_validation() {
        let mut validator = SetValidator::new();

        // Testing the effectiveness of the priority settings
        let item = SetItem::new(
            SetStatementType::SetPriority,
            create_contextual_expr(Expression::Variable("$priority".to_string())),
            create_contextual_expr(Expression::Literal(Value::Int(5))),
        );
        validator.add_set_item(item);

        let result = validator.validate_set();
        assert!(result.is_ok());
    }

    #[test]
    fn test_set_priority_negative() {
        let mut validator = SetValidator::new();

        // Testing invalid priorities (negative numbers)
        let item = SetItem::new(
            SetStatementType::SetPriority,
            create_contextual_expr(Expression::Variable("$priority".to_string())),
            create_contextual_expr(Expression::Literal(Value::Int(-1))),
        );
        validator.add_set_item(item);

        let result = validator.validate_set();
        assert!(result.is_err());
    }
}
