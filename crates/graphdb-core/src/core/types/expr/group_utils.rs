//! Grouping Utility Functions
//!
//! Provides types and functions for GROUP BY processing, including:
//! - GroupSuite for grouping keys, items, and aggregates
//! - Extraction of grouping information from expressions

use crate::core::types::expr::Expression;

/// Group Suite for GROUP BY processing
///
/// Contains grouping keys, group items, and aggregate functions
/// extracted from expressions for query optimization.
#[derive(Debug, Clone, Default)]
pub struct GroupSuite {
    /// Set of grouping keys
    pub group_keys: Vec<Expression>,
    /// Collection of group items
    pub group_items: Vec<Expression>,
    /// Collection of aggregate functions
    pub aggregates: Vec<Expression>,
}

impl GroupSuite {
    /// Create a new empty GroupSuite
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a grouping key if not already present
    pub fn add_group_key(&mut self, expression: Expression) {
        if !self.group_keys.contains(&expression) {
            self.group_keys.push(expression);
        }
    }

    /// Add a group item if not already present
    pub fn add_group_item(&mut self, expression: Expression) {
        if !self.group_items.contains(&expression) {
            self.group_items.push(expression);
        }
    }

    /// Add an aggregate expression if not already present
    pub fn add_aggregate(&mut self, expression: Expression) {
        if !self.aggregates.contains(&expression) {
            self.aggregates.push(expression);
        }
    }

    /// Check if the suite is empty
    pub fn is_empty(&self) -> bool {
        self.group_keys.is_empty() && self.group_items.is_empty() && self.aggregates.is_empty()
    }

    /// Union with another GroupSuite
    pub fn union(&mut self, other: &GroupSuite) {
        for key in &other.group_keys {
            self.add_group_key(key.clone());
        }
        for item in &other.group_items {
            self.add_group_item(item.clone());
        }
        for agg in &other.aggregates {
            self.add_aggregate(agg.clone());
        }
    }
}

/// Extract the grouping suite from the expression
///
/// Used for GROUP BY optimization; identifies expressions and aggregate functions
/// that can be used for grouping.
///
/// # Parameters
/// - `expression`: Expression to be analyzed
///
/// # Returns
/// - `Ok(GroupSuite)`: Extracted grouping suite
/// - `Err(String)`: Error message
pub fn extract_group_suite(expression: &Expression) -> Result<GroupSuite, String> {
    let mut group_suite = GroupSuite::new();
    extract_group_suite_recursive(expression, &mut group_suite);
    Ok(group_suite)
}

/// Recursive helper function for extracting group suite
fn extract_group_suite_recursive(expression: &Expression, group_suite: &mut GroupSuite) {
    match expression {
        Expression::Literal(value) => {
            group_suite.add_group_key(Expression::Literal(value.clone()));
        }
        Expression::Variable(name) => {
            group_suite.add_group_key(Expression::Variable(name.clone()));
        }
        Expression::Property { object, property } => {
            let prop_expression = Expression::Property {
                object: Box::new(object.as_ref().clone()),
                property: property.clone(),
            };
            group_suite.add_group_key(prop_expression);
            extract_group_suite_recursive(object, group_suite);
        }
        Expression::Binary { left, right, .. } => {
            if is_groupable(left) {
                group_suite.add_group_key(left.as_ref().clone());
            }
            if is_groupable(right) {
                group_suite.add_group_key(right.as_ref().clone());
            }
            extract_group_suite_recursive(left, group_suite);
            extract_group_suite_recursive(right, group_suite);
        }
        Expression::Unary { operand, .. } => {
            if is_groupable(operand) {
                group_suite.add_group_key(operand.as_ref().clone());
            }
            extract_group_suite_recursive(operand, group_suite);
        }
        Expression::Function { name, args } => {
            let name_upper = name.to_uppercase();
            if matches!(name_upper.as_str(), "ID" | "SRC" | "DST") && args.len() == 1 {
                let func_expression = Expression::Function {
                    name: name.clone(),
                    args: args.clone(),
                };
                group_suite.add_group_key(func_expression);
            }
            for arg in args {
                extract_group_suite_recursive(arg, group_suite);
            }
        }
        Expression::Aggregate {
            func,
            arg,
            distinct,
        } => {
            let agg_expression = Expression::Aggregate {
                func: func.clone(),
                arg: Box::new(arg.as_ref().clone()),
                distinct: *distinct,
            };
            group_suite.add_aggregate(agg_expression);
            extract_group_suite_recursive(arg, group_suite);
        }
        Expression::List(items) => {
            for item in items {
                extract_group_suite_recursive(item, group_suite);
            }
        }
        Expression::Map(pairs) => {
            for (_, expr) in pairs {
                extract_group_suite_recursive(expr, group_suite);
            }
        }
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            if let Some(test) = test_expr {
                extract_group_suite_recursive(test, group_suite);
            }
            for (cond, expr) in conditions {
                extract_group_suite_recursive(cond, group_suite);
                extract_group_suite_recursive(expr, group_suite);
            }
            if let Some(def) = default {
                extract_group_suite_recursive(def, group_suite);
            }
        }
        Expression::TypeCast { expression, .. } => {
            extract_group_suite_recursive(expression, group_suite);
        }
        Expression::Subscript { collection, index } => {
            extract_group_suite_recursive(collection, group_suite);
            extract_group_suite_recursive(index, group_suite);
        }
        Expression::Range {
            collection,
            start,
            end,
        } => {
            extract_group_suite_recursive(collection, group_suite);
            if let Some(s) = start {
                extract_group_suite_recursive(s, group_suite);
            }
            if let Some(e) = end {
                extract_group_suite_recursive(e, group_suite);
            }
        }
        Expression::Path(items) => {
            for item in items {
                extract_group_suite_recursive(item, group_suite);
            }
        }
        Expression::Label(name) => {
            group_suite.add_group_key(Expression::Label(name.clone()));
        }
        Expression::ListComprehension {
            variable,
            source,
            filter,
            map,
        } => {
            group_suite.add_group_key(Expression::Variable(variable.clone()));
            extract_group_suite_recursive(source, group_suite);
            if let Some(f) = filter {
                extract_group_suite_recursive(f, group_suite);
            }
            if let Some(m) = map {
                extract_group_suite_recursive(m, group_suite);
            }
        }
        Expression::Parameter(name) => {
            group_suite.add_group_key(Expression::Parameter(name.clone()));
        }
        _ => {}
    }
}

