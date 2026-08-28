//! Extraction of storage pushdown predicates from scan-level filter
//! expressions.
//!
//! The optimizer ensures filter conditions sit directly above the scan node
//! (either as a `Filter` node or as the scan's own vertex filter, set by the
//! `PushVFilterDownScanVertices` rule).  This module rewrites the subset of
//! conjuncts that the storage layer can evaluate — single-column scalar
//! comparisons — into [`ScanPredicate`]s.
//!
//! The original filter expression is never modified: the pushed predicate is
//! a pure pre-filter and the full condition still runs on top of the scan.

use crate::storage::ScanPredicate;
use graphdb_core::types::expr::{ContextualExpression, Expression};
use graphdb_core::types::operators::BinaryOperator;
use graphdb_core::Value;

/// Extract pushable conjuncts from a scan-level filter expression.
///
/// `projected` lists the properties the scan already projects.  A conjunct
/// is only pushed when its column is guaranteed to be decoded by the scan
/// (either the projection is empty — full row decode — or the column is
/// already projected), so the storage-side evaluation never sees data the
/// query layer would not have seen.
pub fn extract_scan_predicates(
    condition: Option<&ContextualExpression>,
    projected: &[String],
) -> Vec<ScanPredicate> {
    let Some(condition) = condition else {
        return Vec::new();
    };
    let Some(meta) = condition.expression() else {
        return Vec::new();
    };
    let mut predicates = Vec::new();
    for conjunct in pushable_conjuncts(meta.inner()) {
        let Some((column, op, literal)) = match_comparison(&conjunct) else {
            continue;
        };
        if !projected.is_empty() && !projected.iter().any(|name| name == column) {
            continue;
        }
        predicates.push(build_predicate(column, op, literal));
    }
    predicates
}

/// The conjuncts of `expr` that the storage layer can evaluate as scan
/// predicates (single-column scalar comparisons).
///
/// Shared by the optimizer (which moves these conjuncts onto the scan node
/// as its vertex filter) and the arena builder (which converts them into
/// [`ScanPredicate`]s).
pub fn pushable_conjuncts(expr: &Expression) -> Vec<Expression> {
    split_conjuncts(expr)
        .into_iter()
        .filter(|conjunct| {
            match_comparison(conjunct).is_some_and(|(_, _, literal)| is_scalar_literal(literal))
        })
        .cloned()
        .collect()
}

/// Fold a list of conjuncts into a single AND expression.
pub fn and_of(conjuncts: Vec<Expression>) -> Option<Expression> {
    let mut iter = conjuncts.into_iter();
    let mut acc = iter.next()?;
    for conjunct in iter {
        acc = Expression::Binary {
            left: Box::new(acc),
            op: BinaryOperator::And,
            right: Box::new(conjunct),
        };
    }
    Some(acc)
}

/// Split an expression into top-level `AND` conjuncts.
fn split_conjuncts(expr: &Expression) -> Vec<&Expression> {
    let mut conjuncts = Vec::new();
    collect_conjuncts(expr, &mut conjuncts);
    conjuncts
}

fn collect_conjuncts<'a>(expr: &'a Expression, out: &mut Vec<&'a Expression>) {
    if let Expression::Binary {
        left,
        op: BinaryOperator::And,
        right,
    } = expr
    {
        collect_conjuncts(left, out);
        collect_conjuncts(right, out);
    } else {
        out.push(expr);
    }
}

/// Recognize `column OP literal` / `literal OP column` comparison shapes.
///
/// Returns the column name, the operator, and the literal expression.  Only
/// the comparison operators the storage layer can evaluate are matched.
fn match_comparison(expr: &Expression) -> Option<(&str, BinaryOperator, &Expression)> {
    let Expression::Binary { left, op, right } = expr else {
        return None;
    };
    if !is_comparison_op(*op) {
        return None;
    }
    match (property_column(left), as_literal(right)) {
        (Some(column), Some(literal)) => Some((column, *op, literal)),
        (None, None) => match (property_column(right), as_literal(left)) {
            (Some(column), Some(literal)) => Some((column, swapped(*op)?, literal)),
            _ => None,
        },
        _ => None,
    }
}

