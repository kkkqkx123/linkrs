//! ORDER BY clause validator
//! Verify the sorting expression and direction of the ORDER BY clause.
//!
//! This document has been restructured in accordance with the new trait + enumeration validator framework.
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. All the original functions have been retained.
//! Sequence verification
//! Type checking (comparable types)
//! Input column compatibility verification
//! Type inference of expressions
//! Collection of expression references
//! 3. Use QueryContext to manage the context in a unified manner.
//! 4. Added schema validation support for property references.

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::OrderDirection;
use crate::query::parser::ast::stmt::Ast;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::helpers::schema_validator::SchemaValidator;
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::collections::HashMap;
use std::sync::Arc;

/// Sorting column definition
#[derive(Debug, Clone)]
pub struct OrderColumn {
    pub expression: ContextualExpression,
    pub alias: Option<String>,
    pub direction: OrderDirection,
}

/// ORDER BY Validator – New implementation of the system
///
/// Functionality integrity assurance:
/// 1. Complete validation lifecycle
/// 2. Management of input/output columns
/// 3. Expression property tracing
/// 4. Validation of sorting expressions
/// 5. Type compatibility check
/// 6. Schema validation for property references
#[derive(Debug)]
pub struct OrderByValidator {
    // List of sorted columns
    order_columns: Vec<OrderColumn>,
    // Input column definitions (from the previous query)
    input_columns: HashMap<String, ValueType>,
    // Input column definitions (for the trait interface)
    inputs: Vec<ColumnDef>,
    // Column definition for the ORDER BY clause (ORDER BY does not change the output structure)
    outputs: Vec<ColumnDef>,
    // Expression properties
    expr_props: ExpressionProps,
    // User-defined variables
    user_defined_vars: Vec<String>,
    // List of validation errors
    validation_errors: Vec<ValidationError>,
    // Schema validator for property validation
    schema_validator: Option<SchemaValidator>,
    // Space name for schema lookup
    space_name: Option<String>,
    // Available variables and their types (variable_name -> tag_name/edge_type)
    available_vars: HashMap<String, String>,
}

impl OrderByValidator {
    /// Create a new instance of the validator.
    pub fn new() -> Self {
        Self {
            order_columns: Vec::new(),
            input_columns: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validation_errors: Vec::new(),
            schema_validator: None,
            space_name: None,
            available_vars: HashMap::new(),
        }
    }

