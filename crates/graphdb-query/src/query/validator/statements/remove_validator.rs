//! Remove the statement validator.
//! Used to verify the REMOVE statement (deletion of properties/tagging in Cypher style)

use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::parser::ast::stmt::{Ast, RemoveStmt};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::structs::AliasType;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Remove the statement validator.
#[derive(Debug)]
pub struct RemoveValidator {
    items: Vec<ContextualExpression>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl RemoveValidator {
    /// Create a new Remove validator.
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    /// Verify the removed items.
    fn validate_remove_item(&self, item: &ContextualExpression) -> Result<(), ValidationError> {
        let expr_meta = item.expression().ok_or_else(|| {
            ValidationError::new(
                "Remove item expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            )
        })?;
        let expr = expr_meta.inner();
        self.validate_remove_item_internal(expr)
    }

    /// Internal method: Verify the removal of the item
    fn validate_remove_item_internal(
        &self,
        item: &crate::core::types::expr::Expression,
    ) -> Result<(), ValidationError> {
        use crate::core::types::expr::Expression;

        match item {
            // Remove property: REMOVE n.property
            Expression::Property { object, property } => {
                self.validate_property_access_internal(object, property)
            }
            // The variable itself: REMOVE n (Removes the node)
            Expression::Variable(var) => self.validate_variable_remove(var),
            _ => Err(ValidationError::new(
                format!("Invalid REMOVE expression: {:?}", item),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Internal method: Verification of attribute access removal
    fn validate_property_access_internal(
        &self,
        object: &crate::core::types::expr::Expression,
        property: &str,
    ) -> Result<(), ValidationError> {
        use crate::core::types::expr::Expression;

        // The object can be a variable or a literal (for direct vertex ID access like REMOVE 1.temp_field)
        match object {
            Expression::Variable(var_name) => {
                // Check whether the variable exists.
                if !self.user_defined_vars.iter().any(|v| v == var_name) {
                    return Err(ValidationError::new(
                        format!("Variable '{}' not defined", var_name),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            Expression::Literal(_) => {
                // Literal values (like 1 in "REMOVE 1.temp_field") are valid for direct vertex ID access
                // No additional validation needed for literals
            }
            _ => {
                return Err(ValidationError::new(
                    "REMOVE property target must be a variable or literal".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        // The attribute name cannot be empty.
        if property.is_empty() {
            return Err(ValidationError::new(
                "Property name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    /// Verify the removal of variables (deletion of nodes/edges)
    fn validate_variable_remove(&self, var: &str) -> Result<(), ValidationError> {
        // Check whether the variable exists.
        if !self.user_defined_vars.iter().any(|v| v == var) {
            return Err(ValidationError::new(
                format!("Variable '{}' not defined", var),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    /// Public method for validating property access (used in tests)
    pub fn validate_property_access(
        &self,
        object: &crate::core::types::expr::Expression,
        property: &str,
    ) -> Result<(), ValidationError> {
        self.validate_property_access_internal(object, property)
    }

    fn validate_impl(&mut self, stmt: &RemoveStmt) -> Result<(), ValidationError> {
        // Verify that there is at least one removed item.
        if stmt.items.is_empty() {
            return Err(ValidationError::new(
                "REMOVE clause must have at least one item".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify each item that has been removed.
        for item in &stmt.items {
            self.validate_remove_item(item)?;
        }

        // Save the information.
        self.items = stmt.items.clone();

        // Set the output columns
        self.setup_outputs();

        Ok(())
    }

    fn setup_outputs(&mut self) {
        // The `REMOVE` statement returns the number of items that were removed.
        self.outputs = vec![ColumnDef {
            name: "removed_count".to_string(),
            type_: ValueType::Int,
        }];
    }

    /// Setting the input columns (the columns passed from the parent query)
    pub fn set_inputs(&mut self, inputs: Vec<ColumnDef>) {
        // Update the available user-defined variables.
        self.user_defined_vars = inputs.iter().map(|c| c.name.clone()).collect();
        self.inputs = inputs;
    }
}

impl Default for RemoveValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as arguments.
impl StatementValidator for RemoveValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let remove_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Remove(remove_stmt) => remove_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected REMOVE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(remove_stmt)?;

        let mut info = ValidationInfo::new();

        for item in &self.items {
            if let Some(expr_meta) = item.expression() {
                let expr = expr_meta.inner();
                match expr {
                    crate::core::types::expr::Expression::Property {
                        object,
                        property: _,
                    } => {
                        if let crate::core::types::expr::Expression::Variable(var_name) =
                            object.as_ref()
                        {
                            info.add_alias(var_name.clone(), AliasType::Node);
                        }
                    }
                    crate::core::types::expr::Expression::Variable(var_name) => {
                        info.add_alias(var_name.clone(), AliasType::Node);
                    }
                    _ => {}
                }
            }
        }

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Remove
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // “REMOVE” is not a global statement.
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
    use crate::core::Expression;

    #[test]
    fn test_remove_validator_new() {
        let validator = RemoveValidator::new();
        assert_eq!(validator.statement_type(), StatementType::Remove);
        assert!(!validator.is_global_statement());
    }

    #[test]
    fn test_validate_property_access() {
        let mut validator = RemoveValidator::new();
        validator.user_defined_vars.push("n".to_string());

        // Effective access to attributes
        let obj = Expression::Variable("n".to_string());
        assert!(validator.validate_property_access(&obj, "name").is_ok());

        // Invalid attribute name
        assert!(validator.validate_property_access(&obj, "").is_err());

        // Undefined variable
        let obj2 = Expression::Variable("m".to_string());
        assert!(validator.validate_property_access(&obj2, "name").is_err());
    }

    #[test]
    fn test_validate_variable_remove() {
        let mut validator = RemoveValidator::new();
        validator.user_defined_vars.push("n".to_string());

        // Effective variables
        assert!(validator.validate_variable_remove("n").is_ok());

        // Undefined variable
        assert!(validator.validate_variable_remove("m").is_err());
    }
}
