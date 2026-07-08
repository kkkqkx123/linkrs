//! YIELD clause validator – New system version
//! Verify the expressions in the YIELD clause and the column definitions.
//!
//! This document has been restructured in accordance with the new trait + enumeration validator framework.
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. All original functions have been retained.
//! Column definition validation: At least one column must be present, and there must be no duplicate column names.
//! Alias verification
//! Type inference
//! - DISTINCT validation
//! 3. Use QueryContext to manage the context in a unified manner.
//! 4. Added schema validation support for property references.

use crate::core::metadata::SchemaManager;
use crate::core::YieldColumn;
use crate::query::parser::ast::stmt::Ast;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::helpers::schema_validator::SchemaValidator;
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::structs::AliasType;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::collections::HashMap;
use std::sync::Arc;

/// Verified YIELD information
#[derive(Debug, Clone)]
pub struct ValidatedYield {
    pub columns: Vec<YieldColumn>,
    pub distinct: bool,
    pub output_types: Vec<ValueType>,
}

/// YIELD Validator – New System Implementation
///
/// Functionality integrity assurance:
/// 1. Complete validation lifecycle
/// 2. Management of input/output columns
/// 3. Expression property tracing
/// 4. Column Alias Management
/// 5. Schema validation for property references
#[derive(Debug)]
pub struct YieldValidator {
    // List of the YIELD columns
    yield_columns: Vec<YieldColumn>,
    // Should duplicates be removed?
    distinct: bool,
    // Available alias mapping
    aliases_available: HashMap<String, ValueType>,
    // Input column definition
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
    validated_result: Option<ValidatedYield>,
    // Schema validator for property validation
    schema_validator: Option<SchemaValidator>,
    // Space name for schema lookup
    space_name: Option<String>,
    // Available variables and their types (variable_name -> tag_name/edge_type)
    available_vars: HashMap<String, String>,
}

impl YieldValidator {
    /// Create a new instance of the validator.
    pub fn new() -> Self {
        Self {
            yield_columns: Vec::new(),
            distinct: false,
            aliases_available: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validation_errors: Vec::new(),
            validated_result: None,
            schema_validator: None,
            space_name: None,
            available_vars: HashMap::new(),
        }
    }

    /// Create a new instance with schema manager
    pub fn with_schema_manager(schema_manager: Arc<SchemaManager>) -> Self {
        Self {
            yield_columns: Vec::new(),
            distinct: false,
            aliases_available: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validation_errors: Vec::new(),
            validated_result: None,
            schema_validator: Some(SchemaValidator::new(schema_manager)),
            space_name: None,
            available_vars: HashMap::new(),
        }
    }