fn is_comparison_op(op: BinaryOperator) -> bool {
    matches!(
        op,
        BinaryOperator::Equal
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterThanOrEqual
            | BinaryOperator::LessThan
            | BinaryOperator::LessThanOrEqual
    )
}

/// Extract the property name of a `var.property` expression.
fn property_column(expr: &Expression) -> Option<&str> {
    match expr {
        Expression::Property { object, property } => match object.as_ref() {
            Expression::Variable(_) => Some(property.as_str()),
            _ => None,
        },
        _ => None,
    }
}

fn as_literal(expr: &Expression) -> Option<&Expression> {
    matches!(expr, Expression::Literal(_)).then_some(expr)
}

/// The operator of `literal OP column` reads back-to-front.
fn swapped(op: BinaryOperator) -> Option<BinaryOperator> {
    match op {
        BinaryOperator::Equal => Some(BinaryOperator::Equal),
        BinaryOperator::LessThan => Some(BinaryOperator::GreaterThan),
        BinaryOperator::LessThanOrEqual => Some(BinaryOperator::GreaterThanOrEqual),
        BinaryOperator::GreaterThan => Some(BinaryOperator::LessThan),
        BinaryOperator::GreaterThanOrEqual => Some(BinaryOperator::LessThanOrEqual),
        _ => None,
    }
}

/// Only scalar literals are pushable (no lists, maps, or vertex values).
fn is_scalar_literal(expr: &Expression) -> bool {
    match expr {
        Expression::Literal(value) => matches!(
            value,
            Value::SmallInt(_)
                | Value::Int(_)
                | Value::BigInt(_)
                | Value::Float(_)
                | Value::Double(_)
                | Value::String(_)
                | Value::FixedString(_)
                | Value::Bool(_)
        ),
        _ => false,
    }
}

