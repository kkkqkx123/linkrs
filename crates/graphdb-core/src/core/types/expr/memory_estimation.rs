//! Memory estimation for Expression types
//!
//! This module provides memory estimation for the Expression enum and related types.

use crate::core::types::expr::Expression;
use crate::core::types::memory_estimation::MemoryEstimatable;
use crate::core::value::Value;

impl MemoryEstimatable for Expression {
    fn estimate_memory(&self) -> usize {
        let base_size = std::mem::size_of::<Expression>();

        match self {
            // Leaf nodes: base size + data size
            Expression::Literal(value) => base_size + estimate_value_memory(value),
            Expression::Variable(name) => base_size + estimate_string_memory(name),
            Expression::Label(name) => base_size + estimate_string_memory(name),
            Expression::Parameter(name) => base_size + estimate_string_memory(name),

            // Unary operations: base size + operand
            Expression::Unary { operand, .. } => base_size + operand.estimate_memory(),
            Expression::TypeCast { expression, .. } => base_size + expression.estimate_memory(),
            Expression::Aggregate { arg, .. } => base_size + arg.estimate_memory(),

            // Binary operations: base size + two operands
            Expression::Binary { left, right, .. } => {
                base_size + left.estimate_memory() + right.estimate_memory()
            }
            Expression::Subscript { collection, index } => {
                base_size + collection.estimate_memory() + index.estimate_memory()
            }

            // Property access
            Expression::Property { object, property } => {
                base_size + object.estimate_memory() + estimate_string_memory(property)
            }
            Expression::TagProperty { tag_name, property } => {
                base_size + estimate_string_memory(tag_name) + estimate_string_memory(property)
            }
            Expression::EdgeProperty {
                edge_name,
                property,
            } => base_size + estimate_string_memory(edge_name) + estimate_string_memory(property),
            Expression::LabelTagProperty { tag, property } => {
                base_size + tag.estimate_memory() + estimate_string_memory(property)
            }

            // Collection types
            Expression::List(items) => {
                base_size + items.iter().map(|e| e.estimate_memory()).sum::<usize>()
            }
            Expression::Map(entries) => {
                base_size
                    + entries
                        .iter()
                        .map(|(k, v)| estimate_string_memory(k) + v.estimate_memory())
                        .sum::<usize>()
            }
            Expression::Path(items) => {
                base_size + items.iter().map(|e| e.estimate_memory()).sum::<usize>()
            }
            Expression::PathBuild(items) => {
                base_size + items.iter().map(|e| e.estimate_memory()).sum::<usize>()
            }

            // Function calls
            Expression::Function { name, args } => {
                base_size
                    + estimate_string_memory(name)
                    + args.iter().map(|e| e.estimate_memory()).sum::<usize>()
            }
            Expression::Predicate { func, args } => {
                base_size
                    + estimate_string_memory(func)
                    + args.iter().map(|e| e.estimate_memory()).sum::<usize>()
            }

            // Conditional expression
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let test_size = test_expr.as_ref().map(|e| e.estimate_memory()).unwrap_or(0);
                let conditions_size = conditions
                    .iter()
                    .map(|(cond, result)| cond.estimate_memory() + result.estimate_memory())
                    .sum::<usize>();
                let default_size = default.as_ref().map(|e| e.estimate_memory()).unwrap_or(0);
                base_size + test_size + conditions_size + default_size
            }

            // Range expression
            Expression::Range {
                collection,
                start,
                end,
            } => {
                let start_size = start.as_ref().map(|e| e.estimate_memory()).unwrap_or(0);
                let end_size = end.as_ref().map(|e| e.estimate_memory()).unwrap_or(0);
                base_size + collection.estimate_memory() + start_size + end_size
            }

            // List comprehension
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => {
                let filter_size = filter.as_ref().map(|e| e.estimate_memory()).unwrap_or(0);
                let map_size = map.as_ref().map(|e| e.estimate_memory()).unwrap_or(0);
                base_size
                    + estimate_string_memory(variable)
                    + source.estimate_memory()
                    + filter_size
                    + map_size
            }

            // Reduce expression
            Expression::Reduce {
                accumulator,
                initial,
                variable,
                source,
                mapping,
            } => {
                base_size
                    + estimate_string_memory(accumulator)
                    + estimate_string_memory(variable)
                    + initial.estimate_memory()
                    + source.estimate_memory()
                    + mapping.estimate_memory()
            }
            Expression::Vector(data) => base_size + data.len() * std::mem::size_of::<f32>(),
        }
    }
}

