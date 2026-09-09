//! Variable substitution for macro expansion.
//!
//! [`substitute_variables`] replaces free `Expression::Variable` occurrences
//! with caller-supplied expressions. Binders introduced inside the rewritten
//! tree (`Lambda` parameters, comprehension variables, `Reduce`
//! accumulator/element) shadow same-named mapping entries so expansion can
//! never capture query-local variables.

use std::collections::{HashMap, HashSet};

use super::def::{Expression, FunctionArg};

/// Substitute free variables in `expr` using `mapping`.
///
/// Both mapping keys and variable names are matched case-insensitively
/// (macro parameters are case-insensitive); the replacement expression is
/// cloned into every occurrence.
pub fn substitute_variables(
    expr: &Expression,
    mapping: &HashMap<String, Expression>,
) -> Expression {
    // Canonicalize mapping keys once.
    let canonical: HashMap<String, &Expression> = mapping
        .iter()
        .map(|(k, v)| (k.to_ascii_uppercase(), v))
        .collect();
    let mut shadowed = HashSet::new();
    substitute_inner(expr, &canonical, &mut shadowed)
}

fn substitute_inner(
    expr: &Expression,
    mapping: &HashMap<String, &Expression>,
    shadowed: &mut HashSet<String>,
) -> Expression {
    match expr {
        Expression::Variable(name) => {
            let key = name.to_ascii_uppercase();
            if shadowed.contains(&key) {
                expr.clone()
            } else {
                mapping
                    .get(&key)
                    .map(|e| (*e).clone())
                    .unwrap_or_else(|| expr.clone())
            }
        }
        Expression::Lambda { params, body } => {
            let added: Vec<String> = params.iter().map(|p| p.to_ascii_uppercase()).collect();
            for name in &added {
                shadowed.insert(name.clone());
            }
            let new_body = Box::new(substitute_inner(body, mapping, shadowed));
            for name in &added {
                shadowed.remove(name);
            }
            Expression::Lambda {
                params: params.clone(),
                body: new_body,
            }
        }
        Expression::ListComprehension {
            variable,
            source,
            filter,
            map,
        } => {
            // `source` is evaluated outside the binding; `filter`/`map` see it.
            let new_source = Box::new(substitute_inner(source, mapping, shadowed));
            let key = variable.to_ascii_uppercase();
            let was_shadowed = !shadowed.insert(key.clone());
            let new_filter = filter
                .as_ref()
                .map(|e| Box::new(substitute_inner(e, mapping, shadowed)));
            let new_map = map
                .as_ref()
                .map(|e| Box::new(substitute_inner(e, mapping, shadowed)));
            if !was_shadowed {
                shadowed.remove(&key);
            }
            Expression::ListComprehension {
                variable: variable.clone(),
                source: new_source,
                filter: new_filter,
                map: new_map,
            }
        }
        Expression::Reduce {
            accumulator,
            initial,
            variable,
            source,
            mapping: mapping_expr,
        } => {
            // `initial` and `source` are evaluated outside the bindings.
            let new_initial = Box::new(substitute_inner(initial, mapping, shadowed));
            let new_source = Box::new(substitute_inner(source, mapping, shadowed));
            let acc_key = accumulator.to_ascii_uppercase();
            let var_key = variable.to_ascii_uppercase();
            let acc_was = !shadowed.insert(acc_key.clone());
            let var_was = !shadowed.insert(var_key.clone());
            let new_mapping = Box::new(substitute_inner(mapping_expr, mapping, shadowed));
            if !acc_was {
                shadowed.remove(&acc_key);
            }
            if !var_was {
                shadowed.remove(&var_key);
            }
            Expression::Reduce {
                accumulator: accumulator.clone(),
                initial: new_initial,
                variable: variable.clone(),
                source: new_source,
                mapping: new_mapping,
            }
        }
        Expression::Binary { left, op, right } => Expression::Binary {
            left: Box::new(substitute_inner(left, mapping, shadowed)),
            op: *op,
            right: Box::new(substitute_inner(right, mapping, shadowed)),
        },
        Expression::Unary { op, operand } => Expression::Unary {
            op: *op,
            operand: Box::new(substitute_inner(operand, mapping, shadowed)),
        },
        Expression::Function { name, args } => Expression::Function {
            name: name.clone(),
            args: args
                .iter()
                .map(|a| match a {
                    FunctionArg::Positional(e) => {
                        FunctionArg::Positional(substitute_inner(e, mapping, shadowed))
                    }
                    FunctionArg::Named { name, value } => FunctionArg::Named {
                        name: name.clone(),
                        value: substitute_inner(value, mapping, shadowed),
                    },
                })
                .collect(),
        },
        Expression::Aggregate {
            func,
            args,
            distinct,
            filter,
        } => Expression::Aggregate {
            func: func.clone(),
            args: args
                .iter()
                .map(|e| substitute_inner(e, mapping, shadowed))
                .collect(),
            distinct: *distinct,
            filter: filter
                .as_ref()
                .map(|e| Box::new(substitute_inner(e, mapping, shadowed))),
        },
        Expression::Property { object, property } => Expression::Property {
            object: Box::new(substitute_inner(object, mapping, shadowed)),
            property: property.clone(),
        },
        Expression::StructField { base, field } => Expression::StructField {
            base: Box::new(substitute_inner(base, mapping, shadowed)),
            field: field.clone(),
        },
        Expression::LabelTagProperty { tag, property } => Expression::LabelTagProperty {
            tag: Box::new(substitute_inner(tag, mapping, shadowed)),
            property: property.clone(),
        },
        Expression::Predicate { func, args } => Expression::Predicate {
            func: func.clone(),
            args: args
                .iter()
                .map(|e| substitute_inner(e, mapping, shadowed))
                .collect(),
        },
        Expression::Subscript { collection, index } => Expression::Subscript {
            collection: Box::new(substitute_inner(collection, mapping, shadowed)),
            index: Box::new(substitute_inner(index, mapping, shadowed)),
        },
        Expression::List(items) => Expression::List(
            items
                .iter()
                .map(|e| substitute_inner(e, mapping, shadowed))
                .collect(),
        ),
        Expression::Map(entries) => Expression::Map(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), substitute_inner(v, mapping, shadowed)))
                .collect(),
        ),
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => Expression::Case {
            test_expr: test_expr
                .as_ref()
                .map(|e| Box::new(substitute_inner(e, mapping, shadowed))),
            conditions: conditions
                .iter()
                .map(|(w, t)| {
                    (
                        substitute_inner(w, mapping, shadowed),
                        substitute_inner(t, mapping, shadowed),
                    )
                })
                .collect(),
            default: default
                .as_ref()
                .map(|e| Box::new(substitute_inner(e, mapping, shadowed))),
        },
        Expression::TypeCast {
            expression,
            target_type,
        } => Expression::TypeCast {
            expression: Box::new(substitute_inner(expression, mapping, shadowed)),
            target_type: target_type.clone(),
        },
        Expression::WindowFunction {
            name,
            args,
            over_partition_by,
            over_order_by,
            over_order_desc,
        } => Expression::WindowFunction {
            name: name.clone(),
            args: args
                .iter()
                .map(|e| substitute_inner(e, mapping, shadowed))
                .collect(),
            over_partition_by: over_partition_by
                .iter()
                .map(|e| substitute_inner(e, mapping, shadowed))
                .collect(),
            over_order_by: over_order_by
                .iter()
                .map(|e| substitute_inner(e, mapping, shadowed))
                .collect(),
            over_order_desc: over_order_desc.clone(),
        },
        Expression::Path(items) => Expression::Path(
            items
                .iter()
                .map(|e| substitute_inner(e, mapping, shadowed))
                .collect(),
        ),
        Expression::PathBuild(items) => Expression::PathBuild(
            items
                .iter()
                .map(|e| substitute_inner(e, mapping, shadowed))
                .collect(),
        ),
        Expression::Range {
            collection,
            start,
            end,
        } => Expression::Range {
            collection: Box::new(substitute_inner(collection, mapping, shadowed)),
            start: start
                .as_ref()
                .map(|e| Box::new(substitute_inner(e, mapping, shadowed))),
            end: end
                .as_ref()
                .map(|e| Box::new(substitute_inner(e, mapping, shadowed))),
        },
        // Subquery bodies, literals, parameters, session variables and all
        // leaf nodes are left untouched: macro parameters must not leak into
        // nested query scopes, and closed leaves have no variables.
        _ => expr.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::operators::BinaryOperator;
    use crate::Value;

    fn var(name: &str) -> Expression {
        Expression::Variable(name.to_string())
    }

    fn lit(n: i32) -> Expression {
        Expression::Literal(Value::Int(n))
    }

    #[test]
    fn test_substitute_basic() {
        let expr = Expression::Binary {
            left: Box::new(var("x")),
            op: BinaryOperator::Multiply,
            right: Box::new(var("y")),
        };
        let mut mapping = HashMap::new();
        mapping.insert("x".to_string(), lit(21));
        let out = substitute_variables(&expr, &mapping);
        match out {
            Expression::Binary { left, right, .. } => {
                assert_eq!(*left, lit(21));
                assert_eq!(*right, var("y"));
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn test_substitute_case_insensitive() {
        let mut mapping = HashMap::new();
        mapping.insert("X".to_string(), lit(1));
        assert_eq!(substitute_variables(&var("x"), &mapping), lit(1));
    }

    #[test]
    fn test_substitute_lambda_shadowing() {
        let expr = Expression::Lambda {
            params: vec!["x".to_string()],
            body: Box::new(var("x")),
        };
        let mut mapping = HashMap::new();
        mapping.insert("x".to_string(), lit(9));
        let out = substitute_variables(&expr, &mapping);
        // Bound lambda parameter must not be captured.
        assert_eq!(out, expr);
    }

    #[test]
    fn test_substitute_no_capture_in_comprehension_source() {
        // `source` is outside the binding: substitution applies there.
        let expr = Expression::ListComprehension {
            variable: "x".to_string(),
            source: Box::new(var("x")),
            filter: None,
            map: Some(Box::new(var("x"))),
        };
        let mut mapping = HashMap::new();
        mapping.insert("x".to_string(), lit(7));
        match substitute_variables(&expr, &mapping) {
            Expression::ListComprehension { source, map, .. } => {
                assert_eq!(*source, lit(7));
                assert_eq!(map.unwrap().as_ref(), &var("x"));
            }
            other => panic!("unexpected {:?}", other),
        }
    }
}
