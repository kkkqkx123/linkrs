//! The `trait` for defining the context in which expressions are evaluated
//!
//! Provide a unified context interface for evaluating expressions in graph databases.
//!
//! Note: This trait is used for the evaluation of runtime expressions.
//! For compilation-time analysis, please use `ExpressionAnalysisContext`.

use crate::core::types::expr::{Expression, SubqueryBody};
use crate::core::value::list::List;
use crate::core::value::NullType;
use crate::core::vertex_edge_path::{Path, Step};
use crate::core::Value;
use crate::query::executor::expression::evaluation_context::graph_storage::GraphStorageRef;
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::functions::OwnedFunctionRef;
use crate::query::executor::expression::ExpressionError;
use crate::query::executor::streaming::slot::SlotId;

/// The "expression evaluation context trait"
///
/// Provide a unified context interface for evaluating graph database expressions.
///
/// Note: This trait is used for the evaluation of runtime expressions.
/// For compilation-time analysis, please use `ExpressionAnalysisContext`.
pub trait ExpressionContext {
    /// Obtain the value of the variable
    fn get_variable(&self, name: &str) -> Option<Value>;

    /// Obtain the value of a query parameter (`@name`).
    /// Default implementation returns `None` — contexts without parameter
    /// support simply ignore parameter references.
    fn get_parameter(&self, _name: &str) -> Option<Value> {
        None
    }

    /// Obtain the value of a session variable (`$name`).
    ///
    /// Default implementation reports an error: an undefined session
    /// variable is a query error, not NULL. Row contexts backed by a
    /// session snapshot override this to return the actual value.
    fn get_session_variable(&self, name: &str) -> Result<Value, ExpressionError> {
        Err(ExpressionError::type_error(format!(
            "Session variable `{}` is not defined in this context",
            name
        )))
    }

    /// Obtain the value of a variable by slot ID (fast path).
    /// Default implementation falls back to name-based lookup.
    fn get_variable_by_slot(&self, _slot: SlotId) -> Option<Value> {
        None
    }

    /// Setting variable values
    fn set_variable(&mut self, name: String, value: Value);

    /// Obtain a function reference
    fn get_function(&self, name: &str) -> Option<OwnedFunctionRef> {
        let _ = name;
        None
    }

    /// Obtain the graph storage accessor for graph algorithm functions
    fn get_graph_storage(&self) -> Option<GraphStorageRef> {
        None
    }

    /// Check whether the context supports caching.
    fn supports_cache(&self) -> bool {
        false
    }

    /// Obtain the cache manager (if available).
    ///
    /// The caching function has been removed; the result is "None".
    fn get_cache(&mut self) -> Option<&mut ()> {
        None
    }

    /// Execute a subquery body and return the result values.
    ///
    /// Used for EXISTS and IN subquery expressions.
    /// Returns the list of values from the subquery's RETURN clause.
    fn execute_subquery(&mut self, body: &SubqueryBody) -> Result<Vec<Value>, ExpressionError> {
        let _ = body;
        Err(ExpressionError::type_error(
            "Subquery execution not supported in this context",
        ))
    }

    /// EXISTS semantics: whether the subquery produces at least one row.
    ///
    /// Default implementation runs [`Self::execute_subquery`] and tests for
    /// a non-empty result; streaming contexts override it to short-circuit
    /// and to cache non-correlated results.
    fn execute_exists(&mut self, body: &SubqueryBody) -> Result<bool, ExpressionError> {
        let results = self.execute_subquery(body)?;
        Ok(!results.is_empty())
    }

    /// IN semantics: whether `value` occurs in the subquery result.
    ///
    /// A NULL left operand, or NULL values inside the result set, never
    /// match — consistent with the conjunctive `keys_match` path.
    fn contains_subquery(
        &mut self,
        body: &SubqueryBody,
        value: &Value,
    ) -> Result<Value, ExpressionError> {
        if value.is_null() {
            return Ok(Value::Bool(false));
        }
        let results = self.execute_subquery(body)?;
        Ok(Value::Bool(
            results.iter().any(|v| !v.is_null() && v == value),
        ))
    }

