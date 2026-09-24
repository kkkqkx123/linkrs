//! Implementation of an expression evaluator
//!
//! Provide a function for evaluating specific expressions, implemented using direct recursive matching to avoid unnecessary abstract overhead.

use crate::executor::expression::evaluator::collection_operations::CollectionOperationEvaluator;
use crate::executor::expression::evaluator::functions::FunctionEvaluator;
use crate::executor::expression::evaluator::operations::{
    BinaryOperationEvaluator, UnaryOperationEvaluator,
};
use crate::executor::expression::evaluator::traits::ExpressionContext;
use crate::executor::expression::functions::global_registry;
use crate::executor::expression::ExpressionError;
use graphdb_core::types::expr::analysis_utils::is_evaluable;
use graphdb_core::types::expr::Expression;
use graphdb_core::value::list::List;
use graphdb_core::value::NullType;
use graphdb_core::Value;

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
                if let Some(value) = Self::evaluate_higher_order(name, args, context)? {
                    return Ok(value);
                }
                let arg_values: Result<Vec<Value>, ExpressionError> = args
                    .iter()
                    .map(|arg| Self::evaluate_recursive(arg.as_expr(), context))
                    .collect();
                let arg_values = arg_values?;

                // First, obtain the function (as an immutable borrowing).
                let func_ref = context.get_function(name);

                if let Some(func_ref) = func_ref {
                    // Convert to a function reference with ownership to avoid borrowing issues.
                    let owned_func: crate::executor::expression::functions::OwnedFunctionRef =
                        func_ref.clone();

                    // Explicitly releasing the borrow of func_ref
                    drop(func_ref);

                    // Storage-backed execution takes precedence: the cache
                    // path cannot supply graph storage to the builtins that
                    // need it (e.g. startnode/endnode label resolution).
                    if let Some(storage) = context.get_graph_storage() {
                        owned_func.execute_with_storage(&arg_values, &storage)
                    } else if context.supports_cache() {
                        // Retrieve the cache (variable borrowing).
                        match context.get_cache() {
                            Some(cache) => owned_func.execute_with_cache(&arg_values, cache),
                            None => owned_func.execute(&arg_values),
                        }
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
                        return Ok(Value::Null(graphdb_core::NullType::Null));
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
                    map_values.insert(Value::string(key.clone()), value);
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

            // STRUCT field access (e.g. `addr.city`)
            Expression::StructField { base, field } => {
                let base_value = Self::evaluate_recursive(base, context)?;
                CollectionOperationEvaluator::eval_struct_field_access(&base_value, field)
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
            Expression::CountSubquery { body } => {
                let results = context.execute_subquery(body)?;
                Ok(Value::Int(results.len() as i32))
            }
            Expression::ScalarSubquery { body } => {
                // First result value, or NULL when the subquery is empty.
                context.execute_scalar_subquery(body)
            }
            Expression::Lambda { .. } => Err(ExpressionError::type_error(
                "Lambda expression cannot be evaluated directly; \
                 use it as an argument to a higher-order function like list_transform or list_filter",
            )),
        }
    }

    fn evaluate_higher_order<C: ExpressionContext>(
        name: &str,
        args: &[graphdb_core::types::expr::FunctionArg],
        context: &mut C,
    ) -> Result<Option<Value>, ExpressionError> {
        let lower = name.to_ascii_lowercase();
        if !matches!(
            lower.as_str(),
            "list_filter"
                | "list_transform"
                | "list_any"
                | "list_all"
                | "list_single"
                | "list_reduce"
        ) {
            return Ok(None);
        }
        // Only intercept the lambda form. Legacy mask calls such as
        // `list_filter(list, mask)` carry no Lambda and must fall through
        // to the normal eager path so context-local functions, storage
        // and named-argument handling stay intact.
        let lambda_pos = args
            .iter()
            .position(|a| matches!(a.as_expr(), Expression::Lambda { .. }));
        let Some(lambda_index) = lambda_pos else {
            return Ok(None);
        };
        // Unified signature places the lambda at index 1:
        // `list_filter(source, lambda)`, `list_reduce(source, lambda, initial)`.
        // Named arguments still preserve positions, so enforce the position
        // instead of silently accepting permuted orders.
        if lambda_index != 1 {
            return Err(ExpressionError::type_error(format!(
                "{name} expects the lambda as the second argument"
            )));
        }
        let Some(source_expr) = args.first().map(|a| a.as_expr()) else {
            return Err(ExpressionError::type_error(format!(
                "{name} requires a list as first argument"
            )));
        };
        let source = Self::evaluate_recursive(source_expr, context)?;
        // NULL source propagates NULL, matching REDUCE and comprehension semantics.
        if matches!(source, Value::Null(_)) {
            return Ok(Some(Value::Null(NullType::Null)));
        }
        let Value::List(list) = source else {
            return Err(ExpressionError::type_error(format!(
                "{name} requires a list as first argument"
            )));
        };
        let Expression::Lambda { params, body } = args[lambda_index].as_expr() else {
            return Ok(None);
        };

        if lower == "list_reduce" {
            if args.len() != 2 && args.len() != 3 {
                return Err(ExpressionError::type_error(
                    "list_reduce requires (source, lambda, initial)".to_string(),
                ));
            }
            if params.len() != 2 {
                return Err(ExpressionError::type_error(
                    "list_reduce lambda requires two parameters (acc, item)".to_string(),
                ));
            }
            let initial = args
                .get(2)
                .map(|a| Self::evaluate_recursive(a.as_expr(), context))
                .transpose()?
                .unwrap_or(Value::Null(NullType::Null));
            let mut acc = initial;
            for item in list.values {
                context.set_variable(params[0].clone(), acc.clone());
                context.set_variable(params[1].clone(), item);
                acc = Self::evaluate_recursive(body, context)?;
            }
            return Ok(Some(acc));
        }

        if args.len() != 2 {
            return Err(ExpressionError::type_error(format!(
                "{name} requires (source, lambda)"
            )));
        }
        if params.is_empty() {
            return Err(ExpressionError::type_error(format!(
                "{name} lambda requires a parameter"
            )));
        }
        let param = params[0].clone();
        match lower.as_str() {
            "list_filter" => {
                let mut kept = Vec::new();
                for item in list.values {
                    context.set_variable(param.clone(), item.clone());
                    match Self::evaluate_recursive(body, context)? {
                        Value::Bool(true) => kept.push(item),
                        Value::Bool(false) | Value::Null(_) => {}
                        other => {
                            return Err(ExpressionError::type_error(format!(
                                "list_filter lambda must return boolean, got {:?}",
                                other.get_type()
                            )))
                        }
                    }
                }
                Ok(Some(Value::list(List::from(kept))))
            }
            "list_transform" => {
                let mut out = Vec::with_capacity(list.values.len());
                for item in list.values {
                    context.set_variable(param.clone(), item);
                    out.push(Self::evaluate_recursive(body, context)?);
                }
                Ok(Some(Value::list(List::from(out))))
            }
            "list_any" | "list_all" | "list_single" => {
                let mut matched = 0usize;
                let total = list.values.len();
                for item in list.values {
                    context.set_variable(param.clone(), item);
                    match Self::evaluate_recursive(body, context)? {
                        Value::Bool(true) => matched += 1,
                        Value::Bool(false) | Value::Null(_) => {}
                        other => {
                            return Err(ExpressionError::type_error(format!(
                                "{name} lambda must return boolean, got {:?}",
                                other.get_type()
                            )))
                        }
                    }
                }
                let result = match lower.as_str() {
                    "list_any" => matched > 0,
                    "list_all" => matched == total,
                    _ => matched == 1,
                };
                Ok(Some(Value::Bool(result)))
            }
            _ => Ok(None),
        }
    }

    /// Type conversion for evaluation
    pub fn eval_type_cast(
        value: &Value,
        target_type: &graphdb_core::types::DataType,
    ) -> Result<Value, ExpressionError> {
        use graphdb_core::types::DataType;

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
            DataType::List(_) => value.to_list(),
            DataType::Map(_) => value.to_map(),
            DataType::Json => match value {
                Value::String(s) => {
                    let j = graphdb_core::value::json::Json::parse(s)
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
                    let jb = graphdb_core::value::json::JsonB::parse(s)
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

#[cfg(test)]
mod higher_order_tests {
    use super::*;
    use crate::executor::expression::evaluation_context::DefaultExpressionContext;
    use graphdb_core::types::expr::FunctionArg;
    use graphdb_core::types::operators::BinaryOperator;

    fn int_list(values: Vec<i32>) -> Expression {
        Expression::List(values.into_iter().map(Expression::int).collect())
    }

    fn lambda_two_body() -> Expression {
        Expression::lambda(
            vec!["acc".to_string(), "x".to_string()],
            Expression::binary(
                Expression::variable("acc"),
                BinaryOperator::Add,
                Expression::variable("x"),
            ),
        )
    }

    fn eval_function(name: &str, args: Vec<FunctionArg>) -> Result<Value, ExpressionError> {
        let expr = Expression::Function {
            name: name.to_string(),
            args,
        };
        let mut ctx = DefaultExpressionContext::new();
        ExpressionEvaluator::evaluate(&expr, &mut ctx)
    }

    #[test]
    fn list_reduce_sums_ints() {
        let result = eval_function(
            "list_reduce",
            vec![
                FunctionArg::positional(int_list(vec![1, 2, 3])),
                FunctionArg::positional(lambda_two_body()),
                FunctionArg::positional(Expression::int(0)),
            ],
        )
        .expect("reduce ints");
        assert_eq!(result, Value::Int(6));
    }

    #[test]
    fn list_reduce_concatenates_strings() {
        let source = Expression::List(vec![Expression::string("a"), Expression::string("b")]);
        let result = eval_function(
            "list_reduce",
            vec![
                FunctionArg::positional(source),
                FunctionArg::positional(lambda_two_body()),
                FunctionArg::positional(Expression::string("")),
            ],
        )
        .expect("reduce strings");
        assert_eq!(result, Value::string("ab"));
    }

    #[test]
    fn list_reduce_adds_floats() {
        let source = Expression::List(vec![Expression::int(1), Expression::int(2)]);
        let result = eval_function(
            "list_reduce",
            vec![
                FunctionArg::positional(source),
                FunctionArg::positional(lambda_two_body()),
                FunctionArg::positional(Expression::float(0.0)),
            ],
        )
        .expect("reduce floats");
        assert_eq!(result, Value::Float(3.0));
    }

    #[test]
    fn list_reduce_empty_returns_initial() {
        let result = eval_function(
            "list_reduce",
            vec![
                FunctionArg::positional(Expression::List(vec![])),
                FunctionArg::positional(lambda_two_body()),
                FunctionArg::positional(Expression::int(42)),
            ],
        )
        .expect("empty reduce");
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn list_reduce_null_source_returns_null() {
        let result = eval_function(
            "list_reduce",
            vec![
                FunctionArg::positional(Expression::null()),
                FunctionArg::positional(lambda_two_body()),
                FunctionArg::positional(Expression::int(0)),
            ],
        )
        .expect("null reduce");
        assert!(matches!(result, Value::Null(_)));
    }

    #[test]
    fn list_reduce_requires_two_lambda_params() {
        let bad = Expression::lambda(vec!["x".to_string()], Expression::variable("x"));
        let err = eval_function(
            "list_reduce",
            vec![
                FunctionArg::positional(int_list(vec![1])),
                FunctionArg::positional(bad),
                FunctionArg::positional(Expression::int(0)),
            ],
        )
        .expect_err("single-param reduce must fail");
        assert!(err.message.contains("two parameters"), "{}", err.message);
    }

    #[test]
    fn list_filter_applies_lambda_per_element() {
        let lambda = Expression::lambda(
            vec!["x".to_string()],
            Expression::binary(
                Expression::variable("x"),
                BinaryOperator::GreaterThan,
                Expression::int(1),
            ),
        );
        let result = eval_function(
            "list_filter",
            vec![
                FunctionArg::positional(int_list(vec![1, 2, 3])),
                FunctionArg::positional(lambda),
            ],
        )
        .expect("filter");
        assert_eq!(
            result,
            Value::list(List::from(vec![Value::Int(2), Value::Int(3)]))
        );
    }

    #[test]
    fn list_transform_applies_lambda_per_element() {
        let lambda = Expression::lambda(
            vec!["x".to_string()],
            Expression::binary(
                Expression::variable("x"),
                BinaryOperator::Multiply,
                Expression::int(2),
            ),
        );
        let result = eval_function(
            "list_transform",
            vec![
                FunctionArg::positional(int_list(vec![1, 2])),
                FunctionArg::positional(lambda),
            ],
        )
        .expect("transform");
        assert_eq!(
            result,
            Value::list(List::from(vec![Value::Int(2), Value::Int(4)]))
        );
    }

    #[test]
    fn list_any_all_single_use_lambda_truth() {
        let gt_one = || {
            Expression::lambda(
                vec!["x".to_string()],
                Expression::binary(
                    Expression::variable("x"),
                    BinaryOperator::GreaterThan,
                    Expression::int(1),
                ),
            )
        };
        let source = || int_list(vec![1, 2, 3]);
        let any = eval_function(
            "list_any",
            vec![
                FunctionArg::positional(source()),
                FunctionArg::positional(gt_one()),
            ],
        )
        .expect("any");
        assert_eq!(any, Value::Bool(true));
        let all = eval_function(
            "list_all",
            vec![
                FunctionArg::positional(source()),
                FunctionArg::positional(gt_one()),
            ],
        )
        .expect("all");
        assert_eq!(all, Value::Bool(false));
        let gt_two = Expression::lambda(
            vec!["x".to_string()],
            Expression::binary(
                Expression::variable("x"),
                BinaryOperator::GreaterThan,
                Expression::int(2),
            ),
        );
        let single = eval_function(
            "list_single",
            vec![
                FunctionArg::positional(source()),
                FunctionArg::positional(gt_two),
            ],
        )
        .expect("single");
        assert_eq!(single, Value::Bool(true));
    }

    #[test]
    fn list_reduce_supports_smallint_with_wrapping_overflow() {
        let source = Expression::List(vec![
            Expression::Literal(Value::SmallInt(1)),
            Expression::Literal(Value::SmallInt(2)),
        ]);
        let result = eval_function(
            "list_reduce",
            vec![
                FunctionArg::positional(source),
                FunctionArg::positional(lambda_two_body()),
                FunctionArg::positional(Expression::Literal(Value::SmallInt(0))),
            ],
        )
        .expect("reduce smallint");
        assert_eq!(result, Value::SmallInt(3));
        // Overflow wraps rather than erroring, matching integer arithmetic.
        let overflowing = Expression::List(vec![Expression::Literal(Value::Int(i32::MAX))]);
        let wrapped = eval_function(
            "list_reduce",
            vec![
                FunctionArg::positional(overflowing),
                FunctionArg::positional(lambda_two_body()),
                FunctionArg::positional(Expression::int(1)),
            ],
        )
        .expect("wrapping overflow");
        assert_eq!(wrapped, Value::Int(i32::MIN));
    }

    #[test]
    fn list_reduce_supports_decimal() {
        use graphdb_core::value::Decimal128Value;
        let dec = |n: i64| Expression::Literal(Value::Decimal128(Decimal128Value::from_i64(n)));
        let source = Expression::List(vec![dec(1), dec(2)]);
        let result = eval_function(
            "list_reduce",
            vec![
                FunctionArg::positional(source),
                FunctionArg::positional(lambda_two_body()),
                FunctionArg::positional(dec(0)),
            ],
        )
        .expect("reduce decimal");
        assert_eq!(result, Value::Decimal128(Decimal128Value::from_i64(3)));
    }

    #[test]
    fn list_reduce_null_element_surfaces_type_error() {
        // NULL has no addition rule, so the lambda body error propagates
        // instead of being silently skipped.
        let source = Expression::List(vec![Expression::int(1), Expression::null()]);
        let err = eval_function(
            "list_reduce",
            vec![
                FunctionArg::positional(source),
                FunctionArg::positional(lambda_two_body()),
                FunctionArg::positional(Expression::int(0)),
            ],
        )
        .expect_err("null element must error");
        assert!(
            err.message.contains("Cannot perform addition"),
            "{}",
            err.message
        );
    }
}