fn build_predicate(column: &str, op: BinaryOperator, literal: &Expression) -> ScanPredicate {
    let Expression::Literal(value) = literal else {
        unreachable!("literal checked by is_scalar_literal")
    };
    match op {
        BinaryOperator::Equal => ScanPredicate::ColumnEqual {
            column: column.to_string(),
            value: value.clone(),
        },
        BinaryOperator::GreaterThan => ScanPredicate::ColumnRange {
            column: column.to_string(),
            lower: Some(value.clone()),
            upper: None,
            include_lower: false,
            include_upper: false,
        },
        BinaryOperator::GreaterThanOrEqual => ScanPredicate::ColumnRange {
            column: column.to_string(),
            lower: Some(value.clone()),
            upper: None,
            include_lower: true,
            include_upper: false,
        },
        BinaryOperator::LessThan => ScanPredicate::ColumnRange {
            column: column.to_string(),
            lower: None,
            upper: Some(value.clone()),
            include_lower: false,
            include_upper: false,
        },
        BinaryOperator::LessThanOrEqual => ScanPredicate::ColumnRange {
            column: column.to_string(),
            lower: None,
            upper: Some(value.clone()),
            include_lower: false,
            include_upper: true,
        },
        _ => unreachable!("comparison operators only"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::expr::ExpressionMeta;

    fn lit(value: Value) -> Expression {
        Expression::Literal(value)
    }

    fn prop(property: &str) -> Expression {
        Expression::Property {
            object: Box::new(Expression::Variable("v".to_string())),
            property: property.to_string(),
        }
    }

    fn bin(left: Expression, op: BinaryOperator, right: Expression) -> Expression {
        Expression::Binary {
            left: Box::new(left),
            op,
            right: Box::new(right),
        }
    }

    fn contextual(expr: Expression) -> ContextualExpression {
        let ctx = std::sync::Arc::new(graphdb_core::types::expr::ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    #[test]
    fn extract_equality_conjunct() {
        let expr = bin(prop("age"), BinaryOperator::Equal, lit(Value::Int(30)));
        let predicates = extract_scan_predicates(Some(&contextual(expr)), &["age".to_string()]);
        assert_eq!(predicates.len(), 1);
        assert_eq!(predicates[0].column(), "age");
        assert!(predicates[0].matches(&[("age".to_string(), Value::Int(30))]));
        assert!(!predicates[0].matches(&[("age".to_string(), Value::Int(31))]));
    }

    #[test]
    fn extract_range_conjuncts() {
        let expr = bin(
            prop("age"),
            BinaryOperator::GreaterThan,
            lit(Value::Int(18)),
        );
        let predicates = extract_scan_predicates(Some(&contextual(expr)), &["age".to_string()]);
        assert_eq!(predicates.len(), 1);
        assert!(predicates[0].matches(&[("age".to_string(), Value::Int(19))]));
        assert!(!predicates[0].matches(&[("age".to_string(), Value::Int(18))]));
    }

    #[test]
    fn extract_combined_range() {
        let expr = bin(
            bin(
                prop("age"),
                BinaryOperator::GreaterThan,
                lit(Value::Int(18)),
            ),
            BinaryOperator::And,
            bin(prop("age"), BinaryOperator::LessThan, lit(Value::Int(30))),
        );
        let predicates = extract_scan_predicates(Some(&contextual(expr)), &["age".to_string()]);
        assert_eq!(predicates.len(), 2);
        assert!(predicates[0].matches(&[("age".to_string(), Value::Int(25))]));
        assert!(predicates[1].matches(&[("age".to_string(), Value::Int(25))]));
        assert!(!predicates[0].matches(&[("age".to_string(), Value::Int(10))]));
        assert!(!predicates[1].matches(&[("age".to_string(), Value::Int(40))]));
    }

    #[test]
    fn extract_literal_on_left() {
        let expr = bin(
            lit(Value::Int(30)),
            BinaryOperator::GreaterThan,
            prop("age"),
        );
        let predicates = extract_scan_predicates(Some(&contextual(expr)), &["age".to_string()]);
        assert_eq!(predicates.len(), 1);
        assert!(predicates[0].matches(&[("age".to_string(), Value::Int(25))]));
        assert!(!predicates[0].matches(&[("age".to_string(), Value::Int(35))]));
    }

    #[test]
    fn skip_unpushable_conjuncts() {
        let expr = bin(
            prop("name"),
            BinaryOperator::NotEqual,
            lit(Value::string("bob")),
        );
        let predicates = extract_scan_predicates(Some(&contextual(expr)), &["name".to_string()]);
        assert!(predicates.is_empty());

        let expr = bin(
            prop("age"),
            BinaryOperator::Equal,
            Expression::Function {
                name: "abs".to_string(),
                args: vec![lit(Value::Int(30))],
            },
        );
        let predicates = extract_scan_predicates(Some(&contextual(expr)), &["age".to_string()]);
        assert!(predicates.is_empty());
    }

    #[test]
    fn skip_unprojected_columns() {
        let expr = bin(prop("age"), BinaryOperator::Equal, lit(Value::Int(30)));
        let predicates = extract_scan_predicates(Some(&contextual(expr)), &["name".to_string()]);
        assert!(predicates.is_empty());
    }

    #[test]
    fn missing_column_never_matches() {
        let predicate = ScanPredicate::ColumnEqual {
            column: "age".to_string(),
            value: Value::Int(30),
        };
        assert!(!predicate.matches(&[("name".to_string(), Value::string("bob"))]));
    }

    #[test]
    fn cross_numeric_comparison() {
        let predicate = ScanPredicate::ColumnRange {
            column: "age".to_string(),
            lower: Some(Value::BigInt(18)),
            upper: None,
            include_lower: true,
            include_upper: false,
        };
        assert!(predicate.matches(&[("age".to_string(), Value::Double(18.5))]));
        assert!(!predicate.matches(&[("age".to_string(), Value::Double(17.9))]));
    }
}