    /// Create a new instance with schema manager
    pub fn with_schema_manager(schema_manager: Arc<SchemaManager>) -> Self {
        Self {
            order_columns: Vec::new(),
            input_columns: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validation_errors: Vec::new(),
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

    /// Add a sorting column
    pub fn add_order_column(&mut self, col: OrderColumn) {
        self.order_columns.push(col);
    }

    /// Set the input columns
    pub fn set_input_columns(&mut self, columns: HashMap<String, ValueType>) {
        self.input_columns = columns;
        // Synchronize with the inputs.
        self.inputs = self
            .input_columns
            .iter()
            .map(|(name, type_)| ColumnDef {
                name: name.clone(),
                type_: type_.clone(),
            })
            .collect();
    }

    /// Obtain a list of sorted columns
    pub fn order_columns(&self) -> &[OrderColumn] {
        &self.order_columns
    }

    /// Retrieve the input column
    pub fn input_columns(&self) -> &HashMap<String, ValueType> {
        &self.input_columns
    }

    /// Clear the verification errors.
    fn clear_errors(&mut self) {
        self.validation_errors.clear();
    }

    /// Perform validation (in the traditional manner, while maintaining backward compatibility).
    pub fn validate_order_by(&mut self) -> Result<(), ValidationError> {
        self.clear_errors();
        self.validate_impl()?;
        Ok(())
    }

    fn validate_impl(&mut self) -> Result<(), ValidationError> {
        self.validate_columns()?;
        self.validate_types()?;
        self.validate_input_compatibility()?;
        Ok(())
    }

    fn validate_columns(&mut self) -> Result<(), ValidationError> {
        if self.order_columns.is_empty() {
            return Err(ValidationError::new(
                "ORDER BY clause must have at least one column".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        for col in &self.order_columns {
            if self.expression_is_empty(&col.expression) {
                return Err(ValidationError::new(
                    "ORDER BY expression cannot be empty".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    fn validate_types(&self) -> Result<(), ValidationError> {
        for col in &self.order_columns {
            let expr_type = self.deduce_expr_type(&col.expression)?;
            if !self.is_comparable_type(&expr_type) {
                return Err(ValidationError::new(
                    format!("ORDER BY expression type {:?} is not comparable", expr_type),
                    ValidationErrorType::TypeError,
                ));
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

    fn validate_input_compatibility(&self) -> Result<(), ValidationError> {
        for col in &self.order_columns {
            if let Some(alias) = &col.alias {
                if !self.input_columns.contains_key(alias) {
                    return Err(ValidationError::new(
                        format!("ORDER BY alias '{}' not found in input columns", alias),
                        ValidationErrorType::SemanticError,
                    ));
                }
            } else {
                let refs = self.get_expression_references(&col.expression);
                for ref_name in refs {
                    if !self.input_columns.contains_key(&ref_name) && ref_name != "$" {
                        return Err(ValidationError::new(
                            format!(
                                "ORDER BY expression references unknown column '{}'",
                                ref_name
                            ),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn expression_is_empty(&self, expression: &ContextualExpression) -> bool {
        if let Some(e) = expression.get_expression() {
            self.expression_is_empty_internal(&e)
        } else {
            true
        }
    }

    /// Internal method: Check whether the expression is empty.
    fn expression_is_empty_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
    ) -> bool {
        use crate::core::types::expr::Expression;

        match expression {
            Expression::Literal(value) => match value {
                crate::core::Value::Null(_) => true,
                crate::core::Value::String(s) => s.is_empty(),
                _ => false,
            },
            Expression::Variable(name) => name.is_empty(),
            Expression::Function { name, args } => name.is_empty() && args.is_empty(),
            Expression::Binary { left, right, .. } => {
                self.expression_is_empty_internal(left) && self.expression_is_empty_internal(right)
            }
            Expression::Unary { operand, .. } => self.expression_is_empty_internal(operand),
            Expression::List(items) => items.is_empty(),
            Expression::Map(pairs) => pairs.is_empty(),
            Expression::ListComprehension { .. } => false,
            Expression::TagProperty { .. } => false,
            Expression::EdgeProperty { .. } => false,
            Expression::LabelTagProperty { .. } => false,
            Expression::Predicate { .. } => false,
            Expression::Reduce { .. } => false,
            Expression::PathBuild(_) => false,
            // Other types of expressions are not empty by default.
            _ => false,
        }
    }

    fn deduce_expr_type(
        &self,
        expression: &ContextualExpression,
    ) -> Result<ValueType, ValidationError> {
        if let Some(e) = expression.get_expression() {
            self.deduce_expr_type_internal(&e)
        } else {
            Ok(ValueType::Unknown)
        }
    }

    /// Internal method: Deriving the type of an expression
    fn deduce_expr_type_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
    ) -> Result<ValueType, ValidationError> {
        // If schema validator is available, use it for type inference
        if let (Some(ref schema_validator), Some(ref space_name)) =
            (&self.schema_validator, &self.space_name)
        {
            let inferred_type = schema_validator.infer_expression_type(
                expression,
                space_name,
                &self.available_vars,
                &self.input_columns,
            );
            // If we got a known type from schema, return it
            if inferred_type != ValueType::Unknown {
                return Ok(inferred_type);
            }
        }

        // Fallback to basic type inference
        use crate::core::types::expr::Expression;

        match expression {
            Expression::Literal(value) => match value {
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
                crate::core::Value::Null(_) => Ok(ValueType::Null),
                crate::core::Value::Vertex(_) => Ok(ValueType::Vertex),
                crate::core::Value::Edge(_) => Ok(ValueType::Edge),
                crate::core::Value::Path(_) => Ok(ValueType::Path),
                crate::core::Value::List(_) => Ok(ValueType::List),
                crate::core::Value::Map(_) => Ok(ValueType::Map),
                crate::core::Value::Set(_) => Ok(ValueType::Set),
                _ => Ok(ValueType::Unknown),
            },
            Expression::Variable(name) => {
                // Try to obtain the type from the input column.
                if let Some(column_type) = self.input_columns.get(name) {
                    Ok(column_type.clone())
                } else {
                    Ok(ValueType::Unknown) // If the corresponding column cannot be found, an unknown type is returned.
                }
            }
            Expression::Binary { left, op, right } => {
                // For comparison operations, the result is of the boolean type.
                match op {
                    crate::core::BinaryOperator::Equal
                    | crate::core::BinaryOperator::NotEqual
                    | crate::core::BinaryOperator::LessThan
                    | crate::core::BinaryOperator::LessThanOrEqual
                    | crate::core::BinaryOperator::GreaterThan
                    | crate::core::BinaryOperator::GreaterThanOrEqual
                    | crate::core::BinaryOperator::And
                    | crate::core::BinaryOperator::Or
                    | crate::core::BinaryOperator::Xor
                    | crate::core::BinaryOperator::Like
                    | crate::core::BinaryOperator::In
                    | crate::core::BinaryOperator::NotIn
                    | crate::core::BinaryOperator::Contains
                    | crate::core::BinaryOperator::StartsWith
                    | crate::core::BinaryOperator::EndsWith => Ok(ValueType::Bool),
                    // Arithmetic operations usually return values of numeric types.
                    crate::core::BinaryOperator::Add
                    | crate::core::BinaryOperator::Subtract
                    | crate::core::BinaryOperator::Multiply
                    | crate::core::BinaryOperator::Divide
                    | crate::core::BinaryOperator::Modulo
                    | crate::core::BinaryOperator::Exponent => {
                        let left_type = self.deduce_expr_type_internal(left)?;
                        let right_type = self.deduce_expr_type_internal(right)?;

                        // If either of the operands is a floating-point number, the result will also be a floating-point number.
                        if matches!(left_type, ValueType::Float)
                            || matches!(right_type, ValueType::Float)
                        {
                            Ok(ValueType::Float)
                        } else if matches!(left_type, ValueType::Int)
                            || matches!(right_type, ValueType::Int)
                        {
                            Ok(ValueType::Int)
                        } else {
                            Ok(ValueType::Unknown)
                        }
                    }
                    // The string concatenation operation returns a string.
                    crate::core::BinaryOperator::StringConcat => Ok(ValueType::String),
                    // Other operations return unknown types.
                    _ => Ok(ValueType::Unknown),
                }
            }
            Expression::Unary { op, operand } => match op {
                crate::core::UnaryOperator::Not => Ok(ValueType::Bool),
                crate::core::UnaryOperator::IsNull | crate::core::UnaryOperator::IsNotNull => {
                    Ok(ValueType::Bool)
                }
                crate::core::UnaryOperator::IsEmpty | crate::core::UnaryOperator::IsNotEmpty => {
                    Ok(ValueType::Bool)
                }
                crate::core::UnaryOperator::Plus | crate::core::UnaryOperator::Minus => {
                    let operand_type = self.deduce_expr_type_internal(operand)?;
                    Ok(operand_type)
                }
            },
            Expression::Function { name, args: _ } => {
                // Determine the return type based on the function name.
                match name.to_lowercase().as_str() {
                    "id" => Ok(ValueType::String),
                    "count" | "sum" | "avg" | "min" | "max" => Ok(ValueType::Float),
                    "length" | "size" => Ok(ValueType::Int),
                    "to_string" | "string" => Ok(ValueType::String),
                    "abs" => Ok(ValueType::Float),
                    "floor" | "ceil" | "round" => Ok(ValueType::Int),
                    _ => Ok(ValueType::Unknown),
                }
            }
            Expression::Aggregate { func, .. } => match func {
                crate::core::AggregateFunction::Count(_) => Ok(ValueType::Int),
                crate::core::AggregateFunction::Sum(_) => Ok(ValueType::Float),
                crate::core::AggregateFunction::Avg(_) => Ok(ValueType::Float),
                crate::core::AggregateFunction::Collect(_) => Ok(ValueType::List),
                _ => Ok(ValueType::Unknown),
            },
            Expression::List(_) => Ok(ValueType::List),
            Expression::Map(_) => Ok(ValueType::Map),
            Expression::Vector(_) => Ok(ValueType::List), // Vector types are treated as List for type inference
            Expression::Case { .. } => Ok(ValueType::Unknown), // The result type of a CASE expression depends on the branch that is executed.
            Expression::TypeCast { target_type, .. } => {
                // Please provide the text you would like to have translated, as well as the target language you need the translation to. I will then perform the translation for you.
                match target_type {
                    crate::core::DataType::Bool => Ok(ValueType::Bool),
                    crate::core::DataType::SmallInt
                    | crate::core::DataType::Int
                    | crate::core::DataType::BigInt => Ok(ValueType::Int),
                    crate::core::DataType::Float | crate::core::DataType::Double => {
                        Ok(ValueType::Float)
                    }
                    crate::core::DataType::String => Ok(ValueType::String),
                    crate::core::DataType::Date => Ok(ValueType::Date),
                    crate::core::DataType::Time => Ok(ValueType::Time),
                    crate::core::DataType::DateTime => Ok(ValueType::DateTime),
                    _ => Ok(ValueType::Unknown),
                }
            }
            // Unified processing of attribute expressions
            Expression::Property { object, property } => {
                if let Expression::Variable(var_name) = object.as_ref() {
                    if let Some(column_type) = self.input_columns.get(var_name) {
                        return Ok(column_type.clone());
                    }
                }
                if let Some(column_type) = self.input_columns.get(property) {
                    Ok(column_type.clone())
                } else {
                    Ok(ValueType::Unknown)
                }
            }
            Expression::Subscript { .. } => Ok(ValueType::Unknown),
            Expression::Range { .. } => Ok(ValueType::List),
            Expression::Path(_) => Ok(ValueType::Path),
            Expression::Label(_) => Ok(ValueType::String),
            Expression::ListComprehension { .. } => Ok(ValueType::List),
            Expression::LabelTagProperty { .. } => Ok(ValueType::Unknown),
            Expression::TagProperty { .. } => Ok(ValueType::Unknown),
            Expression::EdgeProperty { .. } => Ok(ValueType::Unknown),
            Expression::Predicate { .. } => Ok(ValueType::Bool),
            Expression::Reduce { .. } => Ok(ValueType::Unknown),
            Expression::PathBuild(_) => Ok(ValueType::Path),
            Expression::Parameter(_) => Ok(ValueType::Unknown),
        }
    }

    fn is_comparable_type(&self, type_: &ValueType) -> bool {
        matches!(
            type_,
            ValueType::Bool
                | ValueType::Int
                | ValueType::Float
                | ValueType::String
                | ValueType::Date
                | ValueType::Time
                | ValueType::DateTime
                | ValueType::Null
        )
    }

    fn get_expression_references(&self, expression: &ContextualExpression) -> Vec<String> {
        if let Some(e) = expression.get_expression() {
            let mut refs = Vec::new();
            self.collect_refs_internal(&e, &mut refs);
            refs
        } else {
            Vec::new()
        }
    }

    // Auxiliary function: Recursively collect column references in expressions
    fn collect_refs_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
        refs: &mut Vec<String>,
    ) {
        use crate::core::types::expr::Expression;

        match expression {
            Expression::Variable(name) => {
                if !refs.contains(name) {
                    refs.push(name.clone());
                }
            }
            Expression::Function { args, .. } => {
                for arg in args {
                    self.collect_refs_internal(arg, refs);
                }
            }
            Expression::Binary { left, right, .. } => {
                self.collect_refs_internal(left, refs);
                self.collect_refs_internal(right, refs);
            }
            Expression::Unary { operand, .. } => {
                self.collect_refs_internal(operand, refs);
            }
            Expression::Aggregate { arg, .. } => {
                self.collect_refs_internal(arg, refs);
            }
            Expression::List(items) => {
                for item in items {
                    self.collect_refs_internal(item, refs);
                }
            }
            Expression::Map(pairs) => {
                for (_, value) in pairs {
                    self.collect_refs_internal(value, refs);
                }
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(test_expression) = test_expr {
                    self.collect_refs_internal(test_expression, refs);
                }
                for (condition, value) in conditions {
                    self.collect_refs_internal(condition, refs);
                    self.collect_refs_internal(value, refs);
                }
                if let Some(default_expression) = default {
                    self.collect_refs_internal(default_expression, refs);
                }
            }
            Expression::TypeCast { expression, .. } => {
                self.collect_refs_internal(expression, refs);
            }
            Expression::Subscript { collection, index } => {
                self.collect_refs_internal(collection, refs);
                self.collect_refs_internal(index, refs);
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                self.collect_refs_internal(collection, refs);
                if let Some(start_expression) = start {
                    self.collect_refs_internal(start_expression, refs);
                }
                if let Some(end_expression) = end {
                    self.collect_refs_internal(end_expression, refs);
                }
            }
            // Unified processing of attribute expressions
            Expression::Property { object, property } => {
                self.collect_refs_internal(object, refs);
                if !refs.contains(property) {
                    refs.push(property.clone());
                }
            }
            Expression::Literal(_) => {}
            Expression::Path(_) => {}
            Expression::Label(_) => {}
            Expression::ListComprehension { .. } => {}
            Expression::LabelTagProperty { tag, .. } => {
                self.collect_refs_internal(tag, refs);
            }
            Expression::TagProperty { .. } => {}
            Expression::EdgeProperty { .. } => {}
            Expression::Predicate { args, .. } => {
                for arg in args {
                    self.collect_refs_internal(arg, refs);
                }
            }
            Expression::Reduce {
                initial,
                source,
                mapping,
                ..
            } => {
                self.collect_refs_internal(initial, refs);
                self.collect_refs_internal(source, refs);
                self.collect_refs_internal(mapping, refs);
            }
            Expression::PathBuild(exprs) => {
                for expr in exprs {
                    self.collect_refs_internal(expr, refs);
                }
            }
            Expression::Parameter(_) => {}
            Expression::Vector(_) => {}
        }
    }
}

impl Default for OrderByValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for OrderByValidator {
    fn validate(
        &mut self,
        _ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        self.clear_errors();

        // Get space information from QueryContext
        if let Some(space_name) = qctx.space_name() {
            self.space_name = Some(space_name);
        }

        // Please provide the text you would like to have translated. I will then perform the verification and translate it into English.
        if let Err(e) = self.validate_impl() {
            return Ok(ValidationResult::failure(vec![e]));
        }

        // The `ORDER BY` clause does not change the output structure; the output remains the same as the input.
        self.outputs = self.inputs.clone();

        let mut info = ValidationInfo::new();

        for col in &self.order_columns {
            info.semantic_info
                .ordering_fields
                .push(format!("{:?}", col.expression));
        }

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::OrderBy
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `ORDER BY` statement is not a global statement; therefore, the relevant data space must be selected in advance.
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
    use crate::core::Value;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;

    #[test]
    fn test_order_by_validator_new() {
        let validator = OrderByValidator::new();
        assert!(validator.order_columns().is_empty());
        assert!(validator.input_columns().is_empty());
    }

    #[test]
    fn test_add_order_column() {
        use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use std::sync::Arc;

        let mut validator = OrderByValidator::new();
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Literal(Value::Int(1));
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);
        let col = OrderColumn {
            expression: ctx_expr,
            alias: Some("col1".to_string()),
            direction: OrderDirection::Asc,
        };
        validator.add_order_column(col);
        assert_eq!(validator.order_columns().len(), 1);
    }

    #[test]
    fn test_validate_empty_columns() {
        let mut validator = OrderByValidator::new();
        let result = validator.validate_order_by();
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_valid_column() {
        use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use std::sync::Arc;

        let mut validator = OrderByValidator::new();
        let mut input_cols = HashMap::new();
        input_cols.insert("name".to_string(), ValueType::String);
        validator.set_input_columns(input_cols);

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("name".to_string());
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);
        let col = OrderColumn {
            expression: ctx_expr,
            alias: None,
            direction: OrderDirection::Asc,
        };
        validator.add_order_column(col);

        let result = validator.validate_order_by();
        assert!(result.is_ok());
    }
}