/// Check whether the expression is a groupable expression
fn is_groupable(expression: &Expression) -> bool {
    matches!(
        expression,
        Expression::Literal(_)
            | Expression::Variable(_)
            | Expression::Property { .. }
            | Expression::Function { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::{AggregateFunction, BinaryOperator};

    #[test]
    fn test_group_suite_new() {
        let suite = GroupSuite::new();
        assert!(suite.is_empty());
        assert_eq!(suite.group_keys.len(), 0);
        assert_eq!(suite.group_items.len(), 0);
        assert_eq!(suite.aggregates.len(), 0);
    }

    #[test]
    fn test_group_suite_add_group_key() {
        let mut suite = GroupSuite::new();
        let expr = Expression::Variable("a".to_string());

        suite.add_group_key(expr.clone());
        assert!(!suite.is_empty());
        assert_eq!(suite.group_keys.len(), 1);

        // Adding duplicate should not increase count
        suite.add_group_key(expr);
        assert_eq!(suite.group_keys.len(), 1);
    }

    #[test]
    fn test_group_suite_add_group_item() {
        let mut suite = GroupSuite::new();
        let expr = Expression::Variable("b".to_string());

        suite.add_group_item(expr.clone());
        assert_eq!(suite.group_items.len(), 1);

        // Adding duplicate should not increase count
        suite.add_group_item(expr);
        assert_eq!(suite.group_items.len(), 1);
    }

    #[test]
    fn test_group_suite_add_aggregate() {
        let mut suite = GroupSuite::new();
        let expr = Expression::Aggregate {
            func: AggregateFunction::Count(None),
            arg: Box::new(Expression::Variable("x".to_string())),
            distinct: false,
        };

        suite.add_aggregate(expr.clone());
        assert_eq!(suite.aggregates.len(), 1);

        // Adding duplicate should not increase count
        suite.add_aggregate(expr);
        assert_eq!(suite.aggregates.len(), 1);
    }

    #[test]
    fn test_group_suite_union() {
        let mut suite1 = GroupSuite::new();
        suite1.add_group_key(Expression::Variable("a".to_string()));

        let mut suite2 = GroupSuite::new();
        suite2.add_group_key(Expression::Variable("b".to_string()));
        suite2.add_group_item(Expression::Variable("c".to_string()));

        suite1.union(&suite2);
        assert_eq!(suite1.group_keys.len(), 2);
        assert_eq!(suite1.group_items.len(), 1);
    }

    #[test]
    fn test_extract_group_suite_simple() {
        let expr = Expression::Binary {
            op: BinaryOperator::Add,
            left: Box::new(Expression::Variable("a".to_string())),
            right: Box::new(Expression::Variable("b".to_string())),
        };

        let suite = extract_group_suite(&expr).expect("Failed to extract group suite");
        assert_eq!(suite.group_keys.len(), 2);
    }

    #[test]
    fn test_extract_group_suite_with_aggregate() {
        let expr = Expression::Binary {
            op: BinaryOperator::Add,
            left: Box::new(Expression::Aggregate {
                func: AggregateFunction::Count(None),
                arg: Box::new(Expression::Variable("x".to_string())),
                distinct: false,
            }),
            right: Box::new(Expression::Variable("y".to_string())),
        };

        let suite = extract_group_suite(&expr).expect("Failed to extract group suite");
        assert_eq!(suite.aggregates.len(), 1);
        assert_eq!(suite.group_keys.len(), 2); // x from aggregate arg, y from right side
    }
}
