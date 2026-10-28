//! Join-key extraction for pattern sides the pattern graph leaves open.
//!
//! Two MATCH patterns without a relationship between them are combined with a
//! cartesian product. When the WHERE clause equates a column of one side with
//! a column of the other, that equality is a join condition: using it as such
//! keeps the combination from degenerating into the full cartesian product,
//! which the executor would otherwise have to materialize.

use std::collections::HashSet;
use std::sync::Arc;

use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::expr::ExpressionMeta;
use linkrs_core::types::operators::BinaryOperator;
use linkrs_core::types::Expression;

use crate::planning::scan_predicate::split_conjuncts;

/// Whether the expression reads only variables bound by `vars`.
///
/// An expression without variables (a literal) never qualifies: it relates
/// no side to the other.
fn reads_only(expr: &Expression, vars: &HashSet<String>) -> bool {
    let used = expr.get_variables();
    !used.is_empty() && used.iter().all(|name| vars.contains(name))
}

/// Whether the expression embeds a subquery.
///
/// Subqueries are compiled by the WHERE clause planner; a join key must stay
/// a plain expression so the hash join can evaluate it per row.
fn contains_subquery(expr: &Expression) -> bool {
    if matches!(
        expr,
        Expression::Exists { .. }
            | Expression::In { .. }
            | Expression::CountSubquery { .. }
            | Expression::ScalarSubquery { .. }
    ) {
        return true;
    }
    expr.children().iter().any(|child| contains_subquery(child))
}

/// Register an expression in the shared analysis context.
fn to_contextual(
    expr: Expression,
    context: &Arc<ExpressionAnalysisContext>,
) -> ContextualExpression {
    let id = context.register_expression(ExpressionMeta::new(expr));
    ContextualExpression::new(id, Arc::clone(context))
}

/// Derive the equi-join keys that relate two pattern sides.
///
/// The build keys are evaluated on the right side and the probe keys on the
/// left side, matching the executor's default build side. `None` means the
/// two sides share no equality, so the combination stays a cartesian
/// product.
pub fn equi_join_keys(
    where_clause: Option<&ContextualExpression>,
    left_vars: &HashSet<String>,
    right_vars: &HashSet<String>,
) -> Option<(Vec<ContextualExpression>, Vec<ContextualExpression>)> {
    let clause = where_clause?;
    let expr = clause.expression()?;

    let conjuncts: Vec<Expression> = split_conjuncts(expr.inner()).into_iter().cloned().collect();

    let context = clause.context().clone();
    let mut build_keys = Vec::new();
    let mut probe_keys = Vec::new();
    for conjunct in conjuncts {
        let Expression::Binary {
            left,
            op: BinaryOperator::Equal,
            right,
        } = conjunct
        else {
            continue;
        };
        if contains_subquery(&left) || contains_subquery(&right) {
            continue;
        }
        let (build_side, probe_side) =
            if reads_only(&left, left_vars) && reads_only(&right, right_vars) {
                (&right, &left)
            } else if reads_only(&left, right_vars) && reads_only(&right, left_vars) {
                (&left, &right)
            } else {
                continue;
            };
        build_keys.push(to_contextual(build_side.as_ref().clone(), &context));
        probe_keys.push(to_contextual(probe_side.as_ref().clone(), &context));
    }

    if build_keys.is_empty() {
        return None;
    }
    Some((build_keys, probe_keys))
}

#[cfg(test)]
mod tests {
    use super::*;
    use linkrs_core::types::operators::BinaryOperator;

    fn contextual(expr: Expression) -> ContextualExpression {
        let context = Arc::new(ExpressionAnalysisContext::new());
        to_contextual(expr, &context)
    }

    fn vars(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn property(var: &str, property: &str) -> Expression {
        Expression::Property {
            object: Box::new(Expression::Variable(var.to_string())),
            property: property.to_string(),
        }
    }

    fn equal(left: Expression, right: Expression) -> Expression {
        Expression::Binary {
            left: Box::new(left),
            op: BinaryOperator::Equal,
            right: Box::new(right),
        }
    }

    #[test]
    fn equality_across_the_two_sides_yields_join_keys() {
        let clause = contextual(equal(property("a", "value"), property("b", "value")));
        let keys = equi_join_keys(Some(&clause), &vars(&["a"]), &vars(&["b"]))
            .expect("cross-side equality yields keys");
        let (build_keys, probe_keys) = keys;
        assert_eq!(build_keys.len(), 1);
        assert_eq!(probe_keys.len(), 1);
    }

    #[test]
    fn single_sided_equality_yields_no_keys() {
        let clause = contextual(equal(property("a", "value"), property("a", "other")));
        assert!(equi_join_keys(Some(&clause), &vars(&["a"]), &vars(&["b"])).is_none());
    }

    #[test]
    fn conjunction_keeps_only_the_cross_side_equality() {
        let clause = contextual(Expression::Binary {
            left: Box::new(equal(property("a", "value"), property("b", "value"))),
            op: BinaryOperator::And,
            right: Box::new(equal(property("a", "value"), property("a", "other"))),
        });
        let keys = equi_join_keys(Some(&clause), &vars(&["a"]), &vars(&["b"]))
            .expect("cross-side equality yields keys");
        assert_eq!(keys.0.len(), 1);
        assert_eq!(keys.1.len(), 1);
    }

    #[test]
    fn literal_equality_yields_no_keys() {
        let clause = contextual(equal(
            Expression::Literal(linkrs_core::Value::Int(1)),
            Expression::Literal(linkrs_core::Value::Int(1)),
        ));
        assert!(equi_join_keys(Some(&clause), &vars(&["a"]), &vars(&["b"])).is_none());
    }

    #[test]
    fn missing_where_clause_yields_no_keys() {
        assert!(equi_join_keys(None, &vars(&["a"]), &vars(&["b"])).is_none());
    }
}
