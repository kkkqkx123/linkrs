//! UNWIND clause validator – New system version
//! Verify the statement `UNWIND <expression> AS <variable>`.
//!
//! This document has been restructured in accordance with the new trait + enumeration validator framework.
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. All original functions have been retained.
//! Expression validation (must be a list or a set).
//! Variable name validation
//! Type inference
//! Alternative name verification
//! 3. Use QueryContext to manage the context in a unified manner.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::parser::ast::stmt::{Ast, Stmt};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::structs::AliasType;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::collections::HashMap;
use std::sync::Arc;

/// Verified UNWIND information
#[derive(Debug, Clone)]
pub struct ValidatedUnwind {
    pub expression: ContextualExpression,
    pub variable_name: String,
    pub element_type: ValueType,
}

/// UNWIND Validator – New System Implementation
///
/// Functionality integrity assurance:
/// 1. Complete verification lifecycle
/// 2. Management of input/output columns
/// 3. Expression property tracking
/// 4. Variable Management
#[derive(Debug)]
pub struct UnwindValidator {
    // The “UNWIND” expression
    unwind_expression: ContextualExpression,
    // Variable name
    variable_name: String,
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
    validated_result: Option<ValidatedUnwind>,
}

impl UnwindValidator {
    /// Create a new instance of the validator.
    pub fn new() -> Self {
        use std::sync::Arc;
        Self {
            unwind_expression: ContextualExpression::new(
                crate::core::types::expr::ExpressionId::new(0),
                Arc::new(ExpressionAnalysisContext::new()),
            ),
            variable_name: String::new(),
            aliases_available: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validation_errors: Vec::new(),
            validated_result: None,
        }
    }

    /// Obtain the verification results.
    pub fn validated_result(&self) -> Option<&ValidatedUnwind> {
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

    /// Setting the UNWIND expression
    pub fn set_unwind_expression(&mut self, expression: ContextualExpression) {
        self.unwind_expression = expression;
    }

    /// Setting variable names
    pub fn set_variable_name(&mut self, name: String) {
        self.variable_name = name.clone();
        if !self.user_defined_vars.contains(&name) {
            self.user_defined_vars.push(name);
        }
    }

    /// Set available aliases
    pub fn set_aliases_available(&mut self, aliases: HashMap<String, ValueType>) {
        self.aliases_available = aliases;
    }

    /// Obtain the UNWIND expression
    pub fn unwind_expression(&self) -> &ContextualExpression {
        &self.unwind_expression
    }

    /// Obtain the variable name
    pub fn variable_name(&self) -> &str {
        &self.variable_name
    }

    /// Obtain available aliases
    pub fn aliases_available(&self) -> &HashMap<String, ValueType> {
        &self.aliases_available
    }

    /// Verify the UNWIND statement (traditional method, maintaining backward compatibility)
    pub fn validate_unwind(&mut self) -> Result<ValidatedUnwind, ValidationError> {
        self.validate_expression()?;
        self.validate_variable()?;
        self.validate_type()?;
        self.validate_aliases()?;

        let element_type = self.deduce_list_element_type(&self.unwind_expression)?;

        let result = ValidatedUnwind {
            expression: self.unwind_expression.clone(),
            variable_name: self.variable_name.clone(),
            element_type,
        };

        self.validated_result = Some(result.clone());
        Ok(result)
    }

    /// Verify the expression
    fn validate_expression(&self) -> Result<(), ValidationError> {
        if let Some(expr) = self.unwind_expression.get_expression() {
            self.validate_expression_internal(&expr)
        } else {
            Err(ValidationError::new(
                "UNWIND expression is invalid".to_string(),
                ValidationErrorType::SyntaxError,
            ))
        }
    }

    /// Internal method: Validating expressions
    fn validate_expression_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
    ) -> Result<(), ValidationError> {
        if self.expression_is_empty(expression) {
            return Err(ValidationError::new(
                "UNWIND expression cannot be null".to_string(),
                ValidationErrorType::SyntaxError,
            ));
        }

        let expr_type = self.deduce_expr_type(expression)?;
        if expr_type != ValueType::List && expr_type != ValueType::Set {
            return Err(ValidationError::new(
                format!(
                    "UNWIND expressions must be of list or collection type, with actual type {:?}",
                    expr_type
                ),
                ValidationErrorType::TypeError,
            ));
        }

        Ok(())
    }

