use crate::parser::ast::ReturnItem;
use graphdb_core::error::DBError;
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::semantic::{AliasType, ValueType};
use graphdb_core::value::NullType;
use graphdb_core::DBResult;
use graphdb_core::DataType;
use graphdb_core::Value;

use crate::binder::bound::*;
use crate::binder::expr_binder::ExpressionBinder;
use crate::binder::scope::{BinderScope, BinderVariable};

use super::Binder;

impl Binder {
    // ── Expression binding ─────────────────────────────────────────────────

    pub(crate) fn bind_expr(
        &mut self,
        expr: &ContextualExpression,
    ) -> DBResult<BoundExpression> {
        super::super::semantic_checker::validate_expression(expr)?;
        let type_hint = expr.data_type();
        let Some(inner) = expr.get_expression() else {
            return Err(DBError::from(
                graphdb_core::error::QueryError::invalid_query(
                    "Expression not found in context".to_string(),
                ),
            ));
        };
        self.bind_inner_expr(&inner, type_hint.as_ref())
    }

    /// Walk an expression and reject any variable reference that is not
    /// defined in the current binding scope.
    ///
    /// Used by clause binders that must validate their output variables
    /// (e.g. RETURN), where an undefined reference is a user error rather
    /// than a silently-null value.
    pub(crate) fn ensure_variables_defined(
        &self,
        expr: &ContextualExpression,
    ) -> DBResult<()> {
        fn check(scope: &BinderScope, e: &Expression) -> DBResult<()> {
            match e {
                Expression::Variable(name) => {
                    if !scope.contains(name) {
                        return Err(DBError::from(
                            graphdb_core::error::QueryError::invalid_query(format!(
                                "Undefined variable: {}",
                                name
                            )),
                        ));
                    }
                    Ok(())
                }
                Expression::Property { object, .. } => check(scope, object),
                Expression::Binary { left, right, .. } => {
                    check(scope, left)?;
                    check(scope, right)
                }
                Expression::Unary { operand, .. } => check(scope, operand),
                Expression::Function { args, .. } => args.iter().try_for_each(|a| check(scope, a)),
                Expression::Aggregate { args, filter, .. } => {
                    args.iter().try_for_each(|a| check(scope, a))?;
                    if let Some(f) = filter {
                        check(scope, f)?;
                    }
                    Ok(())
                }
                Expression::List(items) => items.iter().try_for_each(|i| check(scope, i)),
                Expression::Map(pairs) => pairs.iter().try_for_each(|(_, v)| check(scope, v)),
                Expression::Case {
                    test_expr,
                    conditions,
                    default,
                } => {
                    if let Some(t) = test_expr {
                        check(scope, t)?;
                    }
                    for (c, v) in conditions {
                        check(scope, c)?;
                        check(scope, v)?;
                    }
                    if let Some(d) = default {
                        check(scope, d)?;
                    }
                    Ok(())
                }
                Expression::TypeCast { expression, .. } => check(scope, expression),
                Expression::Subscript { collection, index } => {
                    check(scope, collection)?;
                    check(scope, index)
                }
                Expression::Range { collection, .. } => check(scope, collection),
                Expression::Path(items) => items.iter().try_for_each(|i| check(scope, i)),
                Expression::ListComprehension {
                    variable,
                    source,
                    filter,
                    map,
                } => {
                    check(scope, source)?;
                    let inner = inner_scope_with_variable(scope, variable);
                    if let Some(f) = filter {
                        check(&inner, f)?;
                    }
                    if let Some(m) = map {
                        check(&inner, m)?;
                    }
                    Ok(())
                }
                Expression::LabelTagProperty { tag, .. } => check(scope, tag),
                Expression::Predicate { args, .. } => {
                    // First argument is the locally bound iteration variable;
                    // the source is checked in the outer scope and the
                    // predicate condition in a scope where it is bound.
                    let Some(Expression::Variable(variable)) = args.first() else {
                        return args.iter().try_for_each(|a| check(scope, a));
                    };
                    let Some(source_expr) = args.get(1) else {
                        return args.iter().try_for_each(|a| check(scope, a));
                    };
                    let Some(predicate_expr) = args.get(2) else {
                        return args.iter().try_for_each(|a| check(scope, a));
                    };
                    check(scope, source_expr)?;
                    let inner = inner_scope_with_variable(scope, variable);
                    check(&inner, predicate_expr)
                }
                Expression::Reduce {
                    accumulator,
                    initial,
                    variable,
                    source,
                    mapping,
                } => {
                    check(scope, initial)?;
                    check(scope, source)?;
                    let mut inner = inner_scope_with_variable(scope, variable);
                    inner.define_variable(local_variable(accumulator));
                    check(&inner, mapping)
                }
                Expression::PathBuild(items) => items.iter().try_for_each(|i| check(scope, i)),
                Expression::WindowFunction { args, .. } => {
                    args.iter().try_for_each(|a| check(scope, a))
                }
                _ => Ok(()),
            }
        }
        let Some(inner) = expr.get_expression() else {
            return Ok(());
        };
        check(&self.scope, &inner)
    }