/// Helper function to estimate String memory
fn estimate_string_memory(s: &String) -> usize {
    std::mem::size_of::<String>() + s.capacity()
}

/// Helper function to estimate Value memory
fn estimate_value_memory(value: &Value) -> usize {
    use crate::core::types::memory_estimation::MemoryEstimatable;
    value.estimate_memory()
}

/// Helper function to estimate memory for a slice of Expressions
pub fn estimate_expressions_memory(expressions: &[Expression]) -> usize {
    expressions.iter().map(|e| e.estimate_memory()).sum()
}

/// Helper function to estimate memory for an optional boxed Expression
pub fn estimate_option_box_memory(expr: &Option<Box<Expression>>) -> usize {
    expr.as_ref()
        .map(|e| std::mem::size_of::<Box<Expression>>() + e.estimate_memory())
        .unwrap_or(0)
}

impl Expression {
    /// Estimate memory using iterative approach to avoid stack overflow
    /// This is useful for deeply nested expressions
    pub fn estimate_memory_iterative(&self) -> usize {
        let mut total = std::mem::size_of::<Expression>();
        let mut stack: Vec<&Expression> = vec![self];

        while let Some(expr) = stack.pop() {
            match expr {
                // Leaf nodes: only count data size (base already counted)
                Expression::Literal(value) => {
                    total += estimate_value_memory(value) - std::mem::size_of::<Value>();
                }
                Expression::Variable(name)
                | Expression::Label(name)
                | Expression::Parameter(name) => {
                    total += name.capacity();
                }

                // Unary operations: add operand
                Expression::Unary { operand, .. } => {
                    total += std::mem::size_of::<Expression>();
                    stack.push(operand);
                }
                Expression::TypeCast { expression, .. } => {
                    total += std::mem::size_of::<Expression>();
                    stack.push(expression);
                }
                Expression::Aggregate { arg, .. } => {
                    total += std::mem::size_of::<Expression>();
                    stack.push(arg);
                }

                // Binary operations: add two operands
                Expression::Binary { left, right, .. } => {
                    total += std::mem::size_of::<Expression>() * 2;
                    stack.push(left);
                    stack.push(right);
                }
                Expression::Subscript { collection, index } => {
                    total += std::mem::size_of::<Expression>() * 2;
                    stack.push(collection);
                    stack.push(index);
                }

                // Property access
                Expression::Property { object, property } => {
                    total += std::mem::size_of::<Expression>() + property.capacity();
                    stack.push(object);
                }
                Expression::TagProperty { tag_name, property } => {
                    total += tag_name.capacity() + property.capacity();
                }
                Expression::EdgeProperty {
                    edge_name,
                    property,
                } => {
                    total += edge_name.capacity() + property.capacity();
                }
                Expression::LabelTagProperty { tag, property } => {
                    total += std::mem::size_of::<Expression>() + property.capacity();
                    stack.push(tag);
                }

                // Collection types
                Expression::List(items) => {
                    total += items.len() * std::mem::size_of::<Expression>();
                    for item in items {
                        stack.push(item);
                    }
                }
                Expression::Map(entries) => {
                    total += entries.len()
                        * (std::mem::size_of::<String>() + std::mem::size_of::<Expression>());
                    for (k, v) in entries {
                        total += k.capacity();
                        stack.push(v);
                    }
                }
                Expression::Path(items) | Expression::PathBuild(items) => {
                    total += items.len() * std::mem::size_of::<Expression>();
                    for item in items {
                        stack.push(item);
                    }
                }

                // Function calls
                Expression::Function { name, args } => {
                    total += name.capacity() + args.len() * std::mem::size_of::<Expression>();
                    for arg in args {
                        stack.push(arg);
                    }
                }
                Expression::Predicate { func, args } => {
                    total += func.capacity() + args.len() * std::mem::size_of::<Expression>();
                    for arg in args {
                        stack.push(arg);
                    }
                }

                // Conditional expression
                Expression::Case {
                    test_expr,
                    conditions,
                    default,
                } => {
                    if let Some(test) = test_expr {
                        total += std::mem::size_of::<Box<Expression>>();
                        stack.push(test);
                    }
                    total += conditions.len() * std::mem::size_of::<(Expression, Expression)>();
                    for (cond, result) in conditions {
                        stack.push(cond);
                        stack.push(result);
                    }
                    if let Some(def) = default {
                        total += std::mem::size_of::<Box<Expression>>();
                        stack.push(def);
                    }
                }

                // Range expression
                Expression::Range {
                    collection,
                    start,
                    end,
                } => {
                    total += std::mem::size_of::<Expression>();
                    stack.push(collection);
                    if let Some(s) = start {
                        total += std::mem::size_of::<Box<Expression>>();
                        stack.push(s);
                    }
                    if let Some(e) = end {
                        total += std::mem::size_of::<Box<Expression>>();
                        stack.push(e);
                    }
                }

                // List comprehension
                Expression::ListComprehension {
                    variable,
                    source,
                    filter,
                    map,
                } => {
                    total += variable.capacity() + std::mem::size_of::<Expression>();
                    stack.push(source);
                    if let Some(f) = filter {
                        total += std::mem::size_of::<Box<Expression>>();
                        stack.push(f);
                    }
                    if let Some(m) = map {
                        total += std::mem::size_of::<Box<Expression>>();
                        stack.push(m);
                    }
                }

                // Reduce expression
                Expression::Reduce {
                    accumulator,
                    initial,
                    variable,
                    source,
                    mapping,
                } => {
                    total += accumulator.capacity()
                        + variable.capacity()
                        + std::mem::size_of::<Expression>() * 3;
                    stack.push(initial);
                    stack.push(source);
                    stack.push(mapping);
                }
                Expression::Vector(_) => {
                    // Vector leaf node, base already counted
                }
            }
        }

        total
    }