    /// Evaluate a `:Label` expression against the current row binding.
    ///
    /// The label expression is a bare tag reference with no bound variable,
    /// so contexts that cannot resolve it report an error naming the label
    /// instead of a generic "require runtime context" message.
    fn evaluate_label(&self, label: &str) -> Result<Value, ExpressionError> {
        Err(ExpressionError::type_error(format!(
            "Label expression `:{}` requires runtime context to resolve",
            label
        )))
    }

    /// Evaluate a list comprehension `[variable IN source WHERE filter | map]`.
    ///
    /// Generic default: iterates the source collection, binds `variable` in
    /// this context, and applies the optional filter and map expressions.
    /// Works with any context that supports `get_variable`/`set_variable`.
    fn evaluate_list_comprehension(
        &mut self,
        variable: &str,
        source: &Expression,
        filter: Option<&Expression>,
        map: Option<&Expression>,
    ) -> Result<Value, ExpressionError>
    where
        Self: Sized,
    {
        let source_value = ExpressionEvaluator::evaluate(source, self)?;
        let elements = match source_value {
            Value::List(list) => list.values,
            Value::Null(_) => return Ok(Value::Null(NullType::Null)),
            other => {
                return Err(ExpressionError::type_error(format!(
                    "List comprehension source must be a list, got {:?}",
                    other.get_type()
                )))
            }
        };
        let mut results = Vec::with_capacity(elements.len());
        for element in elements {
            self.set_variable(variable.to_string(), element);
            if let Some(filter_expr) = filter {
                match ExpressionEvaluator::evaluate(filter_expr, self)? {
                    Value::Bool(true) => {}
                    Value::Bool(false) | Value::Null(_) => continue,
                    other => {
                        return Err(ExpressionError::type_error(format!(
                            "List comprehension filter must evaluate to a boolean, got {:?}",
                            other.get_type()
                        )))
                    }
                }
            }
            let mapped = match map {
                Some(map_expr) => ExpressionEvaluator::evaluate(map_expr, self)?,
                None => self
                    .get_variable(variable)
                    .unwrap_or(Value::Null(NullType::Null)),
            };
            results.push(mapped);
        }
        Ok(Value::list(List::from(results)))
    }

    /// Evaluate a dynamic tag-property access `tag.property` where `tag` is
    /// itself an expression.
    ///
    /// Generic default: evaluates the tag expression and performs property
    /// access on the resulting vertex value.
    fn evaluate_label_tag_property(
        &mut self,
        tag: &Expression,
        property: &str,
    ) -> Result<Value, ExpressionError>
    where
        Self: Sized,
    {
        let tag_value = ExpressionEvaluator::evaluate(tag, self)?;
        crate::query::executor::expression::evaluator::collection_operations::CollectionOperationEvaluator::eval_property_access(
            &tag_value,
            property,
        )
    }

