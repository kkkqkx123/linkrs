//! Expression Operation Validator
//! Responsible for verifying the operational legality and structural integrity of expressions.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::DataType;
use crate::core::Expression;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::strategies::helpers::TypeDeduceValidator;
use std::collections::HashSet;

/// Expression Operation Validator
pub struct ExpressionOperationsValidator;

impl Default for ExpressionOperationsValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl ExpressionOperationsValidator {
    pub fn new() -> Self {
        Self
    }

    /// Verify the legitimacy of the expression operation.
    pub fn validate_expression_operations(
        &self,
        expression: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        let expr_meta = match expression.expression() {
            Some(e) => e,
            None => {
                return Err(ValidationError::new(
                    "Invalid expression".to_string(),
                    ValidationErrorType::SemanticError,
                ))
            }
        };
        let expr = expr_meta.inner();

        // Use the BFS (Breadth-First Search) method to check the depth of the expression (to prevent Out of Memory errors).
        self.check_expression_depth_bfs(expression, 100)?;
        self.validate_expression_operations_recursive(expr, 0)
    }

    /// Recursive verification of expression operations
    fn validate_expression_operations_recursive(
        &self,
        expression: &crate::core::types::expr::Expression,
        depth: usize,
    ) -> Result<(), ValidationError> {
        // Check the depth of the expression to prevent stack overflow.
        if depth > 100 {
            return Err(ValidationError::new(
                "expressions are nested too deeply in levels".to_string(),
                ValidationErrorType::ExpressionDepthError,
            ));
        }

        match expression {
            crate::core::types::expr::Expression::Binary { op, left, right } => {
                // Verify the binary operator
                self.validate_binary_operation(op, left, right, depth)?;
            }
            crate::core::types::expr::Expression::Unary { op, operand } => {
                // Verify the unary operator
                self.validate_unary_operation(op, operand, depth)?;
            }
            crate::core::types::expr::Expression::Function { name, args } => {
                // Verify function calls
                self.validate_function_call(name, args, depth)?;
            }
            crate::core::types::expr::Expression::Aggregate {
                func,
                arg,
                distinct,
            } => {
                // Verify the aggregate functions
                self.validate_aggregate_operation(func, arg, *distinct, depth)?;
            }
            crate::core::types::expr::Expression::Property {
                object: prop_expression,
                property: name,
            } => {
                // Verify attribute access
                self.validate_property_access(prop_expression, name, depth)?;
            }
            crate::core::types::expr::Expression::Subscript {
                collection: index_expression,
                index,
            } => {
                // Verify index access
                self.validate_index_access(index_expression, index, depth)?;
            }
            crate::core::types::expr::Expression::List(items) => {
                // Verify the list expression
                self.validate_list_expression(items, depth)?;
            }
            crate::core::types::expr::Expression::Map(pairs) => {
                // Verify the mapping expression
                self.validate_map_expression(pairs, depth)?;
            }
            crate::core::types::expr::Expression::Case {
                test_expr,
                conditions: when_clauses,
                default: else_clause,
            } => {
                // Validation condition expression
                self.validate_case_expression(test_expr, when_clauses, else_clause, depth)?;
            }
            _ => {
                // Other types of expressions do not require any special validation.
            }
        }

        Ok(())
    }

