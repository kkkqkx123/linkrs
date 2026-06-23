//! Implementation of an expression evaluator
//!
//! Provide a function for evaluating specific expressions, implemented using direct recursive matching to avoid unnecessary abstract overhead.

use crate::core::types::expr::analysis_utils::is_evaluable;
use crate::core::types::expr::Expression;
use crate::core::value::list::List;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::evaluator::collection_operations::CollectionOperationEvaluator;
use crate::query::executor::expression::evaluator::functions::FunctionEvaluator;
use crate::query::executor::expression::evaluator::operations::{
    BinaryOperationEvaluator, UnaryOperationEvaluator,
};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::functions::global_registry;
use crate::query::executor::expression::ExpressionError;

/// Implementation of an expression evaluator (unit struct, zero overhead)
#[derive(Debug)]
pub struct ExpressionEvaluator;

impl ExpressionEvaluator {
    /// Evaluate the expression in the given context.
    pub fn evaluate<C: ExpressionContext>(
        expression: &Expression,
        context: &mut C,
    ) -> Result<Value, ExpressionError> {
        Self::evaluate_recursive(expression, context)
    }

    /// Evaluating a list of expressions in batches
    pub fn evaluate_batch<C: ExpressionContext>(
        expressions: &[Expression],
        context: &mut C,
    ) -> Result<Vec<Value>, ExpressionError> {
        expressions
            .iter()
            .map(|expr| Self::evaluate(expr, context))
            .collect()
    }

    /// Check whether the expression can be evaluated.
    ///
    /// Check whether the expression can be evaluated without any runtime context.
    /// In other words, the expression contains only constants and does not include any variables or accesses to attributes.
    pub fn can_evaluate(expression: &Expression) -> bool {
        is_evaluable(expression)
    }

