//! Type checking tools
//! Responsible for the derivation of expression types, type validation, and type compatibility checks.

use crate::core::DataType;
use crate::core::Expression;
use crate::core::TypeUtils;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::AliasType;
use crate::query::validator::ValueType;
use std::collections::HashMap;

pub trait ExpressionValidationContext {
    fn get_aliases(&self) -> &HashMap<String, AliasType>;
    fn get_variable_types(&self) -> Option<&HashMap<String, DataType>>;
}

pub struct TypeDeduceValidator;

impl Default for TypeDeduceValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeDeduceValidator {
    pub fn new() -> Self {
        Self
    }

    pub fn deduce_type(
        &self,
        expression: &crate::core::types::expr::contextual::ContextualExpression,
    ) -> DataType {
        if let Some(expr) = expression.get_expression() {
            expr.deduce_type()
        } else {
            DataType::Empty
        }
    }
}

pub struct TypeValidator;

impl Default for TypeValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeValidator {
    pub fn new() -> Self {
        Self
    }

    pub fn is_indexable_type(&self, type_def: &DataType) -> bool {
        TypeUtils::is_indexable_type(type_def)
    }

    pub fn get_default_value(&self, type_def: &DataType) -> Option<Expression> {
        TypeUtils::get_default_value(type_def).map(Expression::Literal)
    }

    pub fn can_cast(&self, from: &DataType, to: &DataType) -> bool {
        TypeUtils::can_cast(from, to)
    }

    pub fn type_to_string(&self, type_def: &DataType) -> String {
        TypeUtils::type_to_string(type_def)
    }

    pub fn are_types_compatible(&self, left: &DataType, right: &DataType) -> bool {
        TypeUtils::are_types_compatible(left, right)
    }

    pub fn validate_expression_type<C: ExpressionValidationContext>(
        &self,
        expression: &Expression,
        context: &C,
        expected_type: DataType,
    ) -> Result<(), ValidationError> {
        self.validate_expression_type_full(expression, context, expected_type)
    }

    pub fn validate_expression_type_full<C: ExpressionValidationContext>(
        &self,
        expression: &Expression,
        context: &C,
        expected_type: DataType,
    ) -> Result<(), ValidationError> {
        match expression {
            Expression::Literal(value) => {
                let actual_type = value.get_type();
                if self.are_types_compatible(&actual_type, &expected_type) {
                    Ok(())
                } else {
                    Err(ValidationError::new(
                        format!(
                            "Expression type mismatch: expected {:?} , actual {:?} , expression: {:?}",
                            expected_type, actual_type, expression
                        ),
                        ValidationErrorType::TypeError,
                    ))
                }
            }
            Expression::Binary { op, left, right } => {
                self.validate_binary_expression_type(op, left, right, context, expected_type)
            }
            Expression::Unary { op, operand } => {
                self.validate_unary_expression_type(op, operand, context, expected_type)
            }
            Expression::Function { name, args } => {
                self.validate_function_return_type(name, args, context, expected_type)
            }
            Expression::Aggregate {
                func,
                arg,
                distinct: _,
            } => self.validate_aggregate_return_type(func, arg, context, expected_type),
            Expression::Variable(name) => self.validate_variable_type(name, context, expected_type),
            _ => {
                let actual_type = self.deduce_expr_type(expression);
                let expected_value_type = Self::value_type_def_to_value_type(&expected_type);
                if self.are_types_compatible_enhanced(&actual_type, &expected_value_type) {
                    Ok(())
                } else {
                    Err(ValidationError::new(
                        format!(
                            "Expression type mismatch: expected {:?} , actual {:?}",
                            expected_type, actual_type
                        ),
                        ValidationErrorType::TypeError,
                    ))
                }
            }
        }
    }