    /// Verify the binary operation
    fn validate_binary_operation(
        &self,
        op: &crate::core::BinaryOperator,
        left: &crate::core::types::expr::Expression,
        right: &crate::core::types::expr::Expression,
        depth: usize,
    ) -> Result<(), ValidationError> {
        // Recursive verification of the left and right operands
        self.validate_expression_operations_recursive(left, depth + 1)?;
        self.validate_expression_operations_recursive(right, depth + 1)?;

        // Verifying the validity of the operator
        match op {
            crate::core::BinaryOperator::Divide => {
                // Division requires special checks: the divisor cannot be the constant 0.
                if let crate::core::types::expr::Expression::Literal(crate::core::Value::Int(0)) =
                    right
                {
                    return Err(ValidationError::new(
                        "The divisor cannot be 0".to_string(),
                        ValidationErrorType::DivisionByZero,
                    ));
                }
                if let Expression::Literal(crate::core::Value::Float(0.0)) = right {
                    return Err(ValidationError::new(
                        "The divisor cannot be 0.0".to_string(),
                        ValidationErrorType::DivisionByZero,
                    ));
                }
            }
            crate::core::BinaryOperator::Modulo => {
                // Modular operations require special checks: the modulus cannot be the constant 0.
                if let Expression::Literal(crate::core::Value::Int(0)) = right {
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

    /// Verify the unary operation
    fn validate_unary_operation(
        &self,
        _op: &crate::core::UnaryOperator,
        operand: &crate::core::types::expr::Expression,
        depth: usize,
    ) -> Result<(), ValidationError> {
        // Recursive validation of operands
        self.validate_expression_operations_recursive(operand, depth + 1)
    }

    /// Verify function calls
    fn validate_function_call(
        &self,
        name: &str,
        args: &[crate::core::types::expr::Expression],
        depth: usize,
    ) -> Result<(), ValidationError> {
        // Verify the format of function names.
        if name.is_empty() {
            return Err(ValidationError::new(
                "Function name cannot be null".to_string(),
                ValidationErrorType::SyntaxError,
            ));
        }

        // Verification of the limit on the number of parameters
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

        // Recursively verify each parameter.
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

    /// Verify the aggregation operation
    fn validate_aggregate_operation(
        &self,
        func: &crate::core::AggregateFunction,
        arg: &crate::core::types::expr::Expression,
        distinct: bool,
        depth: usize,
    ) -> Result<(), ValidationError> {
        // Recursive verification of aggregation parameters
        self.validate_expression_operations_recursive(arg, depth + 1)?;

        // Create a temporary ContextualExpression for type inference.
        let ctx = std::sync::Arc::new(ExpressionAnalysisContext::new());
        let meta = crate::core::types::expr::ExpressionMeta::new(arg.clone());
        let id = ctx.register_expression(meta);
        let contextual_arg = ContextualExpression::new(id, ctx);

        // Use type inference validators to verify the parameter types of aggregate functions.
        let type_validator = TypeDeduceValidator::new();
        let _ = type_validator.deduce_type(&contextual_arg);

        // Verify the DISTINCT flag
        if distinct {
            match func {
                crate::core::AggregateFunction::Count(_)
                | crate::core::AggregateFunction::Sum(_)
                | crate::core::AggregateFunction::Avg(_) => {
                    // These functions support the use of DISTINCT.
                }
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

    /// Verify attribute access
    fn validate_property_access(
        &self,
        expression: &crate::core::types::expr::Expression,
        name: &str,
        depth: usize,
    ) -> Result<(), ValidationError> {
        // Verify the format of the attribute names.
        if name.is_empty() {
            return Err(ValidationError::new(
                "Attribute name cannot be null".to_string(),
                ValidationErrorType::SyntaxError,
            ));
        }

        // Recursive validation of expressions
        self.validate_expression_operations_recursive(expression, depth + 1)
    }

    /// Verify index access
    fn validate_index_access(
        &self,
        expression: &crate::core::types::expr::Expression,
        index: &crate::core::types::expr::Expression,
        depth: usize,
    ) -> Result<(), ValidationError> {
        // Recursive validation of expressions and indices
        self.validate_expression_operations_recursive(expression, depth + 1)?;
        self.validate_expression_operations_recursive(index, depth + 1)?;

        // Create a temporary ContextualExpression for type inference.
        let ctx = std::sync::Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expression.clone());
        let expr_id = ctx.register_expression(expr_meta);
        let contextual_expr = ContextualExpression::new(expr_id, ctx.clone());

        let index_meta = crate::core::types::expr::ExpressionMeta::new(index.clone());
        let index_id = ctx.register_expression(index_meta);
        let contextual_index = ContextualExpression::new(index_id, ctx);

        // Use type inference validators to verify the index type.
        let type_validator = TypeDeduceValidator::new();
        let expr_type = type_validator.deduce_type(&contextual_expr);
        let index_type = type_validator.deduce_type(&contextual_index);

        match expr_type {
            DataType::List => {
                // The list requires integer indices.
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
                // The mapping requires string keys.
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
            DataType::Empty => {
                // Skip validation when the type is unknown.
            }
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

    /// Verify the list expression
    fn validate_list_expression(
        &self,
        items: &[crate::core::types::expr::Expression],
        depth: usize,
    ) -> Result<(), ValidationError> {
        // Verify the list size limit.
        if items.len() > 10000 {
            return Err(ValidationError::new(
                "Too many list expression elements".to_string(),
                ValidationErrorType::TooManyElements,
            ));
        }

        // Recursively verify each element.
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

    /// Verify the mapping expression
    fn validate_map_expression(
        &self,
        pairs: &[(String, crate::core::types::expr::Expression)],
        depth: usize,
    ) -> Result<(), ValidationError> {
        // Verify the limits on the size of the mapping.
        if pairs.len() > 10000 {
            return Err(ValidationError::new(
                "Mapping expressions with too many key-value pairs".to_string(),
                ValidationErrorType::TooManyElements,
            ));
        }

        // Check the uniqueness of the key.
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

        // Recursively verify each value.
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

    /// Validation condition expression
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
        // Verify the number of WHEN clauses
        if when_clauses.is_empty() {
            return Err(ValidationError::new(
                "CASE expressions must have at least one WHEN clause.".to_string(),
                ValidationErrorType::SyntaxError,
            ));
        }

        // Verify the operands (if any exist).
        if let Some(op) = operand {
            self.validate_expression_operations_recursive(op, depth + 1)?;
        }

        // Recursively verify each WHEN clause.
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

        // Verify the ELSE clause (if it exists).
        if let Some(else_expression) = else_clause {
            self.validate_expression_operations_recursive(else_expression, depth + 1)?;
        }

        Ok(())
    }

    /// Verify circular dependencies in expressions
    pub fn validate_expression_cycles(
        &self,
        expression: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        let expr_meta = match expression.expression() {
            Some(e) => e,
            None => {
                return Err(ValidationError::new(
                    "Invalid expression".to_string(),
                    ValidationErrorType::SemanticError,
                ))
            }
        };
        let expr = expr_meta.inner();

        let mut visited = HashSet::new();
        self.check_expression_cycles(expr, &mut visited, 0)
    }

    /// Check for circular dependencies in the expression.
    fn check_expression_cycles(
        &self,
        expression: &crate::core::types::expr::Expression,
        visited: &mut HashSet<String>,
        depth: usize,
    ) -> Result<(), ValidationError> {
        // Prevent infinite recursion
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
            Expression::Binary { left, right, .. } => {
                self.check_expression_cycles(left, visited, depth + 1)?;
                self.check_expression_cycles(right, visited, depth + 1)?;
            }
            Expression::Unary { operand, .. } => {
                self.check_expression_cycles(operand, visited, depth + 1)?;
            }
            Expression::Function { args, .. } => {
                for arg in args {
                    self.check_expression_cycles(arg, visited, depth + 1)?;
                }
            }
            Expression::Aggregate { arg, .. } => {
                self.check_expression_cycles(arg, visited, depth + 1)?;
            }
            _ => {}
        }

        Ok(())
    }

    /// Calculating the depth of an expression
    pub fn calculate_expression_depth(&self, expression: &ContextualExpression) -> usize {
        match expression.expression() {
            Some(e) => self.calculate_expression_depth_internal(e.inner()),
            None => 0,
        }
    }

    /// Internal method: Calculating the depth of an expression
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

    /// Check the depth of the expression using the BFS (Breadth-First Search) method.
    ///
    /// Something similar to ExpressionUtils::checkExprDepth from nebula-graph
    /// Use breadth-first traversal to check the depth of the expression and prevent Out of Memory (OOM) errors.
    pub fn check_expression_depth_bfs(
        &self,
        expression: &ContextualExpression,
        max_depth: usize,
    ) -> Result<(), ValidationError> {
        let expr_meta = match expression.expression() {
            Some(e) => e,
            None => {
                return Err(ValidationError::new(
                    "Invalid expression".to_string(),
                    ValidationErrorType::SemanticError,
                ))
            }
        };
        let expr = expr_meta.inner();

        self.check_expression_depth_bfs_internal(expr, max_depth)
    }

    /// Internal method: Checking the depth of an expression using the BFS (Breadth-First Search) approach
    fn check_expression_depth_bfs_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
        max_depth: usize,
    ) -> Result<(), ValidationError> {
        use std::collections::VecDeque;

        let mut queue = VecDeque::new();
        queue.push_back((expression, 0usize));

        while let Some((expr, depth)) = queue.pop_front() {
            if depth > max_depth {
                return Err(ValidationError::new(
                    format!(
                        "Expression nesting level is too deep, the maximum allowed depth is: {}",
                        max_depth
                    ),
                    ValidationErrorType::ExpressionDepthError,
                ));
            }

            for child in expr.children() {
                queue.push_back((child, depth + 1));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::{ContextualExpression, ExpressionMeta};
    use crate::core::{Expression, Value};
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    /// Create a ContextualExpression from an Expression.
    fn create_contextual_expression(expr: Expression) -> ContextualExpression {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        ContextualExpression::new(id, expr_ctx)
    }

    #[test]
    fn test_expression_operations_validator_creation() {
        let _validator = ExpressionOperationsValidator::new();
        // If the test is successful and you have reached this point, it means that everything has gone as planned.
    }

    #[test]
    fn test_validate_expression_operations() {
        let validator = ExpressionOperationsValidator::new();

        // Simple literal expressions
        let literal_expression = create_contextual_expression(Expression::Literal(Value::Int(42)));
        assert!(validator
            .validate_expression_operations(&literal_expression)
            .is_ok());

        // Simple binary expressions
        let binary_expression = create_contextual_expression(Expression::Binary {
            op: crate::core::BinaryOperator::Add,
            left: Box::new(Expression::Literal(Value::Int(1))),
            right: Box::new(Expression::Literal(Value::Int(2))),
        });
        assert!(validator
            .validate_expression_operations(&binary_expression)
            .is_ok());

        // Zero Detection
        let divide_by_zero = create_contextual_expression(Expression::Binary {
            op: crate::core::BinaryOperator::Divide,
            left: Box::new(Expression::Literal(Value::Int(10))),
            right: Box::new(Expression::Literal(Value::Int(0))),
        });
        assert!(validator
            .validate_expression_operations(&divide_by_zero)
            .is_err());
    }

    #[test]
    fn test_validate_function_call() {
        let validator = ExpressionOperationsValidator::new();

        // Valid function call
        let valid_function = create_contextual_expression(Expression::Function {
            name: "length".to_string(),
            args: vec![Expression::Literal(Value::String("test".to_string()))],
        });
        assert!(validator
            .validate_expression_operations(&valid_function)
            .is_ok());

        // Empty function name
        let empty_function_name = create_contextual_expression(Expression::Function {
            name: "".to_string(),
            args: vec![Expression::Literal(Value::Int(1))],
        });
        assert!(validator
            .validate_expression_operations(&empty_function_name)
            .is_err());
    }

    #[test]
    fn test_calculate_expression_depth() {
        let validator = ExpressionOperationsValidator::new();

        // Simple expressions
        let literal_expression = create_contextual_expression(Expression::Literal(Value::Int(42)));
        assert_eq!(validator.calculate_expression_depth(&literal_expression), 1);

        // Nested expressions
        let nested_expression = create_contextual_expression(Expression::Binary {
            op: crate::core::BinaryOperator::Add,
            left: Box::new(Expression::Literal(Value::Int(1))),
            right: Box::new(Expression::Binary {
                op: crate::core::BinaryOperator::Multiply,
                left: Box::new(Expression::Literal(Value::Int(2))),
                right: Box::new(Expression::Literal(Value::Int(3))),
            }),
        });
        assert_eq!(validator.calculate_expression_depth(&nested_expression), 3);
    }
}