    /// Check if this is a simple expression (leaf node)
    pub fn is_simple(&self) -> bool {
        matches!(
            self,
            Expression::Literal(_)
                | Expression::Variable(_)
                | Expression::Label(_)
                | Expression::Parameter(_)
        )
    }

    /// Get approximate node count for complexity estimation
    pub fn node_count(&self) -> usize {
        let mut count = 1;
        let mut stack: Vec<&Expression> = vec![self];

        while let Some(expr) = stack.pop() {
            match expr {
                Expression::Unary { operand, .. }
                | Expression::TypeCast {
                    expression: operand,
                    ..
                }
                | Expression::Aggregate { arg: operand, .. } => {
                    count += 1;
                    stack.push(operand);
                }
                Expression::Binary { left, right, .. }
                | Expression::Subscript {
                    collection: left,
                    index: right,
                } => {
                    count += 2;
                    stack.push(left);
                    stack.push(right);
                }
                Expression::Property { object, .. }
                | Expression::LabelTagProperty { tag: object, .. } => {
                    count += 1;
                    stack.push(object);
                }
                Expression::List(items)
                | Expression::Path(items)
                | Expression::PathBuild(items) => {
                    count += items.len();
                    for item in items {
                        stack.push(item);
                    }
                }
                Expression::Map(entries) => {
                    count += entries.len();
                    for (_, v) in entries {
                        stack.push(v);
                    }
                }
                Expression::Function { args, .. } | Expression::Predicate { args, .. } => {
                    count += args.len();
                    for arg in args {
                        stack.push(arg);
                    }
                }
                Expression::Case {
                    test_expr,
                    conditions,
                    default,
                } => {
                    if let Some(test) = test_expr {
                        count += 1;
                        stack.push(test);
                    }
                    count += conditions.len() * 2;
                    for (cond, result) in conditions {
                        stack.push(cond);
                        stack.push(result);
                    }
                    if let Some(def) = default {
                        count += 1;
                        stack.push(def);
                    }
                }
                Expression::Range {
                    collection,
                    start,
                    end,
                } => {
                    count += 1;
                    stack.push(collection);
                    if let Some(s) = start {
                        count += 1;
                        stack.push(s);
                    }
                    if let Some(e) = end {
                        count += 1;
                        stack.push(e);
                    }
                }
                Expression::ListComprehension {
                    source,
                    filter,
                    map,
                    ..
                } => {
                    count += 1;
                    stack.push(source);
                    if let Some(f) = filter {
                        count += 1;
                        stack.push(f);
                    }
                    if let Some(m) = map {
                        count += 1;
                        stack.push(m);
                    }
                }
                Expression::Reduce {
                    initial,
                    source,
                    mapping,
                    ..
                } => {
                    count += 3;
                    stack.push(initial);
                    stack.push(source);
                    stack.push(mapping);
                }
                _ => {}
            }
        }

        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::BinaryOperator;

    #[test]
    fn test_simple_expression() {
        let expr = Expression::Variable("x".to_string());
        let size = expr.estimate_memory();
        assert!(size > 0);
        assert!(size >= std::mem::size_of::<Expression>());
    }

    #[test]
    fn test_literal_expression() {
        let expr = Expression::Literal(Value::Int(42));
        let size = expr.estimate_memory();
        assert_eq!(
            size,
            std::mem::size_of::<Expression>() + std::mem::size_of::<Value>()
        );
    }

    #[test]
    fn test_nested_expression() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Literal(Value::Int(1))),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(2))),
        };
        let size = expr.estimate_memory();
        // Should include: Expression + 2 * (Box + Literal)
        assert!(size > std::mem::size_of::<Expression>() * 3);
    }

    #[test]
    fn test_function_expression() {
        let expr = Expression::Function {
            name: "sum".to_string(),
            args: vec![
                Expression::Variable("a".to_string()),
                Expression::Variable("b".to_string()),
                Expression::Variable("c".to_string()),
            ],
        };
        let size = expr.estimate_memory();
        // Should include base + name + 3 variables
        assert!(size > std::mem::size_of::<Expression>() * 4);
    }

    #[test]
    fn test_list_expression() {
        let expr = Expression::List(vec![
            Expression::Literal(Value::Int(1)),
            Expression::Literal(Value::Int(2)),
            Expression::Literal(Value::Int(3)),
        ]);
        let size = expr.estimate_memory();
        // Should include base + 3 literals
        assert!(size > std::mem::size_of::<Expression>() * 3);
    }

    #[test]
    fn test_iterative_memory_estimation() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Function {
                name: "add".to_string(),
                args: vec![
                    Expression::Variable("a".to_string()),
                    Expression::Variable("b".to_string()),
                ],
            }),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(10))),
        };

        let recursive_size = expr.estimate_memory();
        let iterative_size = expr.estimate_memory_iterative();

        // Both methods should give positive results
        // Note: iterative version is an approximation and may differ slightly
        assert!(recursive_size > 0);
        assert!(iterative_size > 0);
        // They should be in the same ballpark (within 50% of each other)
        let ratio = recursive_size as f64 / iterative_size as f64;
        assert!(
            ratio > 0.5 && ratio < 2.0,
            "Recursive: {}, Iterative: {}, ratio: {}",
            recursive_size,
            iterative_size,
            ratio
        );
    }

    #[test]
    fn test_is_simple() {
        assert!(Expression::Variable("x".to_string()).is_simple());
        assert!(Expression::Literal(Value::Int(1)).is_simple());
        assert!(!Expression::Binary {
            left: Box::new(Expression::Literal(Value::Int(1))),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(2))),
        }
        .is_simple());
    }

    #[test]
    fn test_node_count() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Literal(Value::Int(1))),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(2))),
        };
        assert_eq!(expr.node_count(), 3);

        let simple = Expression::Variable("x".to_string());
        assert_eq!(simple.node_count(), 1);
    }
}
