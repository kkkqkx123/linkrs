//! Expression checking tool
//! Responsible for verifying the operational validity and structural integrity of expressions.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::DataType;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use std::collections::HashSet;

pub struct ExpressionChecker;

impl Default for ExpressionChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl ExpressionChecker {
    pub fn new() -> Self {
        Self
    }

    pub fn validate_expression_operations(
        &self,
        expression: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        if let Some(expr_meta) = expression.expression() {
            let expr = expr_meta.inner();
            self.check_expression_depth_bfs(expression, 100)?;
            self.validate_expression_operations_recursive(expr, 0)
        } else {
            Err(ValidationError::new(
                "Invalid expression".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    fn validate_expression_operations_recursive(
        &self,
        expression: &crate::core::types::expr::Expression,
        depth: usize,
    ) -> Result<(), ValidationError> {
        if depth > 100 {
            return Err(ValidationError::new(
                "expressions are nested too deeply in levels".to_string(),
                ValidationErrorType::ExpressionDepthError,
            ));
        }

        match expression {
            crate::core::types::expr::Expression::Binary { op, left, right } => {
                self.validate_binary_operation(op, left, right, depth)?;
            }
            crate::core::types::expr::Expression::Unary { op, operand } => {
                self.validate_unary_operation(op, operand, depth)?;
            }
            crate::core::types::expr::Expression::Function { name, args } => {
                self.validate_function_call(name, args, depth)?;
            }
            crate::core::types::expr::Expression::Aggregate {
                func,
                arg,
                distinct,
            } => {
                self.validate_aggregate_operation(func, arg, *distinct, depth)?;
            }
            crate::core::types::expr::Expression::Property {
                object: prop_expression,
                property: name,
            } => {
                self.validate_property_access(prop_expression, name, depth)?;
            }
            crate::core::types::expr::Expression::Subscript {
                collection: index_expression,
                index,
            } => {
                self.validate_index_access(index_expression, index, depth)?;
            }
            crate::core::types::expr::Expression::List(items) => {
                self.validate_list_expression(items, depth)?;
            }
            crate::core::types::expr::Expression::Map(pairs) => {
                self.validate_map_expression(pairs, depth)?;
            }
            crate::core::types::expr::Expression::Case {
                test_expr,
                conditions: when_clauses,
                default: else_clause,
            } => {
                self.validate_case_expression(test_expr, when_clauses, else_clause, depth)?;
            }
            _ => {}
        }

        Ok(())
    }

    fn validate_binary_operation(
        &self,
        op: &crate::core::BinaryOperator,
        left: &crate::core::types::expr::Expression,
        right: &crate::core::types::expr::Expression,
        depth: usize,
    ) -> Result<(), ValidationError> {
        self.validate_expression_operations_recursive(left, depth + 1)?;
        self.validate_expression_operations_recursive(right, depth + 1)?;

        match op {
            crate::core::BinaryOperator::Divide => {
                if let crate::core::types::expr::Expression::Literal(crate::core::Value::Int(0)) =
                    right
                {
                    return Err(ValidationError::new(
                        "The divisor cannot be 0".to_string(),
                        ValidationErrorType::DivisionByZero,
                    ));
                }
                if let crate::core::types::expr::Expression::Literal(crate::core::Value::Float(
                    0.0,
                )) = right
                {
                    return Err(ValidationError::new(
                        "The divisor cannot be 0.0".to_string(),
                        ValidationErrorType::DivisionByZero,
                    ));
                }
            }
            crate::core::BinaryOperator::Modulo => {
                if let crate::core::types::expr::Expression::Literal(crate::core::Value::Int(0)) =
                    right
                {
                    return Err(ValidationError::new(
                        "Modulus cannot be 0".to_string(),
                        ValidationErrorType::DivisionByZero,
                    ));
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn validate_unary_operation(
        &self,
        _op: &crate::core::UnaryOperator,
        operand: &crate::core::types::expr::Expression,
        depth: usize,
    ) -> Result<(), ValidationError> {
        self.validate_expression_operations_recursive(operand, depth + 1)
    }

    fn validate_function_call(
        &self,
        name: &str,
        args: &[crate::core::types::expr::Expression],
        depth: usize,
    ) -> Result<(), ValidationError> {
        if name.is_empty() {
            return Err(ValidationError::new(
                "Function name cannot be null".to_string(),
                ValidationErrorType::SyntaxError,
            ));
        }

        if args.len() > 100 {
            return Err(ValidationError::new(
                format!(
                    "The function {:?} has too many arguments: {}",
                    name,
                    args.len()
                ),
                ValidationErrorType::TooManyArguments,
            ));
        }

        for (i, arg) in args.iter().enumerate() {
            self.validate_expression_operations_recursive(arg, depth + 1)
                .map_err(|e| {
                    ValidationError::new(
                        format!(
                            "Function {:?} Failed to validate the first {} parameter: {}",
                            name,
                            i + 1,
                            e.message
                        ),
                        e.error_type,
                    )
                })?;
        }

        Ok(())
    }

    fn validate_aggregate_operation(
        &self,
        func: &crate::core::AggregateFunction,
        arg: &crate::core::types::expr::Expression,
        distinct: bool,
        depth: usize,
    ) -> Result<(), ValidationError> {
        self.validate_expression_operations_recursive(arg, depth + 1)?;

        let _ = arg.deduce_type();

        if distinct {
            match func {
                crate::core::AggregateFunction::Count(_)
                | crate::core::AggregateFunction::Sum(_)
                | crate::core::AggregateFunction::Avg(_) => {}
                _ => {
                    return Err(ValidationError::new(
                        format!(
                            "Aggregate Functions {} The DISTINCT keyword is not supported.",
                            func.name()
                        ),
                        ValidationErrorType::SyntaxError,
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate_property_access(
        &self,
        expression: &crate::core::types::expr::Expression,
        name: &str,
        depth: usize,
    ) -> Result<(), ValidationError> {
        if name.is_empty() {
            return Err(ValidationError::new(
                "Attribute name cannot be null".to_string(),
                ValidationErrorType::SyntaxError,
            ));
        }

        self.validate_expression_operations_recursive(expression, depth + 1)
    }

    fn validate_index_access(
        &self,
        expression: &crate::core::types::expr::Expression,
        index: &crate::core::types::expr::Expression,
        depth: usize,
    ) -> Result<(), ValidationError> {
        self.validate_expression_operations_recursive(expression, depth + 1)?;
        self.validate_expression_operations_recursive(index, depth + 1)?;

        let expr_type = expression.deduce_type();
        let index_type = index.deduce_type();

        match expr_type {
            DataType::List => {
                if index_type != DataType::Int && index_type != DataType::Empty {
                    return Err(ValidationError::new(
                        format!(
                            "List subscripts need to be of integer type, but get: {:?}",
                            index_type
                        ),
                        ValidationErrorType::TypeError,
                    ));
                }
            }
            DataType::Map => {
                if index_type != DataType::String && index_type != DataType::Empty {
                    return Err(ValidationError::new(
                        format!(
                            "Mapping keys requires a string type, but gets: {:?}",
                            index_type
                        ),
                        ValidationErrorType::TypeError,
                    ));
                }
            }
            DataType::Empty => {}
            _ => {
                return Err(ValidationError::new(
                    format!(
                        "Unsupported types for subscript operations: {:?}",
                        expr_type
                    ),
                    ValidationErrorType::TypeError,
                ));
            }
        }

        Ok(())
    }

    fn validate_list_expression(
        &self,
        items: &[crate::core::types::expr::Expression],
        depth: usize,
    ) -> Result<(), ValidationError> {
        if items.len() > 10000 {
            return Err(ValidationError::new(
                "Too many list expression elements".to_string(),
                ValidationErrorType::TooManyElements,
            ));
        }

        for (i, item) in items.iter().enumerate() {
            self.validate_expression_operations_recursive(item, depth + 1)
                .map_err(|e| {
                    ValidationError::new(
                        format!(
                            "Validation of the {}th element of the list expression failed: {}",
                            i + 1,
                            e.message
                        ),
                        e.error_type,
                    )
                })?;
        }

        Ok(())
    }

    fn validate_map_expression(
        &self,
        pairs: &[(String, crate::core::types::expr::Expression)],
        depth: usize,
    ) -> Result<(), ValidationError> {
        if pairs.len() > 10000 {
            return Err(ValidationError::new(
                "Mapping expressions with too many key-value pairs".to_string(),
                ValidationErrorType::TooManyElements,
            ));
        }

        let mut keys = HashSet::new();
        for (key, _) in pairs {
            if !keys.insert(key) {
                return Err(ValidationError::new(
                    format!(
                        "There are duplicate keys in the mapping expression: {:?}",
                        key
                    ),
                    ValidationErrorType::DuplicateKey,
                ));
            }
        }

        for (key, value) in pairs {
            self.validate_expression_operations_recursive(value, depth + 1)
                .map_err(|e| {
                    ValidationError::new(
                        format!(
                            "The value of the mapping expression key {:?} fails to validate: {}",
                            key, e.message
                        ),
                        e.error_type,
                    )
                })?;
        }

        Ok(())
    }

    fn validate_case_expression(
        &self,
        operand: &Option<Box<crate::core::types::expr::Expression>>,
        when_clauses: &[(
            crate::core::types::expr::Expression,
            crate::core::types::expr::Expression,
        )],
        else_clause: &Option<Box<crate::core::types::expr::Expression>>,
        depth: usize,
    ) -> Result<(), ValidationError> {
        if when_clauses.is_empty() {
            return Err(ValidationError::new(
                "CASE expressions must have at least one WHEN clause.".to_string(),
                ValidationErrorType::SyntaxError,
            ));
        }

        if let Some(op) = operand {
            self.validate_expression_operations_recursive(op, depth + 1)?;
        }

        for (i, (when_expression, then_expression)) in when_clauses.iter().enumerate() {
            self.validate_expression_operations_recursive(when_expression, depth + 1)
                .map_err(|e| {
                    ValidationError::new(
                        format!(
                            "CASE Failed to validate the first {} WHEN clause of the expression: {}",
                            i + 1,
                            e.message
                        ),
                        e.error_type,
                    )
                })?;
            self.validate_expression_operations_recursive(then_expression, depth + 1)
                .map_err(|e| {
                    ValidationError::new(
                        format!(
                            "CASE {}th expression THEN clause validation failed: {}",
                            i + 1,
                            e.message
                        ),
                        e.error_type,
                    )
                })?;
        }

        if let Some(else_expression) = else_clause {
            self.validate_expression_operations_recursive(else_expression, depth + 1)?;
        }

        Ok(())
    }

    pub fn validate_expression_cycles(
        &self,
        expression: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        if let Some(expr_meta) = expression.expression() {
            let expr = expr_meta.inner();
            let mut visited = HashSet::new();
            return self.check_expression_cycles(expr, &mut visited, 0);
        }
        Err(ValidationError::new(
            "Invalid expression".to_string(),
            ValidationErrorType::SemanticError,
        ))
    }

    fn check_expression_cycles(
        &self,
        expression: &crate::core::types::expr::Expression,
        visited: &mut HashSet<String>,
        depth: usize,
    ) -> Result<(), ValidationError> {
        if depth > 100 {
            return Err(ValidationError::new(
                "Expression Cyclic Dependency Detection Depth Overrun".to_string(),
                ValidationErrorType::ExpressionDepthError,
            ));
        }

        match expression {
            crate::core::types::expr::Expression::Variable(name) => {
                if visited.contains(name) {
                    return Err(ValidationError::new(
                        format!("Variable loop dependency detected: {:?}", name),
                        ValidationErrorType::CyclicReference,
                    ));
                }
                visited.insert(name.clone());
            }
            crate::core::types::expr::Expression::Binary { left, right, .. } => {
                self.check_expression_cycles(left, visited, depth + 1)?;
                self.check_expression_cycles(right, visited, depth + 1)?;
            }
            crate::core::types::expr::Expression::Unary { operand, .. } => {
                self.check_expression_cycles(operand, visited, depth + 1)?;
            }
            crate::core::types::expr::Expression::Function { args, .. } => {
                for arg in args {
                    self.check_expression_cycles(arg, visited, depth + 1)?;
                }
            }
            crate::core::types::expr::Expression::Aggregate { arg, .. } => {
                self.check_expression_cycles(arg, visited, depth + 1)?;
            }
            _ => {}
        }

        Ok(())
    }

    pub fn calculate_expression_depth(&self, expression: &ContextualExpression) -> usize {
        if let Some(expr_meta) = expression.expression() {
            let expr = expr_meta.inner();
            return self.calculate_expression_depth_internal(expr);
        }
        0
    }

    fn calculate_expression_depth_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
    ) -> usize {
        match expression {
            crate::core::types::expr::Expression::Literal(_)
            | crate::core::types::expr::Expression::Variable(_) => 1,
            crate::core::types::expr::Expression::Binary { left, right, .. } => {
                let left_depth = self.calculate_expression_depth_internal(left);
                let right_depth = self.calculate_expression_depth_internal(right);
                1 + left_depth.max(right_depth)
            }
            crate::core::types::expr::Expression::Unary { operand, .. } => {
                1 + self.calculate_expression_depth_internal(operand)
            }
            crate::core::types::expr::Expression::Function { args, .. } => {
                let max_arg_depth = args
                    .iter()
                    .map(|arg| self.calculate_expression_depth_internal(arg))
                    .max()
                    .unwrap_or(0);
                1 + max_arg_depth
            }
            crate::core::types::expr::Expression::Aggregate { arg, .. } => {
                1 + self.calculate_expression_depth_internal(arg)
            }
            crate::core::types::expr::Expression::Property {
                object: prop_expression,
                ..
            } => 1 + self.calculate_expression_depth_internal(prop_expression),
            crate::core::types::expr::Expression::Subscript {
                collection: index_expression,
                index,
            } => {
                let expr_depth = self.calculate_expression_depth_internal(index_expression);
                let index_depth = self.calculate_expression_depth_internal(index);
                1 + expr_depth.max(index_depth)
            }
            crate::core::types::expr::Expression::List(items) => {
                let max_item_depth = items
                    .iter()
                    .map(|item| self.calculate_expression_depth_internal(item))
                    .max()
                    .unwrap_or(0);
                1 + max_item_depth
            }
            crate::core::types::expr::Expression::Map(pairs) => {
                let max_value_depth = pairs
                    .iter()
                    .map(|(_, value)| self.calculate_expression_depth_internal(value))
                    .max()
                    .unwrap_or(0);
                1 + max_value_depth
            }
            crate::core::types::expr::Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let mut depths = Vec::new();

                if let Some(test_expression) = test_expr {
                    depths.push(self.calculate_expression_depth_internal(test_expression));
                }

                for (when_expression, then_expression) in conditions {
                    depths.push(self.calculate_expression_depth_internal(when_expression));
                    depths.push(self.calculate_expression_depth_internal(then_expression));
                }

                if let Some(else_expression) = default {
                    depths.push(self.calculate_expression_depth_internal(else_expression));
                }

                let max_depth = depths.into_iter().max().unwrap_or(0);
                1 + max_depth
            }
            _ => 1,
        }
    }

    fn check_expression_depth_bfs(
        &self,
        expression: &ContextualExpression,
        max_depth: usize,
    ) -> Result<(), ValidationError> {
        let depth = self.calculate_expression_depth(expression);
        if depth > max_depth {
            return Err(ValidationError::new(
                format!("Expression depth {} exceeds limit {}", depth, max_depth),
                ValidationErrorType::ExpressionDepthError,
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::Expression;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::types::ContextualExpression;
    use crate::core::Value;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_expression_checker_creation() {
        let _checker = ExpressionChecker::new();
        // If the test is successful and you have reached this point, it means that everything has gone as planned.
    }

    #[test]
    fn test_validate_expression_operations() {
        let checker = ExpressionChecker::new();

        let valid_expression = Expression::Binary {
            op: crate::core::BinaryOperator::Add,
            left: Box::new(Expression::Literal(Value::Int(1))),
            right: Box::new(Expression::Literal(Value::Int(2))),
        };
        let meta = ExpressionMeta::new(valid_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));

        assert!(checker.validate_expression_operations(&ctx_expr).is_ok());
    }

    #[test]
    fn test_validate_division_by_zero() {
        let checker = ExpressionChecker::new();

        let invalid_expression = Expression::Binary {
            op: crate::core::BinaryOperator::Divide,
            left: Box::new(Expression::Literal(Value::Int(1))),
            right: Box::new(Expression::Literal(Value::Int(0))),
        };
        let meta = ExpressionMeta::new(invalid_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));

        assert!(checker.validate_expression_operations(&ctx_expr).is_err());
    }

    #[test]
    fn test_calculate_expression_depth() {
        let checker = ExpressionChecker::new();

        let simple_expression = Expression::Literal(Value::Int(1));
        let meta = ExpressionMeta::new(simple_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        assert_eq!(checker.calculate_expression_depth(&ctx_expr), 1);

        let nested_expression = Expression::Binary {
            op: crate::core::BinaryOperator::Add,
            left: Box::new(Expression::Binary {
                op: crate::core::BinaryOperator::Add,
                left: Box::new(Expression::Literal(Value::Int(1))),
                right: Box::new(Expression::Literal(Value::Int(2))),
            }),
            right: Box::new(Expression::Literal(Value::Int(3))),
        };
        let meta = ExpressionMeta::new(nested_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));

        assert_eq!(checker.calculate_expression_depth(&ctx_expr), 3);
    }
}
