//! Update Configs Statement Validator
//! Used to verify the UPDATE CONFIGS statement

use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::parser::ast::stmt::{Ast, UpdateConfigsStmt};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Update Configs Statement Validator
#[derive(Debug)]
pub struct UpdateConfigsValidator {
    module: Option<String>,
    config_name: String,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl UpdateConfigsValidator {
    /// Create a new UpdateConfigs validator.
    pub fn new() -> Self {
        Self {
            module: None,
            config_name: String::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    /// Verify the configuration module name.
    fn validate_module(&self, module: &Option<String>) -> Result<(), ValidationError> {
        if let Some(ref m) = module {
            let valid_modules = ["GRAPH", "META", "STORAGE", "ALL"];
            let upper = m.to_uppercase();
            if !valid_modules.contains(&upper.as_str()) {
                return Err(ValidationError::new(
                    format!(
                        "Invalid config module: {}. Valid modules are: GRAPH, META, STORAGE, ALL",
                        m
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    /// Verify the configuration name.
    fn validate_config_name(&self, name: &str) -> Result<(), ValidationError> {
        if name.is_empty() {
            return Err(ValidationError::new(
                "Config name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Check the format of the configuration name (only letters, digits, and underscores are allowed).
        if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(ValidationError::new(
                format!("Invalid config name format: {}", name),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    /// Verify the configuration values.
    fn validate_config_value(&self, value: &ContextualExpression) -> Result<(), ValidationError> {
        if let Some(e) = value.get_expression() {
            self.validate_config_value_internal(&e)
        } else {
            Err(ValidationError::new(
                "Invalid configuration value expression".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Internal method: Verifying configuration values
    fn validate_config_value_internal(
        &self,
        value: &crate::core::types::expr::Expression,
    ) -> Result<(), ValidationError> {
        use crate::core::types::expr::Expression;

        // The configuration values must be constant expressions.
        match value {
            Expression::Literal(_) => Ok(()),
            _ => Err(ValidationError::new(
                "Config value must be a constant expression".to_string(),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    fn validate_impl(&mut self, stmt: &UpdateConfigsStmt) -> Result<(), ValidationError> {
        // Verify the module name.
        self.validate_module(&stmt.module)?;

        // Verify the configuration name.
        self.validate_config_name(&stmt.config_name)?;

        // Verify the configuration values.
        self.validate_config_value(&stmt.config_value)?;

        // Save the information.
        self.module = stmt.module.clone();
        self.config_name = stmt.config_name.clone();

        // Set the output columns
        self.setup_outputs();

        Ok(())
    }

    fn setup_outputs(&mut self) {
        // The “UPDATE CONFIGS” command outputs the results of the configuration updates.
        self.outputs = vec![
            ColumnDef {
                name: "module".to_string(),
                type_: ValueType::String,
            },
            ColumnDef {
                name: "name".to_string(),
                type_: ValueType::String,
            },
            ColumnDef {
                name: "value".to_string(),
                type_: ValueType::String,
            },
        ];
    }
}

impl Default for UpdateConfigsValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for UpdateConfigsValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let update_configs_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::UpdateConfigs(update_configs_stmt) => {
                update_configs_stmt
            }
            _ => {
                return Err(ValidationError::new(
                    "Expected UPDATE CONFIGS statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(update_configs_stmt)?;

        let mut info = ValidationInfo::new();

        info.semantic_info.query_type = Some("UpdateConfigs".to_string());

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::UpdateConfigs
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // “UPDATE CONFIGS” is a global statement.
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
    use crate::core::types::expr::Expression;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::Value;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_update_configs_validator_new() {
        let validator = UpdateConfigsValidator::new();
        assert_eq!(validator.statement_type(), StatementType::UpdateConfigs);
        assert!(validator.is_global_statement());
    }

    #[test]
    fn test_validate_module() {
        let validator = UpdateConfigsValidator::new();

        // Effective module
        assert!(validator
            .validate_module(&Some("GRAPH".to_string()))
            .is_ok());
        assert!(validator.validate_module(&Some("META".to_string())).is_ok());
        assert!(validator
            .validate_module(&Some("STORAGE".to_string()))
            .is_ok());
        assert!(validator.validate_module(&Some("ALL".to_string())).is_ok());
        assert!(validator.validate_module(&None).is_ok());

        // Invalid module
        assert!(validator
            .validate_module(&Some("INVALID".to_string()))
            .is_err());
    }

    #[test]
    fn test_validate_config_name() {
        let validator = UpdateConfigsValidator::new();

        // Valid configuration name
        assert!(validator.validate_config_name("max_connections").is_ok());
        assert!(validator.validate_config_name("timeout_ms").is_ok());

        // Invalid configuration name
        assert!(validator.validate_config_name("").is_err());
        assert!(validator.validate_config_name("invalid-name").is_err());
        assert!(validator.validate_config_name("invalid.name").is_err());
    }

    #[test]
    fn test_validate_config_value() {
        let validator = UpdateConfigsValidator::new();
        let expr_context = ExpressionAnalysisContext::new();

        // Valid configuration values
        let int_meta = ExpressionMeta::new(Expression::Literal(Value::Int(100)));
        let int_expr_id = expr_context.register_expression(int_meta);
        let int_expr = ContextualExpression::new(int_expr_id, Arc::new(expr_context.clone()));
        assert!(validator.validate_config_value(&int_expr).is_ok());

        let bool_meta = ExpressionMeta::new(Expression::Literal(Value::Bool(true)));
        let bool_expr_id = expr_context.register_expression(bool_meta);
        let bool_expr = ContextualExpression::new(bool_expr_id, Arc::new(expr_context.clone()));
        assert!(validator.validate_config_value(&bool_expr).is_ok());

        // Invalid configuration value (non-constant).
        let var_meta = ExpressionMeta::new(Expression::Variable("var".to_string()));
        let var_expr_id = expr_context.register_expression(var_meta);
        let var_expr = ContextualExpression::new(var_expr_id, Arc::new(expr_context));
        assert!(validator.validate_config_value(&var_expr).is_err());
    }
}