    fn validate_binary_expression_type<C: ExpressionValidationContext>(
        &self,
        op: &crate::core::BinaryOperator,
        left: &Expression,
        right: &Expression,
        context: &C,
        expected_type: DataType,
    ) -> Result<(), ValidationError> {
        match op {
            crate::core::BinaryOperator::Equal
            | crate::core::BinaryOperator::NotEqual
            | crate::core::BinaryOperator::LessThan
            | crate::core::BinaryOperator::LessThanOrEqual
            | crate::core::BinaryOperator::GreaterThan
            | crate::core::BinaryOperator::GreaterThanOrEqual => {
                if expected_type == DataType::Bool {
                    let left_type = self.deduce_expression_type_full(left, context);
                    let right_type = self.deduce_expression_type_full(right, context);
                    if self.are_types_compatible(&left_type, &right_type) {
                        Ok(())
                    } else {
                        Err(ValidationError::new(
                            format!(
                                "Comparison operator operand type mismatch: left {:?} , right {:?}",
                                left_type, right_type
                            ),
                            ValidationErrorType::TypeError,
                        ))
                    }
                } else {
                    Err(ValidationError::new(
                        format!("The result of the comparison operator is a boolean value, but the expected type is {:?}", expected_type),
                        ValidationErrorType::TypeError,
                    ))
                }
            }
            crate::core::BinaryOperator::And | crate::core::BinaryOperator::Or => {
                if expected_type == DataType::Bool {
                    self.validate_expression_type_full(left, context, DataType::Bool)?;
                    self.validate_expression_type_full(right, context, DataType::Bool)
                } else {
                    Err(ValidationError::new(
                        format!("The result of the logical operator is a boolean value, but the expected type is {:?}", expected_type),
                        ValidationErrorType::TypeError,
                    ))
                }
            }
            _ => {
                let left_type = self.deduce_expression_type_full(left, context);
                let right_type = self.deduce_expression_type_full(right, context);
                let result_type = self.deduce_binary_expr_type(op, &left_type, &right_type);

                if self.are_types_compatible(&result_type, &expected_type) {
                    Ok(())
                } else {
                    Err(ValidationError::new(
                        format!(
                            "Arithmetic operators with mismatched result types: expected {:?} , actual {:?}",
                            expected_type, result_type
                        ),
                        ValidationErrorType::TypeError,
                    ))
                }
            }
        }
    }

    fn validate_unary_expression_type<C: ExpressionValidationContext>(
        &self,
        op: &crate::core::UnaryOperator,
        operand: &Expression,
        context: &C,
        expected_type: DataType,
    ) -> Result<(), ValidationError> {
        match op {
            crate::core::UnaryOperator::Not => {
                if expected_type == DataType::Bool {
                    self.validate_expression_type_full(operand, context, DataType::Bool)
                } else {
                    Err(ValidationError::new(
                        format!("The result of a logical non is a boolean, but the expected type is {:?}", expected_type),
                        ValidationErrorType::TypeError,
                    ))
                }
            }
            crate::core::UnaryOperator::Minus | crate::core::UnaryOperator::Plus => {
                let operand_type = self.deduce_expression_type_full(operand, context);
                if self.are_types_compatible(&operand_type, &expected_type) {
                    Ok(())
                } else {
                    Err(ValidationError::new(
                        format!(
                            "Unary operator result type mismatch: expected {:?} , actual {:?}",
                            expected_type, operand_type
                        ),
                        ValidationErrorType::TypeError,
                    ))
                }
            }
            _ => Ok(()),
        }
    }

    fn validate_function_return_type<C: ExpressionValidationContext>(
        &self,
        name: &str,
        args: &[Expression],
        context: &C,
        expected_type: DataType,
    ) -> Result<(), ValidationError> {
        let return_type = self.deduce_function_return_type(name, args, context);
        if self.are_types_compatible(&return_type, &expected_type) {
            Ok(())
        } else {
            Err(ValidationError::new(
                format!(
                    "The function {:?} has a return type of {:?} , but the expected type is {:?}",
                    name, return_type, expected_type
                ),
                ValidationErrorType::TypeError,
            ))
        }
    }

    fn validate_aggregate_return_type<C: ExpressionValidationContext>(
        &self,
        func: &crate::core::AggregateFunction,
        arg: &Expression,
        context: &C,
        expected_type: DataType,
    ) -> Result<(), ValidationError> {
        let arg_type = self.deduce_expression_type_full(arg, context);
        let return_type = self.deduce_aggregate_return_type_with_arg(func, &arg_type);
        if self.are_types_compatible(&return_type, &expected_type) {
            Ok(())
        } else {
            Err(ValidationError::new(
                format!(
                    "The aggregate function {:?} has a return type of {:?} , but the expected type is {:?}",
                    func, return_type, expected_type
                ),
                ValidationErrorType::TypeError,
            ))
        }
    }