    /// Set schema manager
    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_validator = Some(SchemaValidator::new(schema_manager));
    }

    /// Set space name
    pub fn set_space_name(&mut self, space_name: String) {
        self.space_name = Some(space_name);
    }

    /// Set available variables and their types
    pub fn set_available_vars(&mut self, vars: HashMap<String, String>) {
        self.available_vars = vars;
    }

    /// Add a variable with its type
    pub fn add_available_var(&mut self, var_name: String, var_type: String) {
        self.available_vars.insert(var_name, var_type);
    }

    /// Obtain the verification results.
    pub fn validated_result(&self) -> Option<&ValidatedYield> {
        self.validated_result.as_ref()
    }

    /// Obtain the list of verification errors.
    pub fn validation_errors(&self) -> &[ValidationError] {
        &self.validation_errors
    }

    /// Add verification errors.
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

    /// Add a YIELD column
    pub fn add_yield_column(&mut self, col: YieldColumn) {
        self.yield_columns.push(col);
    }

    /// Set whether to remove duplicates.
    pub fn set_distinct(&mut self, distinct: bool) {
        self.distinct = distinct;
    }

    /// Setting available aliases
    pub fn set_aliases_available(&mut self, aliases: HashMap<String, ValueType>) {
        self.aliases_available = aliases;
    }

    /// Obtain the list of the YIELD columns.
    pub fn yield_columns(&self) -> &[YieldColumn] {
        &self.yield_columns
    }

    /// Should duplicates be removed?
    pub fn is_distinct(&self) -> bool {
        self.distinct
    }

    /// Verify the YIELD statement (traditional method, maintaining backward compatibility)
    pub fn validate_yield(&mut self) -> Result<ValidatedYield, ValidationError> {
        self.validate_columns()?;
        self.validate_aliases()?;
        self.validate_types()?;
        self.validate_distinct()?;

        let mut output_types = Vec::new();
        for col in &self.yield_columns {
            let col_type = self.deduce_expr_type(&col.expression)?;
            output_types.push(col_type);
        }

        let result = ValidatedYield {
            columns: self.yield_columns.clone(),
            distinct: self.distinct,
            output_types,
        };

        self.validated_result = Some(result.clone());
        Ok(result)
    }

    /// Verify column definitions
    fn validate_columns(&self) -> Result<(), ValidationError> {
        if self.yield_columns.is_empty() {
            return Err(ValidationError::new(
                "The YIELD clause must have at least one column".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        let mut seen_names: HashMap<String, usize> = HashMap::new();
        for col in &self.yield_columns {
            let name = col.name().to_string();
            if name.is_empty() {
                return Err(ValidationError::new(
                    "YIELD columns must have a name or alias".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }

            let count = seen_names.entry(name.clone()).or_insert(0);
            *count += 1;

            if *count > 1 {
                return Err(ValidationError::new(
                    format!("Duplicate column names in YIELD clause '{}'", name),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    /// Verify the alias
    fn validate_aliases(&self) -> Result<(), ValidationError> {
        for col in &self.yield_columns {
            let alias = col.name();
            if !alias.starts_with('_') && alias.chars().next().unwrap_or_default().is_ascii_digit()
            {
                return Err(ValidationError::new(
                    format!("The alias '{}' cannot start with a number.", alias),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    /// Verification type
    fn validate_types(&mut self) -> Result<(), ValidationError> {
        for col in &self.yield_columns {
            let expr_type = self.deduce_expr_type(&col.expression)?;
            if expr_type == ValueType::Unknown {
                // Type inference failed. A warning is generated, but no error is reported.
                // In practical implementations, more stringent processing may be required.
            }

            // Validate property references if schema validator is available
            if let (Some(ref schema_validator), Some(ref space_name)) =
                (&self.schema_validator, &self.space_name)
            {
                if let Some(expr) = col.expression.get_expression() {
                    if let Err(e) = schema_validator.validate_expression_properties(
                        &expr,
                        space_name,
                        &self.available_vars,
                    ) {
                        return Err(ValidationError::new(
                            e.message,
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Verify the use of the `DISTINCT` keyword.
    fn validate_distinct(&self) -> Result<(), ValidationError> {
        if self.distinct && self.yield_columns.len() > 1 {
            let has_non_comparable = self.yield_columns.iter().any(|col| {
                let col_type = self
                    .deduce_expr_type(&col.expression)
                    .unwrap_or(ValueType::Unknown);
                !matches!(
                    col_type,
                    ValueType::Bool | ValueType::Int | ValueType::Float | ValueType::String
                )
            });
            if has_non_comparable {
                return Err(ValidationError::new(
                    "When using DISTINCT in the YIELD clause, all columns must be of comparable type".to_string(),
                    ValidationErrorType::TypeError,
                ));
            }
        }
        Ok(())
    }

    /// Determine the type of the expression.
    fn deduce_expr_type(
        &self,
        expression: &crate::core::types::expr::contextual::ContextualExpression,
    ) -> Result<ValueType, ValidationError> {
        if let Some(e) = expression.get_expression() {
            self.deduce_expr_type_internal(&e)
        } else {
            Ok(ValueType::Unknown)
        }
    }

    /// Internal method: Derivation of expression types
    fn deduce_expr_type_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
    ) -> Result<ValueType, ValidationError> {
        // If schema validator is available, use it for type inference
        if let (Some(ref schema_validator), Some(ref space_name)) =
            (&self.schema_validator, &self.space_name)
        {
            let input_columns: HashMap<String, ValueType> = self
                .inputs
                .iter()
                .map(|c| (c.name.clone(), c.type_.clone()))
                .collect();

            return Ok(schema_validator.infer_expression_type(
                expression,
                space_name,
                &self.available_vars,
                &input_columns,
            ));
        }

        // Fallback to basic type inference
        use crate::core::types::expr::Expression;
        match expression {
            Expression::Literal(value) => match value {
                crate::core::Value::Null(_) => Ok(ValueType::Null),
                crate::core::Value::Bool(_) => Ok(ValueType::Bool),
                crate::core::Value::SmallInt(_)
                | crate::core::Value::Int(_)
                | crate::core::Value::BigInt(_) => Ok(ValueType::Int),
                crate::core::Value::Float(_) | crate::core::Value::Double(_) => {
                    Ok(ValueType::Float)
                }
                crate::core::Value::String(_) => Ok(ValueType::String),
                crate::core::Value::Date(_) => Ok(ValueType::Date),
                crate::core::Value::Time(_) => Ok(ValueType::Time),
                crate::core::Value::DateTime(_) => Ok(ValueType::DateTime),
                crate::core::Value::Vertex(_) => Ok(ValueType::Vertex),
                crate::core::Value::Edge(_) => Ok(ValueType::Edge),
                crate::core::Value::Path(_) => Ok(ValueType::Path),
                crate::core::Value::List(_) => Ok(ValueType::List),
                crate::core::Value::Map(_) => Ok(ValueType::Map),
                crate::core::Value::Set(_) => Ok(ValueType::Set),
                _ => Ok(ValueType::Unknown),
            },
            Expression::Variable(name) => {
                // Look up in input columns
                for input in &self.inputs {
                    if &input.name == name {
                        return Ok(input.type_.clone());
                    }
                }
                Ok(ValueType::Unknown)
            }
            Expression::Property { .. } => {
                // Property type requires schema information
                Ok(ValueType::Unknown)
            }
            Expression::List(_) => Ok(ValueType::List),
            Expression::Map(_) => Ok(ValueType::Map),
            _ => Ok(ValueType::Unknown),
        }
    }

    /// Verify the specific sentence.
    fn validate_impl(&mut self) -> Result<(), ValidationError> {
        // Performing the YIELD verification
        let validated = self.validate_yield()?;

        // Set the output columns
        self.outputs.clear();
        for (i, col) in validated.columns.iter().enumerate() {
            let col_type = validated
                .output_types
                .get(i)
                .cloned()
                .unwrap_or(ValueType::Unknown);
            self.outputs.push(ColumnDef {
                name: col.name().to_string(),
                type_: col_type,
            });
        }

        Ok(())
    }
}

impl Default for YieldValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for YieldValidator {
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

        // Get space information from QueryContext
        if let Some(space_name) = qctx.space_name() {
            self.space_name = Some(space_name);
        }

        // Perform the specific validation logic.
        if let Err(e) = self.validate_impl() {
            self.add_error(e);
        }

        // If there are any validation errors, return a failure result.
        if self.has_errors() {
            let errors = self.validation_errors.clone();
            return Ok(ValidationResult::failure(errors));
        }

        let mut info = ValidationInfo::new();

        for column in &self.yield_columns {
            if !column.alias.is_empty() {
                info.add_alias(column.alias.clone(), AliasType::Expression);
            }
            info.semantic_info
                .output_fields
                .push(format!("{:?}", column.expression));
        }

        // Return the successful verification result.
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Yield
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // “YIELD” is not a global statement; therefore, the relevant space must be selected in advance.
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
    use crate::core::types::expr::contextual::ContextualExpression;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::{Expression, Value};
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use std::sync::Arc;

    /// Testing the auxiliary function: Creating a simple ContextualExpression
    fn create_test_contextual_expression(expr: Expression) -> ContextualExpression {
        let context = Arc::new(ExpressionAnalysisContext::new());
        let meta = ExpressionMeta::new(expr);
        let id = context.register_expression(meta);
        ContextualExpression::new(id, context)
    }

    #[test]
    fn test_yield_validator_new() {
        let validator = YieldValidator::new();
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());
        assert!(validator.validated_result().is_none());
        assert!(validator.validation_errors().is_empty());
    }

    #[test]
    fn test_yield_validator_default() {
        let validator: YieldValidator = Default::default();
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());
    }

    #[test]
    fn test_statement_type() {
        let validator = YieldValidator::new();
        assert_eq!(validator.statement_type(), StatementType::Yield);
    }

    #[test]
    fn test_yield_validation() {
        let mut validator = YieldValidator::new();

        // Add a column
        let col = YieldColumn::new(
            create_test_contextual_expression(Expression::Literal(Value::Int(42))),
            "result".to_string(),
        );
        validator.add_yield_column(col);

        let result = validator.validate_yield();
        assert!(result.is_ok());

        let validated = result.expect("Failed to validate yield");
        assert_eq!(validated.columns.len(), 1);
        assert!(!validated.distinct);
    }

    #[test]
    fn test_yield_empty_columns() {
        let mut validator = YieldValidator::new();

        // Do not add any columns.
        let result = validator.validate_yield();
        assert!(result.is_err());
    }

    #[test]
    fn test_yield_duplicate_column_names() {
        let mut validator = YieldValidator::new();

        // Add two columns with the same name.
        let col1 = YieldColumn::new(
            create_test_contextual_expression(Expression::Literal(Value::Int(1))),
            "result".to_string(),
        );
        let col2 = YieldColumn::new(
            create_test_contextual_expression(Expression::Literal(Value::Int(2))),
            "result".to_string(),
        );
        validator.add_yield_column(col1);
        validator.add_yield_column(col2);

        let result = validator.validate_yield();
        assert!(result.is_err());
    }

    #[test]
    fn test_yield_invalid_alias() {
        let mut validator = YieldValidator::new();

        // Add aliases that start with a number.
        let col = YieldColumn::new(
            create_test_contextual_expression(Expression::Literal(Value::Int(42))),
            "1result".to_string(),
        );
        validator.add_yield_column(col);

        let result = validator.validate_yield();
        assert!(result.is_err());
    }

    #[test]
    fn test_yield_with_distinct() {
        use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use std::sync::Arc;

        let mut validator = YieldValidator::new();

        // Add a column and set the property to `DISTINCT`.
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Literal(Value::Int(42));
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);
        let col = YieldColumn::new(ctx_expr, "result".to_string());
        validator.add_yield_column(col);
        validator.set_distinct(true);

        let result = validator.validate_yield();
        assert!(result.is_ok());

        let validated = result.expect("Failed to validate yield");
        assert!(validated.distinct);
    }
}
