//! Compiled expression evaluation
//!
//! Compiles an [`Expression`] tree once into a closure tree that evaluates
//! against a raw row slice.  The compiled tree eliminates the per-row
//! recursive `match` over the full expression enum, pre-resolves variable
//! references to slot IDs and function references to the global registry at
//! compile time, and folds constant subtrees inline.
//!
//! Nodes that require the full runtime context (subqueries, predicates,
//! labels, list comprehensions, aggregates, ...) are kept as a fallback node
//! that delegates to the original per-row interpreter with a
//! [`BorrowedRowContext`], preserving the exact semantics of the scalar
//! evaluator.

use std::sync::Arc;

use crate::core::types::expr::Expression;
use crate::core::types::operators::{BinaryOperator, UnaryOperator};
use crate::core::value::list::List;
use crate::core::value::NullType;
use crate::core::DataType;
use crate::core::Value;
use crate::executor::expression::evaluator::collection_operations::CollectionOperationEvaluator;
use crate::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::executor::expression::evaluator::functions::FunctionEvaluator;
use crate::executor::expression::evaluator::operations::{
    BinaryOperationEvaluator, UnaryOperationEvaluator,
};
use crate::executor::expression::functions::{
    global_registry, global_registry_ref, OwnedFunctionRef,
};
use crate::executor::expression::ExpressionError;
use crate::executor::streaming::context::BorrowedRowContext;
use crate::executor::streaming::slot::{SlotId, SlotLayout};
use crate::executor::streaming::subquery::EvalEnv;

/// Global runtime switch for the compiled evaluation path.
///
/// Keep the scalar path selectable so problems can be pinned down to the
/// compiled evaluator without rebuilding.
static COMPILED_EVAL_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