    fn validate_variable_type<C: ExpressionValidationContext>(
        &self,
        name: &str,
        context: &C,
        expected_type: DataType,
    ) -> Result<(), ValidationError> {
        if let Some(var_types) = context.get_variable_types() {
            if let Some(var_type) = var_types.get(name) {
                if self.are_types_compatible(var_type, &expected_type) {
                    return Ok(());
                } else {
                    return Err(ValidationError::new(
                        format!(
                            "The variable {:?} is of type {:?} , but expects the type to be {:?}",
                            name, var_type, expected_type
                        ),
                        ValidationErrorType::TypeError,
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn deduce_expression_type_full<C: ExpressionValidationContext>(
        &self,
        expression: &Expression,
        context: &C,
    ) -> DataType {
        match expression {
            Expression::Literal(value) => value.get_type(),
            Expression::Variable(name) => {
                if let Some(var_types) = context.get_variable_types() {
                    if let Some(var_type) = var_types.get(name) {
                        return var_type.clone();
                    }
                }
                DataType::Empty
            }
            Expression::Binary { op, left, right } => {
                let left_type = self.deduce_expression_type_full(left, context);
                let right_type = self.deduce_expression_type_full(right, context);
                self.deduce_binary_expr_type(op, &left_type, &right_type)
            }
            Expression::Unary { op, operand } => {
                let operand_type = self.deduce_expression_type_full(operand, context);
                self.deduce_unary_expr_type(op, &operand_type)
            }
            Expression::Function { name, args } => {
                self.deduce_function_return_type(name, args, context)
            }
            Expression::Aggregate {
                func,
                arg,
                distinct: _,
            } => {
                let arg_type = self.deduce_expression_type_full(arg, context);
                self.deduce_aggregate_return_type_with_arg(func, &arg_type)
            }
            _ => DataType::Empty,
        }
    }

    fn deduce_binary_expr_type(
        &self,
        op: &crate::core::BinaryOperator,
        left_type: &DataType,
        right_type: &DataType,
    ) -> DataType {
        match op {
            crate::core::BinaryOperator::Equal
            | crate::core::BinaryOperator::NotEqual
            | crate::core::BinaryOperator::LessThan
            | crate::core::BinaryOperator::LessThanOrEqual
            | crate::core::BinaryOperator::GreaterThan
            | crate::core::BinaryOperator::GreaterThanOrEqual => DataType::Bool,
            crate::core::BinaryOperator::And | crate::core::BinaryOperator::Or => DataType::Bool,
            crate::core::BinaryOperator::Add
            | crate::core::BinaryOperator::Subtract
            | crate::core::BinaryOperator::Multiply
            | crate::core::BinaryOperator::Divide
            | crate::core::BinaryOperator::Modulo
            | crate::core::BinaryOperator::Exponent => {
                self.deduce_arithmetic_expr_type(op, left_type, right_type)
            }
            crate::core::BinaryOperator::StringConcat => DataType::String,
            _ => DataType::Empty,
        }
    }

    fn deduce_arithmetic_expr_type(
        &self,
        _op: &crate::core::BinaryOperator,
        left_type: &DataType,
        right_type: &DataType,
    ) -> DataType {
        let left_is_numeric = matches!(
            left_type,
            DataType::SmallInt
                | DataType::Int
                | DataType::BigInt
                | DataType::Float
                | DataType::Double
        );
        let right_is_numeric = matches!(
            right_type,
            DataType::SmallInt
                | DataType::Int
                | DataType::BigInt
                | DataType::Float
                | DataType::Double
        );

        if !left_is_numeric || !right_is_numeric {
            return DataType::Empty;
        }

        let left_is_float = matches!(left_type, DataType::Float | DataType::Double);
        let right_is_float = matches!(right_type, DataType::Float | DataType::Double);

        if left_is_float || right_is_float {
            DataType::Float
        } else {
            DataType::Int
        }
    }

    fn deduce_unary_expr_type(
        &self,
        op: &crate::core::UnaryOperator,
        operand_type: &DataType,
    ) -> DataType {
        match op {
            crate::core::UnaryOperator::Not => match operand_type {
                DataType::Bool => DataType::Bool,
                DataType::Null | DataType::Empty => DataType::Bool,
                _ => DataType::Empty,
            },
            crate::core::UnaryOperator::Minus | crate::core::UnaryOperator::Plus => {
                let is_numeric = matches!(
                    operand_type,
                    DataType::SmallInt
                        | DataType::Int
                        | DataType::BigInt
                        | DataType::Float
                        | DataType::Double
                );
                if is_numeric || matches!(operand_type, DataType::Null | DataType::Empty) {
                    operand_type.clone()
                } else {
                    DataType::Empty
                }
            }
            crate::core::UnaryOperator::IsNull
            | crate::core::UnaryOperator::IsNotNull
            | crate::core::UnaryOperator::IsEmpty
            | crate::core::UnaryOperator::IsNotEmpty => DataType::Bool,
        }
    }

    fn deduce_function_return_type<C: ExpressionValidationContext>(
        &self,
        name: &str,
        args: &[Expression],
        context: &C,
    ) -> DataType {
        match name.to_lowercase().as_str() {
            "abs" | "length" | "size" => DataType::Int,
            "round" | "floor" | "ceil" => DataType::Int,
            "sqrt" | "pow" | "sin" | "cos" | "tan" => DataType::Float,
            "concat" | "substring" | "trim" | "ltrim" | "rtrim" => DataType::String,
            "upper" | "lower" => DataType::String,
            "type" => DataType::String,
            "id" => DataType::Int,
            "properties" => DataType::Map,
            "labels" => DataType::List,
            "keys" => DataType::List,
            "values" => DataType::List,
            "range" => DataType::List,
            "reverse" => DataType::List,
            "head" | "last" | "tail" => {
                if !args.is_empty() {
                    let arg_type = self.deduce_expression_type_full(&args[0], context);
                    if let DataType::List = arg_type {
                        DataType::Empty
                    } else {
                        DataType::Empty
                    }
                } else {
                    DataType::Empty
                }
            }
            _ => DataType::Empty,
        }
    }

    pub fn deduce_aggregate_return_type(&self, func: &crate::core::AggregateFunction) -> DataType {
        match func {
            crate::core::AggregateFunction::Count(_) => DataType::Int,
            crate::core::AggregateFunction::Sum(_) => DataType::Float,
            crate::core::AggregateFunction::Avg(_) => DataType::Float,
            crate::core::AggregateFunction::Max(_) | crate::core::AggregateFunction::Min(_) => {
                DataType::Empty
            }
            crate::core::AggregateFunction::Collect(_) => DataType::List,
            crate::core::AggregateFunction::CollectSet(_) => DataType::Set,
            crate::core::AggregateFunction::Distinct(_) => DataType::Set,
            crate::core::AggregateFunction::Percentile(_, _) => DataType::Float,
            crate::core::AggregateFunction::Std(_) => DataType::Float,
            crate::core::AggregateFunction::BitAnd(_)
            | crate::core::AggregateFunction::BitOr(_) => DataType::Int,
            crate::core::AggregateFunction::GroupConcat(_, _) => DataType::String,
            crate::core::AggregateFunction::VecSum(_) => DataType::Vector,
            crate::core::AggregateFunction::VecAvg(_) => DataType::Vector,
        }
    }

    pub fn deduce_aggregate_return_type_with_arg(
        &self,
        func: &crate::core::AggregateFunction,
        arg_type: &DataType,
    ) -> DataType {
        match func {
            crate::core::AggregateFunction::Count(_) => DataType::Int,
            crate::core::AggregateFunction::Sum(_) => DataType::Float,
            crate::core::AggregateFunction::Avg(_) => DataType::Float,
            crate::core::AggregateFunction::Max(_) | crate::core::AggregateFunction::Min(_) => {
                arg_type.clone()
            }
            crate::core::AggregateFunction::Collect(_) => DataType::List,
            crate::core::AggregateFunction::CollectSet(_) => DataType::Set,
            crate::core::AggregateFunction::Distinct(_) => DataType::Set,
            crate::core::AggregateFunction::Percentile(_, _) => DataType::Float,
            crate::core::AggregateFunction::Std(_) => DataType::Float,
            crate::core::AggregateFunction::BitAnd(_)
            | crate::core::AggregateFunction::BitOr(_) => DataType::Int,
            crate::core::AggregateFunction::GroupConcat(_, _) => DataType::String,
            crate::core::AggregateFunction::VecSum(_) => DataType::Vector,
            crate::core::AggregateFunction::VecAvg(_) => DataType::Vector,
        }
    }

    fn deduce_expr_type(&self, expression: &Expression) -> DataType {
        expression.deduce_type()
    }

    fn are_types_compatible_enhanced(&self, left: &DataType, right: &ValueType) -> bool {
        match (left, right) {
            (DataType::Empty, _) | (_, ValueType::Empty) => true,
            _ => self.are_types_compatible(left, &right.to_data_type()),
        }
    }

    fn value_type_def_to_value_type(type_def: &DataType) -> ValueType {
        ValueType::from_data_type(type_def)
    }

    pub fn has_aggregate_expression_internal(&self, expression: &Expression) -> bool {
        match expression {
            Expression::Aggregate { .. } => true,
            Expression::Unary { operand, .. } => {
                self.has_aggregate_expression_internal(operand.as_ref())
            }
            Expression::Binary { left, right, .. } => {
                self.has_aggregate_expression_internal(left.as_ref())
                    || self.has_aggregate_expression_internal(right.as_ref())
            }
            Expression::Function { args, .. } => args
                .iter()
                .any(|arg| self.has_aggregate_expression_internal(arg)),
            Expression::List(items) => items
                .iter()
                .any(|item| self.has_aggregate_expression_internal(item)),
            Expression::Map(items) => items
                .iter()
                .any(|(_, value)| self.has_aggregate_expression_internal(value)),
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                test_expr
                    .as_ref()
                    .is_some_and(|expr| self.has_aggregate_expression_internal(expr))
                    || conditions.iter().any(|(cond, val)| {
                        self.has_aggregate_expression_internal(cond)
                            || self.has_aggregate_expression_internal(val)
                    })
                    || default
                        .as_ref()
                        .is_some_and(|d| self.has_aggregate_expression_internal(d))
            }
            _ => false,
        }
    }

    pub fn validate_group_key_type<C: ExpressionValidationContext>(
        &self,
        expression: &Expression,
        context: &C,
    ) -> Result<(), ValidationError> {
        self.validate_group_key_type_internal(expression, context)
    }

    fn validate_group_key_type_internal<C: ExpressionValidationContext>(
        &self,
        expression: &Expression,
        _context: &C,
    ) -> Result<(), ValidationError> {
        match expression {
            Expression::Literal(_) | Expression::Variable(_) | Expression::Property { .. } => {
                Ok(())
            }
            Expression::Binary { left, right, .. } => {
                self.validate_group_key_type_internal(left.as_ref(), _context)?;
                self.validate_group_key_type_internal(right.as_ref(), _context)
            }
            Expression::Unary { operand, .. } => {
                self.validate_group_key_type_internal(operand.as_ref(), _context)
            }
            _ => Err(ValidationError::new(
                "The GROUP BY key must be a valid expression".to_string(),
                ValidationErrorType::SemanticError,
            )),
        }
    }
}

pub fn deduce_expression_type(expression: &Expression) -> DataType {
    expression.deduce_type()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::contextual::ContextualExpression;
    use crate::core::types::expr::Expression;
    use crate::core::types::expr::ExpressionMeta;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_deduce_literal_type() {
        let expr = Expression::int(42);
        let validator = TypeDeduceValidator::new();
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);
        let data_type = validator.deduce_type(&ctx_expr);
        assert_eq!(data_type, DataType::Int);
    }

    #[test]
    fn test_deduce_binary_type() {
        let expr = Expression::int(1) + Expression::int(2);
        let validator = TypeDeduceValidator::new();
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);
        let data_type = validator.deduce_type(&ctx_expr);
        assert_eq!(data_type, DataType::Int);
    }

    #[test]
    fn test_deduce_variable_type() {
        let expr = Expression::variable("x");
        let validator = TypeDeduceValidator::new();
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);
        let data_type = validator.deduce_type(&ctx_expr);
        assert_eq!(data_type, DataType::Empty);
    }
}