    /// Recursive evaluation expressions
    fn evaluate_recursive<C: ExpressionContext>(
        expression: &Expression,
        context: &mut C,
    ) -> Result<Value, ExpressionError> {
        match expression {
            // Literal values – return the value directly.
            Expression::Literal(value) => Ok(value.clone()),

            // Variable – Obtained from the context
            Expression::Variable(name) => context
                .get_variable(name)
                .ok_or_else(|| ExpressionError::undefined_variable(name)),

            // Binary operations – Recursive evaluation of the left and right operands
            Expression::Binary { left, op, right } => {
                let left_value = Self::evaluate_recursive(left, context)?;
                let right_value = Self::evaluate_recursive(right, context)?;
                BinaryOperationEvaluator::evaluate(&left_value, op, &right_value)
            }

            // One-element operation – Recursive evaluation of the operand
            Expression::Unary { op, operand } => {
                let value = Self::evaluate_recursive(operand, context)?;
                UnaryOperationEvaluator::evaluate(op, &value)
            }

            // Function calls – Parameter evaluation in batch
            Expression::Function { name, args } => {
                let arg_values: Result<Vec<Value>, ExpressionError> = args
                    .iter()
                    .map(|arg| Self::evaluate_recursive(arg, context))
                    .collect();
                let arg_values = arg_values?;

                // First, obtain the function (as an immutable borrowing).
                let func_ref = context.get_function(name);

                if let Some(func_ref) = func_ref {
                    // Convert to a function reference with ownership to avoid borrowing issues.
                    let owned_func: crate::query::executor::expression::functions::OwnedFunctionRef =
                        func_ref.clone();

                    // Explicitly releasing the borrow of func_ref
                    drop(func_ref);

                    // If the context supports caching, use cache-aware execution.
                    if context.supports_cache() {
                        // Retrieve the cache (variable borrowing).
                        if let Some(cache) = context.get_cache() {
                            return owned_func.execute_with_cache(&arg_values, cache);
                        }
                    }
                    // Otherwise, use the normal execution mode.
                    owned_func.execute(&arg_values)
                } else {
                    // If it is not available in the context, use the global registry.
                    global_registry().execute(name, &arg_values)
                }
            }

            // Aggregate functions – Direct evaluation
            Expression::Aggregate {
                func,
                arg,
                distinct,
            } => {
                let arg_value = Self::evaluate_recursive(arg, context)?;
                FunctionEvaluator::eval_aggregate_function(func, &[arg_value], *distinct)
            }

            // CASE expressions – Short-circuit evaluation
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(expr) = test_expr {
                    let test_value = Self::evaluate_recursive(expr, context)?;
                    for (condition, value) in conditions {
                        let condition_result = Self::evaluate_recursive(condition, context)?;
                        if test_value == condition_result {
                            return Self::evaluate_recursive(value, context);
                        }
                    }
                } else {
                    for (condition, value) in conditions {
                        let condition_result = Self::evaluate_recursive(condition, context)?;
                        match condition_result {
                            Value::Bool(true) => return Self::evaluate_recursive(value, context),
                            Value::Bool(false) => continue,
                            _ => {
                                return Err(ExpressionError::type_error(
                                    "CASE conditions must be Boolean",
                                ))
                            }
                        }
                    }
                }
                match default {
                    Some(default_expression) => {
                        Self::evaluate_recursive(default_expression, context)
                    }
                    None => Ok(Value::Null(NullType::Null)),
                }
            }

            // List – Batch evaluation
            Expression::List(elements) => {
                let element_values: Result<Vec<Value>, ExpressionError> = elements
                    .iter()
                    .map(|elem| Self::evaluate_recursive(elem, context))
                    .collect();
                element_values.map(|vals| Value::list(List::from(vals)))
            }

            // Vector literal – Direct evaluation
            Expression::Vector(data) => Ok(Value::vector(data.clone())),

            // Mapping – Batch evaluation
            Expression::Map(entries) => {
                let mut map_values = std::collections::HashMap::new();
                for (key, value_expression) in entries {
                    let value = Self::evaluate_recursive(value_expression, context)?;
                    map_values.insert(key.clone(), value);
                }
                Ok(Value::map(map_values))
            }

            // Subscript access
            Expression::Subscript { collection, index } => {
                let collection_value = Self::evaluate_recursive(collection, context)?;
                let index_value = Self::evaluate_recursive(index, context)?;
                CollectionOperationEvaluator::eval_subscript_access(&collection_value, &index_value)
            }

            // Range access
            Expression::Range {
                collection,
                start,
                end,
            } => {
                let collection_value = Self::evaluate_recursive(collection, context)?;
                let start_value = start
                    .as_ref()
                    .map(|e| Self::evaluate_recursive(e, context))
                    .transpose()?;
                let end_value = end
                    .as_ref()
                    .map(|e| Self::evaluate_recursive(e, context))
                    .transpose()?;
                CollectionOperationEvaluator::eval_range_access(
                    &collection_value,
                    start_value.as_ref(),
                    end_value.as_ref(),
                )
            }

            // Path expression
            Expression::Path(elements) => {
                let element_values: Result<Vec<Value>, ExpressionError> = elements
                    .iter()
                    .map(|elem| Self::evaluate_recursive(elem, context))
                    .collect();
                element_values.map(|vals| Value::list(List::from(vals)))
            }

            // Attribute access
            Expression::Property { object, property } => {
                let object_value = Self::evaluate_recursive(object, context)?;
                CollectionOperationEvaluator::eval_property_access(&object_value, property)
            }

            // Type conversion
            Expression::TypeCast {
                expression,
                target_type,
            } => {
                let value = Self::evaluate_recursive(expression, context)?;
                Self::eval_type_cast(&value, target_type)
            }

            // Edge attribute access - look up edge variable and access property
            Expression::EdgeProperty {
                edge_name,
                property,
            } => {
                // First try to get the edge value from context using edge_name
                let edge_value = context
                    .get_variable(edge_name)
                    .ok_or_else(|| ExpressionError::undefined_variable(edge_name))?;
                // Then access the property on the edge
                CollectionOperationEvaluator::eval_property_access(&edge_value, property)
            }

            // Other types of expressions that require runtime context to be executed
            Expression::Label(_) => {
                Err(ExpressionError::type_error("Unsolved labeled expressions"))
            }
            Expression::ListComprehension { .. } => Err(ExpressionError::type_error(
                "List Derivation Expressions Require Runtime Contexts",
            )),
            Expression::LabelTagProperty { .. } => Err(ExpressionError::type_error(
                "Tagged attribute expressions require runtime context",
            )),
            Expression::TagProperty { tag_name, property } => {
                // Try to get the tag/vertex value from context using tag_name
                let tag_value = context
                    .get_variable(tag_name)
                    .ok_or_else(|| ExpressionError::undefined_variable(tag_name))?;
                // Then access the property on the tag/vertex
                CollectionOperationEvaluator::eval_property_access(&tag_value, property)
            }
            Expression::Predicate { .. } => Err(ExpressionError::type_error(
                "Predicate expressions require a runtime context",
            )),
            Expression::Reduce { .. } => Err(ExpressionError::type_error(
                "Inductive expressions require a runtime context",
            )),
            Expression::PathBuild(_) => Err(ExpressionError::type_error(
                "Path construction expressions require a runtime context",
            )),
            Expression::Parameter(name) => Err(ExpressionError::type_error(format!(
                "The query parameter '{}' requires values provided by the runtime context.",
                name
            ))),
        }
    }

    /// Type conversion for evaluation
    pub fn eval_type_cast(
        value: &Value,
        target_type: &crate::core::types::DataType,
    ) -> Result<Value, ExpressionError> {
        use crate::core::types::DataType;

        let result = match target_type {
            DataType::Bool => value.to_bool(),
            DataType::Int => value.to_int(),
            DataType::Float => value.to_float(),
            DataType::String => {
                return value
                    .to_string()
                    .map(Value::String)
                    .map_err(ExpressionError::type_error);
            }
            DataType::List => value.to_list(),
            DataType::Map => value.to_map(),
            _ => {
                return Err(ExpressionError::type_error(format!(
                    "Unsupported type conversion: {:?}",
                    target_type
                )))
            }
        };

        // 检查转换结果是否为 Null(BadData)
        if let Value::Null(NullType::BadData) = result {
            Err(ExpressionError::type_error(format!(
                "Unable to convert {:?} to {:?}.",
                value, target_type
            )))
        } else {
            Ok(result)
        }
    }
}