/// Enable or disable the compiled expression evaluation path.
pub fn set_compiled_eval_enabled(enabled: bool) {
    COMPILED_EVAL_ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

/// Whether compiled expression evaluation is currently enabled.
pub fn compiled_eval_enabled() -> bool {
    COMPILED_EVAL_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// A columnar value: either a constant scalar (never materialized as a
/// column) or a full column of values.
#[derive(Debug, Clone)]
pub enum ColumnarValue {
    /// A constant scalar shared by every row.
    Const(Value),
    /// One value per row of the chunk.
    Column(Vec<Value>),
}

impl ColumnarValue {
    /// Materialize into a full-length column, broadcasting a constant.
    pub fn into_values(self, len: usize) -> Vec<Value> {
        match self {
            ColumnarValue::Const(v) => vec![v; len],
            ColumnarValue::Column(col) => col,
        }
    }
}

/// Compiled closure-tree node.
///
/// Mirrors the hot subset of [`Expression`] with slot-resolved variables and
/// pre-resolved function references; anything else is kept verbatim in
/// [`CompiledExpr::Fallback`] and evaluated through the scalar interpreter.
#[derive(Debug, Clone)]
pub enum CompiledExpr {
    /// Compile-time folded constant.
    Const(Value),
    /// A variable resolved to its slot at compile time.
    Slot(SlotId),
    /// A `var.prop` property access resolved to its compound slot at compile
    /// time (flat column layout).
    CompoundSlot(SlotId),
    /// Unary operation over a compiled operand.
    Unary {
        op: UnaryOperator,
        operand: Box<CompiledExpr>,
    },
    /// Binary operation over two compiled operands.
    Binary {
        op: BinaryOperator,
        left: Box<CompiledExpr>,
        right: Box<CompiledExpr>,
    },
    /// CASE expression with short-circuit evaluation.
    Case {
        test: Option<Box<CompiledExpr>>,
        conditions: Vec<(CompiledExpr, CompiledExpr)>,
        default: Option<Box<CompiledExpr>>,
    },
    /// Function call with compile-time resolved function reference.
    Function {
        name: String,
        func: Option<OwnedFunctionRef>,
        args: Vec<CompiledExpr>,
    },
    /// Type cast over a compiled operand.
    TypeCast {
        expr: Box<CompiledExpr>,
        target_type: DataType,
    },
    /// List literal.
    List(Vec<CompiledExpr>),
    /// Map literal.
    Map(Vec<(String, CompiledExpr)>),
    /// Subscript access.
    Subscript {
        collection: Box<CompiledExpr>,
        index: Box<CompiledExpr>,
    },
    /// Range access.
    Range {
        collection: Box<CompiledExpr>,
        start: Option<Box<CompiledExpr>>,
        end: Option<Box<CompiledExpr>>,
    },
    /// Path expression (evaluates to a list).
    Path(Vec<CompiledExpr>),
    /// Query parameter reference, resolved from the environment at runtime.
    Parameter(String),
    /// Session variable reference, resolved from the environment at runtime.
    SessionVariable(String),
    /// Everything that requires the full runtime context — evaluated per row
    /// through the original interpreter with a [`BorrowedRowContext`].
    Fallback(Expression),
}

impl CompiledExpr {
    /// Compile an expression tree against a slot layout.
    ///
    /// Variable references resolve to slot IDs, `var.prop` accesses to their
    /// compound slots, function references to the global registry, and
    /// constant subtrees are folded inline.  Any node that needs the runtime
    /// context stays as [`CompiledExpr::Fallback`].
    pub fn compile(expression: &Expression, layout: &SlotLayout) -> Self {
        match expression {
            Expression::Literal(v) => CompiledExpr::Const(v.clone()),
            Expression::Vector(d) => CompiledExpr::Const(Value::vector(d.clone())),

            Expression::Variable(name) => match layout.slot_id(name) {
                Some(slot) => CompiledExpr::Slot(slot),
                None => CompiledExpr::Fallback(expression.clone()),
            },

            // Flat property access: `var.prop` resolves to the compound slot
            // when the layout carries it; otherwise delegate to the runtime
            // interpreter (which falls back to Vertex/Map extraction).
            Expression::Property { object, property } => {
                if let Expression::Variable(var_name) = object.as_ref() {
                    let compound = format!("{}.{}", var_name, property);
                    if let Some(slot) = layout.slot_id(&compound) {
                        return CompiledExpr::CompoundSlot(slot);
                    }
                }
                CompiledExpr::Fallback(expression.clone())
            }

            Expression::Binary { left, op, right } => {
                let left = Self::compile(left, layout);
                let right = Self::compile(right, layout);
                match (left, right) {
                    (CompiledExpr::Const(l), CompiledExpr::Const(r)) => {
                        match BinaryOperationEvaluator::evaluate(&l, op, &r) {
                            Ok(v) => CompiledExpr::Const(v),
                            // Preserve runtime error semantics: keep the
                            // unfolded node so the error surfaces per row.
                            Err(_) => CompiledExpr::Binary {
                                op: *op,
                                left: Box::new(CompiledExpr::Const(l)),
                                right: Box::new(CompiledExpr::Const(r)),
                            },
                        }
                    }
                    (left, right) => CompiledExpr::Binary {
                        op: *op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                }
            }

            Expression::Unary { op, operand } => {
                let operand = Self::compile(operand, layout);
                match operand {
                    CompiledExpr::Const(v) => match UnaryOperationEvaluator::evaluate(op, &v) {
                        Ok(folded) => CompiledExpr::Const(folded),
                        Err(_) => CompiledExpr::Unary {
                            op: *op,
                            operand: Box::new(CompiledExpr::Const(v)),
                        },
                    },
                    operand => CompiledExpr::Unary {
                        op: *op,
                        operand: Box::new(operand),
                    },
                }
            }

            Expression::Function { name, args } => {
                let args: Vec<CompiledExpr> =
                    args.iter().map(|a| Self::compile(a, layout)).collect();
                // Compile-time folding only for pure functions with constant
                // arguments; unregistered functions are conservatively kept
                // for runtime resolution (identical error semantics).
                if args.iter().all(|a| matches!(a, CompiledExpr::Const(_)))
                    && global_registry_ref()
                        .get_builtin(name.as_str())
                        .is_some_and(|f| f.is_pure())
                {
                    let values: Vec<Value> = args
                        .iter()
                        .map(|a| match a {
                            CompiledExpr::Const(v) => v.clone(),
                            _ => unreachable!("guarded by all-Const check"),
                        })
                        .collect();
                    if let Ok(v) = global_registry().execute(name, &values) {
                        return CompiledExpr::Const(v);
                    }
                }
                let func = global_registry_ref()
                    .get_builtin(name.as_str())
                    .map(|f| OwnedFunctionRef::Builtin(f.clone()))
                    .or_else(|| {
                        global_registry_ref()
                            .get_custom(name.as_str())
                            .map(|f| OwnedFunctionRef::Custom(f.clone()))
                    });
                CompiledExpr::Function {
                    name: name.clone(),
                    func,
                    args,
                }
            }

            Expression::Aggregate {
                func,
                args,
                distinct,
                filter,
            } => {
                let args: Vec<CompiledExpr> =
                    args.iter().map(|a| Self::compile(a, layout)).collect();
                let filter = filter.as_ref().map(|f| Box::new(Self::compile(f, layout)));
                // Aggregates need a row-group context; fold only when the
                // whole call is constant and pure (mirrors the heuristic
                // constant-folding rule).
                if filter.is_none() && args.iter().all(|a| matches!(a, CompiledExpr::Const(_))) {
                    let values: Vec<Value> = args
                        .iter()
                        .map(|a| match a {
                            CompiledExpr::Const(v) => v.clone(),
                            _ => unreachable!("guarded by all-Const check"),
                        })
                        .collect();
                    if let Ok(v) =
                        FunctionEvaluator::eval_aggregate_function(func, &values, *distinct)
                    {
                        return CompiledExpr::Const(v);
                    }
                }
                CompiledExpr::Fallback(expression.clone())
            }

            Expression::List(items) => {
                CompiledExpr::List(items.iter().map(|e| Self::compile(e, layout)).collect())
            }

            Expression::Map(pairs) => CompiledExpr::Map(
                pairs
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::compile(v, layout)))
                    .collect(),
            ),

            Expression::Case {
                test_expr,
                conditions,
                default,
            } => CompiledExpr::Case {
                test: test_expr
                    .as_ref()
                    .map(|e| Box::new(Self::compile(e, layout))),
                conditions: conditions
                    .iter()
                    .map(|(c, v)| (Self::compile(c, layout), Self::compile(v, layout)))
                    .collect(),
                default: default.as_ref().map(|e| Box::new(Self::compile(e, layout))),
            },

            Expression::TypeCast {
                expression,
                target_type,
            } => {
                let expr = Self::compile(expression, layout);
                match expr {
                    CompiledExpr::Const(v) => {
                        match ExpressionEvaluator::eval_type_cast(&v, target_type) {
                            Ok(folded) => CompiledExpr::Const(folded),
                            Err(_) => CompiledExpr::TypeCast {
                                expr: Box::new(CompiledExpr::Const(v)),
                                target_type: target_type.clone(),
                            },
                        }
                    }
                    expr => CompiledExpr::TypeCast {
                        expr: Box::new(expr),
                        target_type: target_type.clone(),
                    },
                }
            }

            Expression::Subscript { collection, index } => CompiledExpr::Subscript {
                collection: Box::new(Self::compile(collection, layout)),
                index: Box::new(Self::compile(index, layout)),
            },

            Expression::Range {
                collection,
                start,
                end,
            } => CompiledExpr::Range {
                collection: Box::new(Self::compile(collection, layout)),
                start: start.as_ref().map(|e| Box::new(Self::compile(e, layout))),
                end: end.as_ref().map(|e| Box::new(Self::compile(e, layout))),
            },

            Expression::Path(items) => {
                CompiledExpr::Path(items.iter().map(|e| Self::compile(e, layout)).collect())
            }

            Expression::Parameter(name) => CompiledExpr::Parameter(name.clone()),
            Expression::SessionVariable(name) => CompiledExpr::SessionVariable(name.clone()),

            // Everything else requires the runtime context.
            _ => CompiledExpr::Fallback(expression.clone()),
        }
    }

    /// Evaluate the compiled expression against a single row.
    ///
    /// `layout` and `env` are only used by fallback nodes that delegate to the
    /// scalar interpreter; the compiled nodes read the row slice directly.
    pub fn evaluate(
        &self,
        row: &[Value],
        layout: Arc<SlotLayout>,
        env: Option<&EvalEnv>,
    ) -> Result<Value, ExpressionError> {
        match self {
            CompiledExpr::Const(v) => Ok(v.clone()),
            CompiledExpr::Slot(slot) => row
                .get(*slot)
                .cloned()
                .ok_or_else(|| ExpressionError::undefined_variable("column slot")),
            CompiledExpr::CompoundSlot(slot) => row
                .get(*slot)
                .cloned()
                .ok_or_else(|| ExpressionError::undefined_variable("column slot")),

            CompiledExpr::Unary { op, operand } => {
                let value = operand.evaluate(row, layout, env)?;
                UnaryOperationEvaluator::evaluate(op, &value)
            }

            CompiledExpr::Binary { op, left, right } => {
                let left_value = left.evaluate(row, layout.clone(), env)?;
                let right_value = right.evaluate(row, layout, env)?;
                BinaryOperationEvaluator::evaluate(&left_value, op, &right_value)
            }

            CompiledExpr::Case {
                test,
                conditions,
                default,
            } => {
                if let Some(test_expr) = test {
                    let test_value = test_expr.evaluate(row, layout.clone(), env)?;
                    for (condition, value) in conditions {
                        let condition_result = condition.evaluate(row, layout.clone(), env)?;
                        if test_value == condition_result {
                            return value.evaluate(row, layout, env);
                        }
                    }
                } else {
                    for (condition, value) in conditions {
                        let condition_result = condition.evaluate(row, layout.clone(), env)?;
                        match condition_result {
                            Value::Bool(true) => return value.evaluate(row, layout, env),
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
                    Some(default_expression) => default_expression.evaluate(row, layout, env),
                    None => Ok(Value::Null(NullType::Null)),
                }
            }

            CompiledExpr::Function { name, func, args } => {
                let mut arg_values = Vec::with_capacity(args.len());
                for arg in args {
                    arg_values.push(arg.evaluate(row, layout.clone(), env)?);
                }
                match func {
                    Some(f) => f.execute(&arg_values),
                    None => global_registry().execute(name, &arg_values),
                }
            }

            CompiledExpr::TypeCast { expr, target_type } => {
                let value = expr.evaluate(row, layout, env)?;
                ExpressionEvaluator::eval_type_cast(&value, target_type)
            }

            CompiledExpr::List(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(item.evaluate(row, layout.clone(), env)?);
                }
                Ok(Value::list(List::from(values)))
            }

            CompiledExpr::Map(entries) => {
                let mut map_values = std::collections::HashMap::new();
                for (key, value_expression) in entries {
                    let value = value_expression.evaluate(row, layout.clone(), env)?;
                    map_values.insert(Value::string(key.clone()), value);
                }
                Ok(Value::map(map_values))
            }

            CompiledExpr::Subscript { collection, index } => {
                let collection_value = collection.evaluate(row, layout.clone(), env)?;
                let index_value = index.evaluate(row, layout, env)?;
                CollectionOperationEvaluator::eval_subscript_access(&collection_value, &index_value)
            }

            CompiledExpr::Range {
                collection,
                start,
                end,
            } => {
                let collection_value = collection.evaluate(row, layout.clone(), env)?;
                let start_value = start
                    .as_ref()
                    .map(|e| e.evaluate(row, layout.clone(), env))
                    .transpose()?;
                let end_value = end
                    .as_ref()
                    .map(|e| e.evaluate(row, layout.clone(), env))
                    .transpose()?;
                CollectionOperationEvaluator::eval_range_access(
                    &collection_value,
                    start_value.as_ref(),
                    end_value.as_ref(),
                )
            }

            CompiledExpr::Path(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(item.evaluate(row, layout.clone(), env)?);
                }
                Ok(Value::list(List::from(values)))
            }

            CompiledExpr::Parameter(name) => env
                .and_then(|e| e.params.as_ref())
                .and_then(|p| p.get(name).cloned())
                .ok_or_else(|| ExpressionError::undefined_parameter(name)),

            CompiledExpr::SessionVariable(name) => env
                .and_then(|e| e.session_variables.as_ref())
                .and_then(|v| v.get(name).cloned())
                .ok_or_else(|| {
                    ExpressionError::type_error(format!(
                        "Session variable `{}` is not defined",
                        name
                    ))
                }),

            CompiledExpr::Fallback(expression) => {
                let mut ctx = match env {
                    Some(env) => BorrowedRowContext::with_env(row, layout, env),
                    None => BorrowedRowContext::new(row, layout),
                };
                ExpressionEvaluator::evaluate(expression, &mut ctx)
            }
        }
    }

    /// Batch-evaluate the compiled expression over every row of a chunk.
    ///
    /// The columnar entry point: constants are never materialized into
    /// columns, binary/unary operators run elementwise over the produced
    /// columns, and fallback nodes delegate per row to the scalar
    /// interpreter.
    pub fn evaluate_batch(
        &self,
        rows: &[Vec<Value>],
        layout: Arc<SlotLayout>,
        env: Option<&EvalEnv>,
    ) -> Result<ColumnarValue, ExpressionError> {
        let len = rows.len();
        match self {
            CompiledExpr::Const(v) => Ok(ColumnarValue::Const(v.clone())),
            CompiledExpr::Slot(slot) | CompiledExpr::CompoundSlot(slot) => {
                let mut col = Vec::with_capacity(len);
                for row in rows {
                    col.push(
                        row.get(*slot)
                            .cloned()
                            .ok_or_else(|| ExpressionError::undefined_variable("column slot"))?,
                    );
                }
                Ok(ColumnarValue::Column(col))
            }

            CompiledExpr::Unary { op, operand } => {
                let operand = operand.evaluate_batch(rows, layout, env)?;
                match operand {
                    ColumnarValue::Const(v) => Ok(ColumnarValue::Const(
                        UnaryOperationEvaluator::evaluate(op, &v)?,
                    )),
                    ColumnarValue::Column(col) => {
                        let mut out = Vec::with_capacity(col.len());
                        for v in col {
                            out.push(UnaryOperationEvaluator::evaluate(op, &v)?);
                        }
                        Ok(ColumnarValue::Column(out))
                    }
                }
            }

            CompiledExpr::Binary { op, left, right } => {
                let left = left.evaluate_batch(rows, layout.clone(), env)?;
                let right = right.evaluate_batch(rows, layout, env)?;
                match (left, right) {
                    (ColumnarValue::Const(l), ColumnarValue::Const(r)) => Ok(ColumnarValue::Const(
                        BinaryOperationEvaluator::evaluate(&l, op, &r)?,
                    )),
                    (ColumnarValue::Const(l), ColumnarValue::Column(r)) => {
                        let mut out = Vec::with_capacity(r.len());
                        for v in r {
                            out.push(BinaryOperationEvaluator::evaluate(&l, op, &v)?);
                        }
                        Ok(ColumnarValue::Column(out))
                    }
                    (ColumnarValue::Column(l), ColumnarValue::Const(r)) => {
                        let mut out = Vec::with_capacity(l.len());
                        for v in l {
                            out.push(BinaryOperationEvaluator::evaluate(&v, op, &r)?);
                        }
                        Ok(ColumnarValue::Column(out))
                    }
                    (ColumnarValue::Column(l), ColumnarValue::Column(r)) => {
                        let mut out = Vec::with_capacity(l.len());
                        for (lv, rv) in l.into_iter().zip(r) {
                            out.push(BinaryOperationEvaluator::evaluate(&lv, op, &rv)?);
                        }
                        Ok(ColumnarValue::Column(out))
                    }
                }
            }

            // Short-circuit semantics (CASE, subqueries, parameter lookup,
            // per-row collection access) run through the per-row evaluator.
            _ => {
                let mut out = Vec::with_capacity(len);
                for row in rows {
                    out.push(self.evaluate(row, layout.clone(), env)?);
                }
                Ok(ColumnarValue::Column(out))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::{BinaryOperator, UnaryOperator};
    use crate::executor::streaming::slot::SlotLayout;

    fn layout(names: &[&str]) -> Arc<SlotLayout> {
        Arc::new(SlotLayout::from_names(
            &names.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        ))
    }

    fn eval_once(expr: &Expression, row: Vec<Value>, names: &[&str]) -> Value {
        let layout = layout(names);
        let compiled = CompiledExpr::compile(expr, &layout);
        compiled
            .evaluate(&row, layout, None)
            .expect("evaluation should succeed")
    }

    #[test]
    fn literal_compiles_to_const() {
        let expr = Expression::Literal(Value::Int(42));
        let layout = layout(&["a"]);
        let compiled = CompiledExpr::compile(&expr, &layout);
        assert!(matches!(compiled, CompiledExpr::Const(_)));
        assert_eq!(
            eval_once(&expr, vec![Value::Int(1)], &["a"]),
            Value::Int(42)
        );
    }

    #[test]
    fn variable_resolves_to_slot() {
        let expr = Expression::Variable("a".to_string());
        let layout = layout(&["a", "b"]);
        let compiled = CompiledExpr::compile(&expr, &layout);
        assert!(matches!(compiled, CompiledExpr::Slot(0)));
        assert_eq!(
            eval_once(&expr, vec![Value::Int(7), Value::Int(9)], &["a", "b"]),
            Value::Int(7)
        );
    }

    #[test]
    fn property_resolves_to_compound_slot() {
        let expr = Expression::Property {
            object: Box::new(Expression::Variable("p".to_string())),
            property: "age".to_string(),
        };
        let layout = layout(&["id", "p.age"]);
        let compiled = CompiledExpr::compile(&expr, &layout);
        assert!(matches!(compiled, CompiledExpr::CompoundSlot(1)));
        assert_eq!(
            eval_once(&expr, vec![Value::Int(1), Value::Int(30)], &["id", "p.age"]),
            Value::Int(30)
        );
    }

    #[test]
    fn binary_folds_constants_at_compile_time() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Literal(Value::Int(2))),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(3))),
        };
        let layout = layout(&["a"]);
        let compiled = CompiledExpr::compile(&expr, &layout);
        assert!(matches!(compiled, CompiledExpr::Const(Value::Int(5))));
        assert_eq!(eval_once(&expr, vec![Value::Int(0)], &["a"]), Value::Int(5));
    }

    #[test]
    fn binary_with_variable_stays_unfolded() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Variable("a".to_string())),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(1))),
        };
        let layout = layout(&["a"]);
        let compiled = CompiledExpr::compile(&expr, &layout);
        assert!(matches!(compiled, CompiledExpr::Binary { .. }));
        assert_eq!(
            eval_once(&expr, vec![Value::Int(10)], &["a"]),
            Value::Int(11)
        );
    }

    #[test]
    fn null_semantics_match_scalar_evaluator() {
        // The compiled path reuses the exact binary evaluator, so NULL
        // handling must be identical to the scalar path.  Assert against the
        // scalar result rather than assuming three-valued logic.
        let expr = Expression::Binary {
            left: Box::new(Expression::Variable("a".to_string())),
            op: BinaryOperator::Equal,
            right: Box::new(Expression::Literal(Value::Int(1))),
        };
        let layout = layout(&["a"]);
        let row = vec![Value::Null(NullType::Null)];
        let compiled = CompiledExpr::compile(&expr, &layout);
        let compiled_result = compiled
            .evaluate(&row, layout.clone(), None)
            .expect("evaluation should succeed");

        let mut ctx = BorrowedRowContext::new(&row, layout);
        let scalar_result =
            ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("evaluation should succeed");
        assert_eq!(compiled_result, scalar_result);
    }

    #[test]
    fn unary_folds_constant() {
        let expr = Expression::Unary {
            op: UnaryOperator::Minus,
            operand: Box::new(Expression::Literal(Value::Int(4))),
        };
        let layout = layout(&["a"]);
        let compiled = CompiledExpr::compile(&expr, &layout);
        assert!(matches!(compiled, CompiledExpr::Const(Value::Int(-4))));
    }

    #[test]
    fn impure_function_is_not_folded() {
        let expr = Expression::Function {
            name: "rand".to_string(),
            args: vec![],
        };
        let layout = layout(&["a"]);
        let compiled = CompiledExpr::compile(&expr, &layout);
        // `rand` is impure: the call stays as a Function node, never a Const.
        assert!(matches!(compiled, CompiledExpr::Function { .. }));
    }

    #[test]
    fn unknown_variable_falls_back_to_interpreter() {
        // A variable that is not in the layout keeps the original expression
        // so the interpreter reports the same undefined-variable error.
        let expr = Expression::Variable("missing".to_string());
        let layout = layout(&["a"]);
        let compiled = CompiledExpr::compile(&expr, &layout);
        assert!(matches!(compiled, CompiledExpr::Fallback(_)));
        let err = compiled
            .evaluate(&[Value::Int(1)], layout, None)
            .unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn batch_evaluation_matches_per_row() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Variable("a".to_string())),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(10))),
        };
        let layout = layout(&["a"]);
        let rows = vec![
            vec![Value::Int(1)],
            vec![Value::Int(2)],
            vec![Value::Int(3)],
        ];
        let compiled = CompiledExpr::compile(&expr, &layout);
        let batch = compiled
            .evaluate_batch(&rows, layout.clone(), None)
            .expect("batch evaluation should succeed")
            .into_values(rows.len());
        assert_eq!(batch, vec![Value::Int(11), Value::Int(12), Value::Int(13)]);
    }

    #[test]
    fn batch_const_never_materializes() {
        let expr = Expression::Literal(Value::Bool(true));
        let layout = layout(&["a"]);
        let rows = vec![vec![Value::Int(1)], vec![Value::Int(2)]];
        let compiled = CompiledExpr::compile(&expr, &layout);
        let batch = compiled
            .evaluate_batch(&rows, layout, None)
            .expect("batch evaluation should succeed");
        assert!(matches!(batch, ColumnarValue::Const(_)));
    }

    #[test]
    fn fallback_switch_toggles_compiled_path() {
        assert!(compiled_eval_enabled());
        set_compiled_eval_enabled(false);
        assert!(!compiled_eval_enabled());
        set_compiled_eval_enabled(true);
        assert!(compiled_eval_enabled());
    }

    #[test]
    fn subquery_stays_fallback() {
        let expr = Expression::Exists {
            body: Box::new(crate::core::types::expr::SubqueryBody {
                id: 0,
                patterns: vec![],
                where_clause: None,
                return_expr: None,
            }),
        };
        let layout = layout(&["a"]);
        let compiled = CompiledExpr::compile(&expr, &layout);
        assert!(matches!(compiled, CompiledExpr::Fallback(_)));
    }

    #[test]
    fn case_short_circuits() {
        let expr = Expression::Case {
            test_expr: None,
            conditions: vec![
                (
                    Expression::Binary {
                        left: Box::new(Expression::Variable("a".to_string())),
                        op: BinaryOperator::GreaterThan,
                        right: Box::new(Expression::Literal(Value::Int(5))),
                    },
                    Expression::Literal(Value::string("big")),
                ),
                (
                    Expression::Literal(Value::Bool(true)),
                    Expression::Literal(Value::string("small")),
                ),
            ],
            default: Some(Box::new(Expression::Literal(Value::Null(NullType::Null)))),
        };
        assert_eq!(
            eval_once(&expr, vec![Value::Int(9)], &["a"]),
            Value::string("big")
        );
        assert_eq!(
            eval_once(&expr, vec![Value::Int(1)], &["a"]),
            Value::string("small")
        );
    }
}