    /// Evaluate a predicate expression `func(variable IN list WHERE predicate)`
    /// with `func` in {ALL, ANY, SINGLE, NONE}.
    ///
    /// Generic default: iterates the collection, binds `variable`, and applies
    /// the predicate with the standard Cypher quantifier semantics.
    fn evaluate_predicate(
        &mut self,
        func: &str,
        args: &[Expression],
    ) -> Result<Value, ExpressionError>
    where
        Self: Sized,
    {
        let func_upper = func.to_uppercase();
        let variable = match args.first() {
            Some(Expression::Variable(name)) => name.clone(),
            _ => {
                return Err(ExpressionError::type_error(format!(
                    "Predicate `{}` requires a variable as its first argument",
                    func
                )))
            }
        };
        let source = args
            .get(1)
            .ok_or_else(|| ExpressionError::argument_count_error(3, args.len()))?;
        let predicate = args
            .get(2)
            .ok_or_else(|| ExpressionError::argument_count_error(3, args.len()))?;

        let source_value = ExpressionEvaluator::evaluate(source, self)?;
        let elements = match source_value {
            Value::List(list) => list.values,
            Value::Null(_) => return Ok(Value::Null(NullType::Null)),
            other => {
                return Err(ExpressionError::type_error(format!(
                    "Predicate `{}` source must be a list, got {:?}",
                    func,
                    other.get_type()
                )))
            }
        };

        let mut matched = 0usize;
        let total = elements.len();
        for element in elements {
            self.set_variable(variable.clone(), element);
            match ExpressionEvaluator::evaluate(predicate, self)? {
                Value::Bool(true) => matched += 1,
                Value::Bool(false) | Value::Null(_) => {}
                other => {
                    return Err(ExpressionError::type_error(format!(
                        "Predicate `{}` condition must evaluate to a boolean, got {:?}",
                        func,
                        other.get_type()
                    )))
                }
            }
        }
        match func_upper.as_str() {
            // ALL: every element satisfies the predicate; vacuously true for
            // an empty collection.
            "ALL" => Ok(Value::Bool(matched == total)),
            // ANY: at least one element satisfies the predicate.
            "ANY" => Ok(Value::Bool(matched > 0)),
            // SINGLE: exactly one element satisfies the predicate.
            "SINGLE" => Ok(Value::Bool(matched == 1)),
            // NONE: no element satisfies the predicate.
            "NONE" => Ok(Value::Bool(matched == 0)),
            _ => Err(ExpressionError::type_error(format!(
                "Unknown predicate function: {}",
                func
            ))),
        }
    }

    /// Evaluate a REDUCE expression
    /// `reduce(acc = initial, variable IN source | mapping)`.
    ///
    /// Generic default: seeds `accumulator` with `initial`, iterates the
    /// source collection binding `variable`, and accumulates the result of
    /// evaluating `mapping` at each step.
    fn evaluate_reduce(
        &mut self,
        accumulator: &str,
        initial: &Expression,
        variable: &str,
        source: &Expression,
        mapping: &Expression,
    ) -> Result<Value, ExpressionError>
    where
        Self: Sized,
    {
        let mut acc = ExpressionEvaluator::evaluate(initial, self)?;
        let source_value = ExpressionEvaluator::evaluate(source, self)?;
        let elements = match source_value {
            Value::List(list) => list.values,
            Value::Null(_) => return Ok(Value::Null(NullType::Null)),
            other => {
                return Err(ExpressionError::type_error(format!(
                    "REDUCE source must be a list, got {:?}",
                    other.get_type()
                )))
            }
        };
        for element in elements {
            self.set_variable(variable.to_string(), element);
            self.set_variable(accumulator.to_string(), acc.clone());
            acc = ExpressionEvaluator::evaluate(mapping, self)?;
        }
        Ok(acc)
    }

