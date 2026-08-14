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
                    // If graph storage is available, use storage-backed execution
                    if let Some(storage) = context.get_graph_storage() {
                        owned_func.execute_with_storage(&arg_values, &storage)
                    } else {
                        owned_func.execute(&arg_values)
                    }
                } else {
                    // If it is not available in the context, use the global registry.
                    // Check if graph storage is available
                    if let Some(storage) = context.get_graph_storage() {
                        global_registry().execute_with_storage(name, &arg_values, &storage)
                    } else {
                        global_registry().execute(name, &arg_values)
                    }
                }
            }

            // Aggregate functions – Direct evaluation
            Expression::Aggregate {
                func,
                args,
                distinct,
                filter,
            } => {
                let arg_values: Vec<Value> = args
                    .iter()
                    .map(|a| Self::evaluate_recursive(a, context))
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(filter_expr) = &filter {
                    let filter_result = Self::evaluate_recursive(filter_expr, context)?;
                    let is_true = matches!(filter_result, Value::Bool(true));
                    if !is_true {
                        return Ok(Value::Null(crate::core::NullType::Null));
                    }
                }
                FunctionEvaluator::eval_aggregate_function(func, &arg_values, *distinct)
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
            Expression::WindowFunction { .. } => Err(ExpressionError::type_error(
                "Window functions require a runtime window context",
            )),

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

            // Attribute access — fast path: when the object is a simple
            // Variable, try `var.prop` as a direct column lookup before
            // falling back to Vertex/Map extraction.
            Expression::Property { object, property } => {
                if let Expression::Variable(var_name) = object.as_ref() {
                    let compound = format!("{}.{}", var_name, property);
                    if let Some(val) = context.get_variable(&compound) {
                        return Ok(val);
                    }
                }
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
                let compound = format!("{}.{}", edge_name, property);
                if let Some(val) = context.get_variable(&compound) {
                    return Ok(val);
                }
                let edge_value = context
                    .get_variable(edge_name)
                    .ok_or_else(|| ExpressionError::undefined_variable(edge_name))?;
                CollectionOperationEvaluator::eval_property_access(&edge_value, property)
            }

            // Expressions that may require runtime context – delegated to the
            // context, which either resolves them against the row binding or
            // reports a precise per-expression error.
            Expression::Label(name) => context.evaluate_label(name),
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => context.evaluate_list_comprehension(
                variable,
                source,
                filter.as_deref(),
                map.as_deref(),
            ),
            Expression::LabelTagProperty { tag, property } => {
                context.evaluate_label_tag_property(tag, property)
            }
            Expression::TagProperty { tag_name, property } => {
                let compound = format!("{}.{}", tag_name, property);
                if let Some(val) = context.get_variable(&compound) {
                    return Ok(val);
                }
                let tag_value = context
                    .get_variable(tag_name)
                    .ok_or_else(|| ExpressionError::undefined_variable(tag_name))?;
                CollectionOperationEvaluator::eval_property_access(&tag_value, property)
            }
            Expression::Predicate { func, args } => context.evaluate_predicate(func, args),
            Expression::Reduce {
                accumulator,
                initial,
                variable,
                source,
                mapping,
            } => context.evaluate_reduce(accumulator, initial, variable, source, mapping),
            Expression::PathBuild(items) => context.evaluate_path_build(items),
            Expression::Parameter(name) => context
                .get_parameter(name)
                .ok_or_else(|| ExpressionError::undefined_parameter(name)),
            Expression::SessionVariable(name) => context.get_session_variable(name),
            Expression::Exists { body } => {
                let exists = context.execute_exists(body)?;
                Ok(Value::Bool(exists))
            }
            Expression::In {
                expr,
                subquery,
                negated,
            } => {
                let value = Self::evaluate_recursive(expr, context)?;
                let found = matches!(
                    context.contains_subquery(subquery, &value)?,
                    Value::Bool(true)
                );
                Ok(Value::Bool(if *negated { !found } else { found }))
            }
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
            DataType::SmallInt => match value.to_int32() {
                Value::Int(i) => Value::SmallInt(i as i16),
                v => v,
            },
            DataType::Int => value.to_int(),
            DataType::BigInt => {
                let int_val = value.to_int();
                match int_val {
                    Value::Int(i) => Value::BigInt(i as i64),
                    Value::Null(_) => Value::Null(NullType::Null),
                    _ => Value::Null(NullType::BadData),
                }
            }
            DataType::Float => value.to_float(),
            DataType::Double => {
                let float_val = value.to_float();
                match float_val {
                    Value::Float(f) => Value::Double(f as f64),
                    Value::Null(_) => Value::Null(NullType::Null),
                    _ => Value::Null(NullType::BadData),
                }
            }
            DataType::String => {
                return value
                    .to_string()
                    .map(Value::string)
                    .map_err(ExpressionError::type_error);
            }
            DataType::List => value.to_list(),
            DataType::Map => value.to_map(),
            DataType::Json => match value {
                Value::String(s) => {
                    let j = crate::core::value::json::Json::parse(s)
                        .map_err(|e| ExpressionError::type_error(format!("Invalid JSON: {}", e)))?;
                    Value::Json(Box::new(j))
                }
                Value::Json(_) => value.clone(),
                Value::JsonB(jb) => {
                    let j = jb.to_json();
                    Value::Json(Box::new(j))
                }
                Value::Null(_) => Value::Null(NullType::Null),
                _ => {
                    return Err(ExpressionError::type_error(format!(
                        "Cannot convert {:?} to JSON",
                        value.get_type()
                    )))
                }
            },
            DataType::JsonB => match value {
                Value::String(s) => {
                    let jb = crate::core::value::json::JsonB::parse(s)
                        .map_err(|e| ExpressionError::type_error(format!("Invalid JSON: {}", e)))?;
                    Value::JsonB(Box::new(jb))
                }
                Value::JsonB(_) => value.clone(),
                Value::Json(j) => {
                    let jb = j
                        .to_jsonb()
                        .map_err(|e| ExpressionError::type_error(format!("Invalid JSON: {}", e)))?;
                    Value::JsonB(Box::new(jb))
                }
                Value::Null(_) => Value::Null(NullType::Null),
                _ => {
                    return Err(ExpressionError::type_error(format!(
                        "Cannot convert {:?} to JSONB",
                        value.get_type()
                    )))
                }
            },
            _ => {
                return Err(ExpressionError::type_error(format!(
                    "Unsupported type conversion: {:?}",
                    target_type
                )))
            }
        };

        // Check if conversion result is Null(BadData)
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