    pub(crate) fn bind_inner_expr(
        &mut self,
        expr: &Expression,
        type_hint: Option<&DataType>,
    ) -> DBResult<BoundExpression> {
        match expr {
            Expression::Literal(v) => {
                let dt = v.data_type();
                Ok(BoundExpression::Literal(v.clone(), dt))
            }
            Expression::Variable(v) => {
                let dt = type_hint.cloned().unwrap_or(DataType::String);
                let _col_type = if let Some(var_info) = self.scope.lookup(v) {
                    var_info.properties.get(v).cloned()
                } else {
                    None
                };
                Ok(BoundExpression::Variable(v.clone(), dt))
            }
            Expression::Property { object, property } => {
                let obj = self.bind_inner_expr(object, type_hint)?;
                let var_name = match object.as_ref() {
                    Expression::Variable(v) => Some(v.clone()),
                    _ => None,
                };
                let prop_type = if let Some(ref var_name) = var_name {
                    self.resolve_property_type(var_name, property)
                } else {
                    DataType::String
                };
                Ok(BoundExpression::Property {
                    object: Box::new(obj),
                    property: property.clone(),
                    value_type: prop_type,
                })
            }
            Expression::StructField { base, field } => {
                let obj = self.bind_inner_expr(base, None)?;
                let field_type = self.resolve_struct_field_type(&obj, field);
                // Binding bug guard: a resolved Struct base must not yield
                // Empty; schema-resolved field types must flow downstream.
                debug_assert!(
                    !matches!(&obj.return_type(), DataType::Struct(_))
                        || field_type != DataType::Empty,
                    "StructField '{}' could not be resolved against its Struct base",
                    field
                );
                Ok(BoundExpression::StructField {
                    base: Box::new(obj),
                    field: field.clone(),
                    return_type: field_type,
                })
            }
            Expression::Binary { left, op, right } => {
                let left = self.bind_inner_expr(left, None)?;
                let right = self.bind_inner_expr(right, None)?;
                let return_type = type_hint.cloned().unwrap_or_else(|| {
                    let expr_binder = ExpressionBinder::new(&self.scope);
                    expr_binder.deduce_arithmetic_type(&left.return_type(), &right.return_type())
                });
                Ok(BoundExpression::BinaryOp {
                    left: Box::new(left),
                    op: *op,
                    right: Box::new(right),
                    return_type,
                })
            }
            Expression::Unary { op, operand } => {
                let operand = self.bind_inner_expr(operand, None)?;
                let return_type = type_hint.cloned().unwrap_or(DataType::Bool);
                Ok(BoundExpression::UnaryOp {
                    op: *op,
                    operand: Box::new(operand),
                    return_type,
                })
            }
            Expression::Function { name, args } => {
                let args = args
                    .iter()
                    .map(|a| self.bind_inner_expr(a, None))
                    .collect::<DBResult<Vec<_>>>()?;
                let arg_types: Vec<DataType> = args.iter().map(|a| a.return_type()).collect();
                let return_type = {
                    let expr_binder = ExpressionBinder::new(&self.scope);
                    ValueType::from_data_type(
                        &expr_binder.deduce_function_return_type(name, &arg_types),
                    )
                };
                Ok(BoundExpression::Function(BoundFunctionCall {
                    name: name.clone(),
                    args,
                    return_type,
                }))
            }
            Expression::Aggregate {
                func,
                args,
                distinct,
                filter: _filter,
            } => {
                let args = args
                    .iter()
                    .map(|a| self.bind_inner_expr(a, None))
                    .collect::<DBResult<Vec<_>>>()?;
                let arg_type = args
                    .first()
                    .map(|a| a.return_type())
                    .unwrap_or(DataType::Unknown);
                let return_type = {
                    let expr_binder = ExpressionBinder::new(&self.scope);
                    ValueType::from_data_type(
                        &expr_binder.deduce_aggregate_return_type(func, &arg_type),
                    )
                };
                Ok(BoundExpression::Aggregate(BoundAggregateCall {
                    function_name: format!("{:?}", func),
                    arguments: args,
                    distinct: *distinct,
                    alias: None,
                    return_type,
                }))
            }
            Expression::List(items) => {
                let items = items
                    .iter()
                    .map(|i| self.bind_inner_expr(i, None))
                    .collect::<DBResult<Vec<_>>>()?;
                let element_type = {
                    let mut common = DataType::Unknown;
                    for item in &items {
                        let item_type = item.return_type();
                        common = if common == DataType::Unknown {
                            item_type
                        } else {
                            graphdb_core::type_system::TypeUtils::get_common_type(
                                &common, &item_type,
                            )
                        };
                        if common == DataType::Empty {
                            break;
                        }
                    }
                    if common == DataType::Empty {
                        DataType::Unknown
                    } else {
                        common
                    }
                };
                Ok(BoundExpression::List(
                    items,
                    DataType::List(Box::new(element_type)),
                ))
            }
            Expression::Map(entries) => {
                let entries = entries
                    .iter()
                    .map(|(k, v)| self.bind_inner_expr(v, None).map(|b| (k.clone(), b)))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundExpression::Map(
                    entries,
                    DataType::Map(Box::new(DataType::Empty)),
                ))
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let test = test_expr
                    .as_ref()
                    .map(|e| self.bind_inner_expr(e, None))
                    .transpose()?;
                let conds = conditions
                    .iter()
                    .map(|(c, r)| {
                        self.bind_inner_expr(c, None)
                            .and_then(|bc| self.bind_inner_expr(r, None).map(|br| (bc, br)))
                    })
                    .collect::<DBResult<Vec<_>>>()?;
                let def = default
                    .as_ref()
                    .map(|e| self.bind_inner_expr(e, None))
                    .transpose()?;
                let return_type = conds
                    .first()
                    .map(|(_, v)| v.return_type())
                    .or_else(|| def.as_ref().map(|d| d.return_type()))
                    .unwrap_or(DataType::String);
                Ok(BoundExpression::Case {
                    expr: test.map(Box::new),
                    when_then: conds,
                    else_expr: def.map(Box::new),
                    return_type,
                })
            }
            Expression::TypeCast {
                expression,
                target_type,
            } => {
                let e = self.bind_inner_expr(expression, Some(target_type))?;
                Ok(BoundExpression::Cast {
                    expr: Box::new(e),
                    target_type: target_type.clone(),
                })
            }
            Expression::Subscript { collection, index } => {
                let col = self.bind_inner_expr(collection, None)?;
                let idx = self.bind_inner_expr(index, None)?;
                Ok(BoundExpression::Subscript {
                    collection: Box::new(col),
                    index: Box::new(idx),
                    return_type: DataType::String,
                })
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                let col = self.bind_inner_expr(collection, None)?;
                let s = start
                    .as_ref()
                    .map(|e| self.bind_inner_expr(e, None))
                    .transpose()?;
                let e = end
                    .as_ref()
                    .map(|r| self.bind_inner_expr(r, None))
                    .transpose()?;
                Ok(BoundExpression::List(
                    vec![
                        col,
                        s.unwrap_or(BoundExpression::Literal(
                            Value::Null(NullType::Null),
                            DataType::Null,
                        )),
                        e.unwrap_or(BoundExpression::Literal(
                            Value::Null(NullType::Null),
                            DataType::Null,
                        )),
                    ],
                    DataType::List(Box::new(DataType::Empty)),
                ))
            }
            Expression::Path(elements) => {
                let elems = elements
                    .iter()
                    .map(|e| self.bind_inner_expr(e, None))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundExpression::Path(
                    elems,
                    DataType::List(Box::new(DataType::Unknown)),
                ))
            }
            Expression::Label(l) => Ok(BoundExpression::Label(l.clone())),
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => {
                let src = self.bind_inner_expr(source, None)?;
                let flt = filter
                    .as_ref()
                    .map(|f| self.bind_inner_expr(f, None))
                    .transpose()?;
                let mp = map
                    .as_ref()
                    .map(|m| self.bind_inner_expr(m, None))
                    .transpose()?;
                Ok(BoundExpression::ListComprehension {
                    variable: variable.clone(),
                    source: Box::new(src),
                    filter: flt.map(Box::new),
                    map: mp.map(Box::new),
                    return_type: DataType::List(Box::new(DataType::Unknown)),
                })
            }
            Expression::LabelTagProperty { tag, property } => {
                let tag_name = match tag.as_ref() {
                    Expression::Variable(v) => v.clone(),
                    _ => format!("{:?}", tag),
                };
                Ok(BoundExpression::TagProperty {
                    tag_name,
                    property: property.clone(),
                    value_type: DataType::String,
                })
            }
            Expression::TagProperty { tag_name, property } => Ok(BoundExpression::TagProperty {
                tag_name: tag_name.clone(),
                property: property.clone(),
                value_type: DataType::String,
            }),
            Expression::EdgeProperty {
                edge_name,
                property,
            } => Ok(BoundExpression::EdgeProperty {
                edge_name: edge_name.clone(),
                property: property.clone(),
                value_type: DataType::String,
            }),
            Expression::Predicate { func, args } => {
                let args = args
                    .iter()
                    .map(|a| self.bind_inner_expr(a, None))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundExpression::Predicate {
                    func: func.clone(),
                    args,
                    return_type: DataType::Bool,
                })
            }
            Expression::Reduce {
                accumulator,
                initial,
                variable,
                source,
                mapping,
            } => {
                let init = self.bind_inner_expr(initial, None)?;
                let src = self.bind_inner_expr(source, None)?;
                let map = self.bind_inner_expr(mapping, None)?;
                // The REDUCE result type is the accumulator type: prefer the
                // initial value type, fall back to the mapping result type.
                let expr_binder = ExpressionBinder::new(&self.scope);
                let mut return_type = init.return_type();
                if return_type == DataType::Unknown {
                    return_type = expr_binder.resolve_type(mapping);
                }
                Ok(BoundExpression::Reduce {
                    accumulator: accumulator.clone(),
                    initial: Box::new(init),
                    variable: variable.clone(),
                    source: Box::new(src),
                    mapping: Box::new(map),
                    return_type,
                })
            }
            Expression::PathBuild(elements) => {
                let elems = elements
                    .iter()
                    .map(|e| self.bind_inner_expr(e, None))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundExpression::PathBuild(
                    elems,
                    DataType::List(Box::new(DataType::Unknown)),
                ))
            }
            Expression::Parameter(p) => {
                Ok(BoundExpression::ParameterRef(p.clone(), DataType::String))
            }
            Expression::SessionVariable(name) => Ok(BoundExpression::SessionVariable(
                name.clone(),
                DataType::Unknown,
            )),
            Expression::Vector(v) => Ok(BoundExpression::Vector(v.clone())),
            Expression::WindowFunction {
                name,
                args,
                over_partition_by,
                over_order_by,
                over_order_desc,
            } => {
                let args = args
                    .iter()
                    .map(|a| self.bind_inner_expr(a, None))
                    .collect::<DBResult<Vec<_>>>()?;
                let part_by = over_partition_by
                    .iter()
                    .map(|p| self.bind_inner_expr(p, None))
                    .collect::<DBResult<Vec<_>>>()?;
                let order_by = over_order_by
                    .iter()
                    .map(|o| self.bind_inner_expr(o, None))
                    .collect::<DBResult<Vec<_>>>()?;
                // Window functions derive their return type from the
                // underlying function name and argument types.
                let return_type = {
                    let expr_binder = ExpressionBinder::new(&self.scope);
                    let arg_types: Vec<DataType> = args.iter().map(|a| a.return_type()).collect();
                    expr_binder.deduce_function_return_type(name, &arg_types)
                };
                Ok(BoundExpression::WindowFunction {
                    name: name.clone(),
                    args,
                    over_partition_by: part_by,
                    over_order_by: order_by,
                    over_order_desc: over_order_desc.clone(),
                    return_type,
                })
            }
            Expression::Exists { body } => {
                let query = self.bind_subquery_body(body)?;
                Ok(BoundExpression::Exists {
                    query: Box::new(query),
                })
            }
            Expression::In {
                expr: innerexpr,
                subquery,
                negated,
            } => {
                let bound_expr = self.bind_inner_expr(innerexpr, None)?;
                let query = self.bind_subquery_body(subquery)?;
                Ok(BoundExpression::In {
                    expr: Box::new(bound_expr),
                    subquery: Box::new(query),
                    negated: *negated,
                })
            }
        }
    }

    fn resolve_property_type(&self, var_name: &str, property: &str) -> DataType {
        if let Some(var_info) = self.scope.lookup(var_name) {
            if let Some(vt) = var_info.properties.get(property) {
                return vt.to_data_type();
            }
            if let Some(ref sm) = self.schema_manager {
                if let Some(ref space_name) = self.space_name {
                    for tag_name in &var_info.tags {
                        if let Ok(Some(tag_info)) = sm.get_tag(space_name, tag_name) {
                            for prop in &tag_info.properties {
                                if prop.name == property {
                                    return prop.data_type.clone();
                                }
                            }
                        }
                    }
                }
            }
        }
        DataType::String
    }

    /// Resolve the type of a STRUCT field access `base.field`.
    ///
    /// The base expression carries its own concrete type when schema context
    /// is available (a `STRUCT{...}` literal, or a `Property` whose schema
    /// declares a Struct type). Falls back to `String` otherwise, mirroring
    /// `Property` semantics.
    fn resolve_struct_field_type(&self, base: &BoundExpression, field: &str) -> DataType {
        if let DataType::Struct(info) = base.return_type() {
            if let Some((_, field_type)) = info.fields.iter().find(|(name, _)| name == field) {
                return field_type.clone();
            }
        }
        DataType::String
    }

    // ── Clause helpers ─────────────────────────────────────────────────────

    pub(crate) fn bind_return_clause(
        &mut self,
        rc: &crate::parser::ast::ReturnClause,
    ) -> DBResult<BoundReturnClause> {
        let items = rc
            .items
            .iter()
            .map(|item| match item {
                ReturnItem::Expression { expression, alias } => {
                    // Reject references to variables that are not defined in
                    // the current binding scope (e.g. `RETURN undefined_var`).
                    self.ensure_variables_defined(expression)?;
                    self.bind_expr(expression).map(|be| BoundReturnItem {
                        expression: be,
                        alias: alias.clone(),
                    })
                }
            })
            .collect::<DBResult<Vec<_>>>()?;

        let order_by = rc
            .order_by
            .as_ref()
            .map(|ob| {
                ob.items
                    .iter()
                    .map(|item| {
                        self.bind_expr(&item.expression)
                            .map(|be| BoundOrderByItem {
                                expression: be,
                                direction: item.direction,
                            })
                    })
                    .collect::<DBResult<Vec<_>>>()
            })
            .transpose()?;

        Ok(BoundReturnClause {
            items,
            distinct: rc.distinct,
            order_by,
            limit: rc.limit.clone(),
            skip: rc.skip.clone(),
            sample: rc.sample.clone(),
        })
    }

    pub(crate) fn bind_yield_clause(
        &mut self,
        yc: &crate::parser::ast::YieldClause,
    ) -> DBResult<BoundYieldClause> {
        let items = yc
            .items
            .iter()
            .map(|item| {
                self.bind_expr(&item.expression).map(|be| BoundYieldItem {
                    expression: be,
                    alias: item.alias.clone(),
                })
            })
            .collect::<DBResult<Vec<_>>>()?;

        let order_by = yc
            .order_by
            .as_ref()
            .map(|ob| {
                ob.items
                    .iter()
                    .map(|item| {
                        self.bind_expr(&item.expression)
                            .map(|be| BoundOrderByItem {
                                expression: be,
                                direction: item.direction,
                            })
                    })
                    .collect::<DBResult<Vec<_>>>()
            })
            .transpose()?;

        Ok(BoundYieldClause {
            items,
            distinct: false,
            order_by,
            limit: yc.limit.clone(),
            skip: yc.skip.clone(),
        })
    }
}

// ── Scope helpers for runtime-context expressions ──────────────────────────

/// Create a child scope that additionally binds `variable` as a local
/// iteration variable (list comprehension / predicate / reduce).
fn inner_scope_with_variable(scope: &BinderScope, variable: &str) -> BinderScope {
    let mut inner = BinderScope::with_parent(scope.clone());
    inner.define_variable(local_variable(variable));
    inner
}

/// Build a runtime-typed local binder variable for a scope-local
/// iteration variable.
fn local_variable(name: &str) -> BinderVariable {
    BinderVariable {
        name: name.to_string(),
        alias_type: AliasType::Runtime,
        tags: Vec::new(),
        properties: std::collections::HashMap::new(),
        is_defined: true,
    }
}