    /// Evaluate a path construction expression `path(v1, e1, v2, e2, v3)`.
    ///
    /// Generic default: evaluates each element and assembles a [`Path`] from
    /// an alternating vertex/edge sequence starting with a vertex.
    fn evaluate_path_build(&mut self, items: &[Expression]) -> Result<Value, ExpressionError>
    where
        Self: Sized,
    {
        let mut iter = items.iter();
        let src_value = match iter.next() {
            Some(expr) => ExpressionEvaluator::evaluate(expr, self)?,
            None => {
                return Err(ExpressionError::path_error(
                    "Path construction requires at least one vertex",
                ))
            }
        };
        let Value::Vertex(src) = src_value else {
            return Err(ExpressionError::path_error(
                "Path construction must start with a vertex",
            ));
        };
        let mut path = Path::new(*src);
        let mut remaining = iter;
        while let Some(edge_expr) = remaining.next() {
            let vertex_expr = remaining.next().ok_or_else(|| {
                ExpressionError::path_error(
                    "Path construction requires alternating edge/vertex elements",
                )
            })?;
            let edge_value = ExpressionEvaluator::evaluate(edge_expr, self)?;
            let vertex_value = ExpressionEvaluator::evaluate(vertex_expr, self)?;
            let Value::Edge(edge) = edge_value else {
                return Err(ExpressionError::path_error(
                    "Path construction edge element must be an edge",
                ));
            };
            let Value::Vertex(vertex) = vertex_value else {
                return Err(ExpressionError::path_error(
                    "Path construction vertex element must be a vertex",
                ));
            };
            path.add_step(Step { dst: vertex, edge });
        }
        Ok(Value::Path(Box::new(path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::Expression;
    use crate::core::types::VertexId;
    use crate::core::vertex_edge_path::{Edge, Tag};
    use crate::query::executor::expression::evaluation_context::DefaultExpressionContext;
    use std::collections::HashMap;

    fn context() -> DefaultExpressionContext {
        DefaultExpressionContext::new()
    }

    fn test_vertex() -> Value {
        let tag = Tag::new(
            "person".to_string(),
            HashMap::from([("name".to_string(), Value::string("Alice"))]),
        );
        Value::Vertex(Box::new(crate::core::vertex_edge_path::Vertex::new(
            VertexId::from_int64(1),
            vec![tag],
        )))
    }

    fn test_edge() -> Value {
        Value::Edge(Box::new(Edge::new(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "knows".to_string(),
            0,
            HashMap::new(),
        )))
    }

    #[test]
    fn evaluate_label_default_reports_label() {
        let mut ctx = context();
        let err = ExpressionEvaluator::evaluate(&Expression::label("Person"), &mut ctx)
            .expect_err("label expression cannot be resolved without context");
        assert!(
            err.message.contains("Person"),
            "error should name the label: {}",
            err.message
        );
    }

    #[test]
    fn evaluate_list_comprehension_with_filter_and_map() {
        let mut ctx = context();
        // [x IN [1,2,3,4] WHERE x > 2 | x * 10]
        let expr = Expression::list_comprehension(
            "x",
            Expression::list(vec![
                Expression::Literal(Value::Int(1)),
                Expression::Literal(Value::Int(2)),
                Expression::Literal(Value::Int(3)),
                Expression::Literal(Value::Int(4)),
            ]),
            Some(Expression::binary(
                Expression::variable("x"),
                crate::core::types::operators::BinaryOperator::GreaterThan,
                Expression::Literal(Value::Int(2)),
            )),
            Some(Expression::binary(
                Expression::variable("x"),
                crate::core::types::operators::BinaryOperator::Multiply,
                Expression::Literal(Value::Int(10)),
            )),
        );
        let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("comprehension");
        assert_eq!(
            result,
            Value::list(List::from(vec![Value::Int(30), Value::Int(40)]))
        );
    }

    #[test]
    fn evaluate_list_comprehension_empty_and_null() {
        let mut ctx = context();
        // Empty source list yields an empty result list.
        let empty = Expression::list_comprehension(
            "x",
            Expression::list(Vec::new()),
            None,
            Some(Expression::variable("x")),
        );
        assert_eq!(
            ExpressionEvaluator::evaluate(&empty, &mut ctx).expect("empty comprehension"),
            Value::list(List::from(Vec::<Value>::new()))
        );
        // Null source propagates null.
        let null_src = Expression::list_comprehension(
            "x",
            Expression::Literal(Value::Null(NullType::Null)),
            None,
            None,
        );
        assert_eq!(
            ExpressionEvaluator::evaluate(&null_src, &mut ctx).expect("null comprehension"),
            Value::Null(NullType::Null)
        );
        // Non-list source is a type error.
        let bad =
            Expression::list_comprehension("x", Expression::Literal(Value::Int(5)), None, None);
        assert!(ExpressionEvaluator::evaluate(&bad, &mut ctx).is_err());
    }

    #[test]
    fn evaluate_predicate_quantifiers() {
        let list = Expression::list(vec![
            Expression::Literal(Value::Int(1)),
            Expression::Literal(Value::Int(2)),
            Expression::Literal(Value::Int(3)),
        ]);
        let gt_two = Expression::binary(
            Expression::variable("x"),
            crate::core::types::operators::BinaryOperator::GreaterThan,
            Expression::Literal(Value::Int(2)),
        );
        let args = vec![Expression::variable("x"), list.clone(), gt_two.clone()];
        let mut ctx = context();
        let all =
            ExpressionEvaluator::evaluate(&Expression::predicate("all", args.clone()), &mut ctx)
                .expect("all");
        assert_eq!(all, Value::Bool(false));

        let mut ctx = context();
        let any =
            ExpressionEvaluator::evaluate(&Expression::predicate("any", args.clone()), &mut ctx)
                .expect("any");
        assert_eq!(any, Value::Bool(true));

        let mut ctx = context();
        let single =
            ExpressionEvaluator::evaluate(&Expression::predicate("single", args.clone()), &mut ctx)
                .expect("single");
        assert_eq!(single, Value::Bool(true));

        let mut ctx = context();
        let none =
            ExpressionEvaluator::evaluate(&Expression::predicate("none", args.clone()), &mut ctx)
                .expect("none");
        assert_eq!(none, Value::Bool(false));

        // All elements satisfy x > 0.
        let gt_zero = Expression::binary(
            Expression::variable("x"),
            crate::core::types::operators::BinaryOperator::GreaterThan,
            Expression::Literal(Value::Int(0)),
        );
        let mut ctx = context();
        let all_true = ExpressionEvaluator::evaluate(
            &Expression::predicate(
                "all",
                vec![Expression::variable("x"), list.clone(), gt_zero],
            ),
            &mut ctx,
        )
        .expect("all true");
        assert_eq!(all_true, Value::Bool(true));

        // Unknown quantifier is an error.
        let mut ctx = context();
        assert!(
            ExpressionEvaluator::evaluate(&Expression::predicate("maybe", args), &mut ctx,)
                .is_err()
        );
    }

    #[test]
    fn evaluate_reduce_accumulates() {
        let mut ctx = context();
        // reduce(acc = 0, x IN [1,2,3] | acc + x)
        let expr = Expression::reduce(
            "acc",
            Expression::Literal(Value::Int(0)),
            "x",
            Expression::list(vec![
                Expression::Literal(Value::Int(1)),
                Expression::Literal(Value::Int(2)),
                Expression::Literal(Value::Int(3)),
            ]),
            Expression::binary(
                Expression::variable("acc"),
                crate::core::types::operators::BinaryOperator::Add,
                Expression::variable("x"),
            ),
        );
        let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("reduce");
        assert_eq!(result, Value::Int(6));
    }

    #[test]
    fn evaluate_label_tag_property_accesses_vertex_property() {
        let mut ctx = context().add_variable("n".to_string(), test_vertex());
        // Dynamic tag access: (n).name
        let expr = Expression::label_tag_property(Expression::variable("n"), "name");
        let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("tag property");
        assert_eq!(result, Value::string("Alice"));
    }

    #[test]
    fn evaluate_path_build_assembles_path() {
        let mut ctx = context();
        let expr = Expression::path_build(vec![
            Expression::Literal(test_vertex()),
            Expression::Literal(test_edge()),
            Expression::Literal(test_vertex()),
        ]);
        let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("path build");
        let Value::Path(path) = result else {
            panic!("expected a path value");
        };
        assert_eq!(path.len(), 1);
        assert_eq!(path.src.vid, VertexId::from_int64(1));

        // Non-vertex start is a path error.
        let bad = Expression::path_build(vec![Expression::Literal(Value::Int(1))]);
        let err = ExpressionEvaluator::evaluate(&bad, &mut ctx).expect_err("path build error");
        assert!(err.message.contains("vertex"), "{}", err.message);
    }
}