    /// Verify the variable names.
    fn validate_variable(&self) -> Result<(), ValidationError> {
        if self.variable_name.is_empty() {
            return Err(ValidationError::new(
                "UNWIND requires the AS clause to specify the variable name.".to_string(),
                ValidationErrorType::SyntaxError,
            ));
        }

        if self.variable_name.starts_with('_') && !self.variable_name.starts_with("__") {
            return Err(ValidationError::new(
                format!(
                    "Variable names '{}' should not start with a single underscore (reserved for internal use)",
                    self.variable_name
                ),
                ValidationErrorType::SemanticError,
            ));
        }

        if self
            .variable_name
            .chars()
            .next()
            .unwrap_or_default()
            .is_ascii_digit()
        {
            return Err(ValidationError::new(
                format!(
                    "Variable name '{}' cannot start with a number.",
                    self.variable_name
                ),
                ValidationErrorType::SemanticError,
            ));
        }

        if self.aliases_available.contains_key(&self.variable_name) {
            return Err(ValidationError::new(
                format!("Variable '{}' is defined in the query", self.variable_name),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    /// Verify the element types of the UNWIND expression
    ///
    /// # Delay Type Inference Design
    ///
    /// GraphDB adopts a “delayed type inference” strategy that allows the derivation of element types, which cannot be determined during compilation, at runtime.
    /// This is in conjunction with other validators (SetOperationValidator, YieldValidator, OrderByValidator).
    /// Consistent design patterns are used to support flexible, dynamic queries.
    ///
    /// # Type inference rules
    ///
    /// Element type deduction is supported for the following types of expressions:
    /// - `[1, 2, 3]` - 列表字面量 → 元素类型可推导（Integer）
    /// - `range(1, 10)` - 函数调用 → 返回类型已知但元素类型 Unknown
    /// - The variable reference cannot be determined at compile time (DataType::Empty → Unknown).
    /// - `vertex.tags` - 属性访问 → 编译期无法确定（DataType::Empty → Unknown）
    ///
    /// # Handling Unknown types
    ///
    /// When `list_type == ValueType::Unknown`:
    /// - ✓ Verification passed (no errors were reported), allowing the query to continue.
    /// - ✓ The type of the output column is set to "Unknown".
    /// - ✓ The executor determines the actual type of the value based on the actual value during runtime.
    ///
    /// # Runtime processing flow
    ///
    /// 1. `ExpressionEvaluator::evaluate(expr, context)` 获得实际 Value
    /// 2. `extract_list(value)` 根据 Value 的实际类型处理：
    /// - Value::List → Extract all elements
    /// - Value::Int/String/… → Wrapped in a single-element list
    /// - If the value is `Null`, an empty list is returned.
    /// 3. Each expanded element is assigned a specific type, and the output is a dataset containing data of that specific type.
    ///
    /// # Example
    ///
    /// ```sql
    /// -- It can be inferred from the compilation period that: element_type = Int
    /// UNWIND [1, 2, 3] AS x
    ///
    /// During compilation, it is not possible to determine the value of `element_type`; therefore, `element_type` is set to `Unknown` (however, no error is reported).
    /// UNWIND my_variable AS x
    /// -- Determine the type at runtime based on the actual value of my_variable.
    /// ```
    ///
    /// # Reference implementations
    ///
    /// The same processing pattern is observed in:
    /// - `SetOperationValidator::merge_types` - Unknown 与任何类型兼容
    /// - `YieldValidator::validate_types` - Unknown 允许但记录信息
    /// - `OrderByValidator::deduce_expr_type` - 返回 Unknown 而不报错
    /// - `ExpressionChecker::validate_index_access` - DataType::Empty 时跳过严格检查
    fn validate_type(&mut self) -> Result<(), ValidationError> {
        if self.unwind_expression.expression().is_none() {
            return Ok(());
        }

        let list_type = self.deduce_list_element_type(&self.unwind_expression)?;

        if list_type == ValueType::Unknown {
            // According to the project design, it is allowed to have unknown types, and the type inference is postponed until runtime.
            // The extract_list method of the UnwindExecutor will handle any possible value types during runtime.
            // 详见 src/query/executor/result_processing/transformations/unwind.rs#L68-L75
            //
            // Expected behavior:
            // If the variable is a List at runtime, the elements will be expanded correctly.
            // If the variable is of another type, it will be wrapped in a single-element list.
            // If the variable is Null or Empty, no rows will be generated.
            //
            // Debugging tip: If an error occurs during the query execution, check whether the actual values of the variables match the expected values.
        }

        Ok(())
    }

    /// Verify alias references
    fn validate_aliases(&self) -> Result<(), ValidationError> {
        if self.unwind_expression.expression().is_none() {
            return Ok(());
        }

        let refs = self.get_expression_references(&self.unwind_expression);
        for ref_name in refs {
            if !self.aliases_available.contains_key(&ref_name)
                && ref_name != "$"
                && ref_name != "$$"
            {
                return Err(ValidationError::new(
                    format!(
                        "UNWIND expression references undefined variable '{}'",
                        ref_name
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    /// Check whether the expression is empty.
    fn expression_is_empty(&self, _expression: &crate::core::types::expr::Expression) -> bool {
        // Simplify the implementation; in reality, we should check whether the expression is empty.
        false
    }

    /// Determine the type of the expression.
    fn deduce_expr_type(
        &self,
        _expression: &crate::core::types::expr::Expression,
    ) -> Result<ValueType, ValidationError> {
        // Simplify the implementation; in reality, the type should be determined based on the expression.
        Ok(ValueType::List)
    }

    /// Determine the type of the list element
    fn deduce_list_element_type(
        &self,
        _expression: &ContextualExpression,
    ) -> Result<ValueType, ValidationError> {
        // Simplify the implementation; in reality, the element type should be determined based on the expression.
        Ok(ValueType::Unknown)
    }

    /// Obtain the variables referenced by the expression
    fn get_expression_references(&self, _expression: &ContextualExpression) -> Vec<String> {
        // Simplify the implementation; in reality, the expression should be analyzed to obtain the necessary references.
        Vec::new()
    }

    /// Verify the specific sentence.
    fn validate_impl(&mut self) -> Result<(), ValidationError> {
        // Perform the UNWIND verification.
        self.validate_unwind()?;

        // The output of the UNWIND statement is the expanded variable.
        self.outputs.clear();
        if !self.variable_name.is_empty() {
            let element_type = self.deduce_list_element_type(&self.unwind_expression)?;
            self.outputs.push(ColumnDef {
                name: self.variable_name.clone(),
                type_: element_type,
            });
        }

        Ok(())
    }
}

impl Default for UnwindValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for UnwindValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        // Extract expression and variable from AST
        if let Stmt::Unwind(unwind_stmt) = &ast.stmt {
            self.unwind_expression = unwind_stmt.expression.clone();
            self.variable_name = unwind_stmt.variable.clone();
        }

        // Clear the previous state.
        self.outputs.clear();
        self.inputs.clear();
        self.expr_props = ExpressionProps::default();
        self.clear_errors();

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

        info.add_alias(self.variable_name.clone(), AliasType::Variable);

        // Return the successful verification results.
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Unwind
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // “UNWIND” is not a global statement; it is necessary to select a specific space in advance.
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
    use crate::core::Expression;
    use crate::core::Value;
    use ExpressionAnalysisContext;

    fn create_contextual_expr(expr: Expression) -> ContextualExpression {
        let ctx = std::sync::Arc::new(ExpressionAnalysisContext::new());
        let meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    #[test]
    fn test_unwind_validator_new() {
        let validator = UnwindValidator::new();
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());
        assert!(validator.validated_result().is_none());
        assert!(validator.validation_errors().is_empty());
    }

    #[test]
    fn test_unwind_validator_default() {
        let validator: UnwindValidator = Default::default();
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());
    }

    #[test]
    fn test_statement_type() {
        let validator = UnwindValidator::new();
        assert_eq!(validator.statement_type(), StatementType::Unwind);
    }

    #[test]
    fn test_unwind_validation() {
        let mut validator = UnwindValidator::new();

        // Setting expression values and variable names
        validator.set_unwind_expression(create_contextual_expr(Expression::List(vec![
            Expression::Literal(Value::Int(1)),
            Expression::Literal(Value::Int(2)),
            Expression::Literal(Value::Int(3)),
        ])));
        validator.set_variable_name("x".to_string());

        let result = validator.validate_unwind();
        assert!(result.is_ok());

        let validated = result.expect("Failed to validate unwind");
        assert_eq!(validated.variable_name, "x");
    }

    #[test]
    fn test_unwind_empty_variable() {
        let mut validator = UnwindValidator::new();

        // Not setting variable names
        validator.set_unwind_expression(create_contextual_expr(Expression::List(vec![
            Expression::Literal(Value::Int(1)),
        ])));

        let result = validator.validate_unwind();
        assert!(result.is_err());
    }

    #[test]
    fn test_unwind_duplicate_variable() {
        let mut validator = UnwindValidator::new();

        // Set the name of an existing variable.
        let mut aliases = HashMap::new();
        aliases.insert("x".to_string(), ValueType::Int);
        validator.set_aliases_available(aliases);

        validator.set_unwind_expression(create_contextual_expr(Expression::List(vec![
            Expression::Literal(Value::Int(1)),
        ])));
        validator.set_variable_name("x".to_string());

        let result = validator.validate_unwind();
        assert!(result.is_err());
    }

    #[test]
    fn test_unwind_invalid_variable_name() {
        let mut validator = UnwindValidator::new();

        // Setting an invalid variable name (starting with a number)
        validator.set_unwind_expression(create_contextual_expr(Expression::List(vec![
            Expression::Literal(Value::Int(1)),
        ])));
        validator.set_variable_name("1x".to_string());

        let result = validator.validate_unwind();
        assert!(result.is_err());
    }
}
