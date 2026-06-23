//! Expression Visitor trait
//!
//! This module defines ExpressionVisitor traits for traversing and analyzing expression trees.
//! Visitor patterns avoid duplicate pattern matching code and improve code maintainability and extensibility.
//!
//! # Examples of use
//!
//! ```rust
//! use crate::core::types::expr::visitor::{ExpressionVisitor, PropertyCollector};
//!
//! let expr = Expression::property("a", "name");
//! let mut collector = PropertyCollector::new();
//! collector.visit(&expr);
//! assert_eq!(collector.properties, vec!["name".to_string()]);
//! ```

use crate::core::types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
use crate::core::types::DataType;
use crate::core::Expression;
use crate::core::Value;

/// Expression Visitor trait
///
/// Used to traverse and analyze the expression tree to avoid repetitive pattern matching code.
/// Implementing this trait creates a custom expression parser.
///
/// # Examples
///
/// ```rust
/// use crate::core::types::expr::visitor::ExpressionVisitor;
///
/// struct MyVisitor {
///     count: usize,
/// }
///
/// impl ExpressionVisitor for MyVisitor {
///     fn visit_literal(&mut self, _value: &Value) {
///         self.count += 1;
///     }
///
///     fn visit_binary(&mut self, _op: BinaryOperator, left: &Expression, right: &Expression) {
///         self.visit(left);
///         self.visit(right);
///     }
///
// ... Other methods
/// }
/// ```
pub trait ExpressionVisitor {
    /// access expression
    ///
    /// The default implementation distributes to specific access methods based on expression type.
    /// Subtypes can override this method to implement custom traversal logic.
    fn visit(&mut self, expr: &Expression) {
        match expr {
            Expression::Literal(value) => self.visit_literal(value),
            Expression::Variable(name) => self.visit_variable(name),
            Expression::Property { object, property } => {
                self.visit_property(object, property);
            }
            Expression::Binary { left, op, right } => {
                self.visit_binary(*op, left, right);
            }
            Expression::Unary { op, operand } => {
                self.visit_unary(*op, operand);
            }
            Expression::Function { name, args } => {
                self.visit_function(name, args);
            }
            Expression::Aggregate {
                func,
                arg,
                distinct,
            } => {
                self.visit_aggregate(func, arg, *distinct);
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                self.visit_case(test_expr.as_deref(), conditions, default.as_deref());
            }
            Expression::List(items) => {
                self.visit_list(items);
            }
            Expression::Map(entries) => {
                self.visit_map(entries);
            }
            Expression::TypeCast {
                expression,
                target_type,
            } => {
                self.visit_type_cast(expression, target_type);
            }
            Expression::Subscript { collection, index } => {
                self.visit_subscript(collection, index);
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                self.visit_range(collection, start.as_deref(), end.as_deref());
            }
            Expression::Path(items) => {
                self.visit_path(items);
            }
            Expression::Label(label) => {
                self.visit_label(label);
            }
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => {
                self.visit_list_comprehension(variable, source, filter.as_deref(), map.as_deref());
            }
            Expression::LabelTagProperty { tag, property } => {
                self.visit_label_tag_property(tag, property);
            }
            Expression::TagProperty { tag_name, property } => {
                self.visit_tag_property(tag_name, property);
            }
            Expression::EdgeProperty {
                edge_name,
                property,
            } => {
                self.visit_edge_property(edge_name, property);
            }
            Expression::Predicate { func, args } => {
                self.visit_predicate(func, args);
            }
            Expression::Reduce {
                accumulator,
                initial,
                variable,
                source,
                mapping,
            } => {
                self.visit_reduce(accumulator, initial, variable, source, mapping);
            }
            Expression::PathBuild(items) => {
                self.visit_path_build(items);
            }
            Expression::Parameter(name) => {
                self.visit_parameter(name);
            }
            Expression::Vector(data) => {
                self.visit_vector(data);
            }
        }
    }

    /// Accessing Literal Expressions
    fn visit_literal(&mut self, _value: &Value) {}

    /// Accessing variable expressions
    fn visit_variable(&mut self, _name: &str) {}

    /// Accessing Property Expressions
    fn visit_property(&mut self, object: &Expression, _property: &str) {
        self.visit(object);
    }

    /// Accessing binary arithmetic expressions
    fn visit_binary(&mut self, _op: BinaryOperator, left: &Expression, right: &Expression) {
        self.visit(left);
        self.visit(right);
    }

    /// Accessing unary arithmetic expressions
    fn visit_unary(&mut self, _op: UnaryOperator, operand: &Expression) {
        self.visit(operand);
    }

    /// Accessing function call expressions
    fn visit_function(&mut self, _name: &str, args: &[Expression]) {
        for arg in args {
            self.visit(arg);
        }
    }

    /// Accessing Aggregate Function Expressions
    fn visit_aggregate(&mut self, _func: &AggregateFunction, arg: &Expression, _distinct: bool) {
        self.visit(arg);
    }

    /// Accessing Conditional Expressions
    fn visit_case(
        &mut self,
        test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        if let Some(test) = test_expr {
            self.visit(test);
        }
        for (when, then) in conditions {
            self.visit(when);
            self.visit(then);
        }
        if let Some(default_expr) = default {
            self.visit(default_expr);
        }
    }

    /// Accessing List Expressions
    fn visit_list(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    /// Accessing mapping expressions
    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        for (_, value) in entries {
            self.visit(value);
        }
    }

    /// Access to type conversion expressions
    fn visit_type_cast(&mut self, expression: &Expression, _target_type: &DataType) {
        self.visit(expression);
    }

    /// Access subscript access expression
    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        self.visit(collection);
        self.visit(index);
    }

    /// Access Range Expressions
    fn visit_range(
        &mut self,
        collection: &Expression,
        start: Option<&Expression>,
        end: Option<&Expression>,
    ) {
        self.visit(collection);
        if let Some(start_expr) = start {
            self.visit(start_expr);
        }
        if let Some(end_expr) = end {
            self.visit(end_expr);
        }
    }

    /// Access path expression
    fn visit_path(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    /// Accessing tag expressions
    fn visit_label(&mut self, _label: &str) {}

    /// Access List Derivation Expressions
    fn visit_list_comprehension(
        &mut self,
        _variable: &str,
        source: &Expression,
        filter: Option<&Expression>,
        map: Option<&Expression>,
    ) {
        self.visit(source);
        if let Some(filter_expr) = filter {
            self.visit(filter_expr);
        }
        if let Some(map_expr) = map {
            self.visit(map_expr);
        }
    }

    /// Dynamic access expressions for accessing tag attributes
    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        self.visit(tag);
    }

    /// Access Tag Attribute Access Expressions
    fn visit_tag_property(&mut self, _tag_name: &str, _property: &str) {}

    /// Accessing edge attribute access expressions
    fn visit_edge_property(&mut self, _edge_name: &str, _property: &str) {}

    /// Access predicate expressions
    fn visit_predicate(&mut self, _func: &str, args: &[Expression]) {
        for arg in args {
            self.visit(arg);
        }
    }

    /// Accessing vector expressions
    fn visit_vector(&mut self, _data: &[f32]) {}

    /// Accessing Reduce Expressions
    fn visit_reduce(
        &mut self,
        _accumulator: &str,
        initial: &Expression,
        _variable: &str,
        source: &Expression,
        mapping: &Expression,
    ) {
        self.visit(initial);
        self.visit(source);
        self.visit(mapping);
    }

    /// Access Path Construction Expressions
    fn visit_path_build(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    /// Accessing query parameter expressions
    fn visit_parameter(&mut self, _name: &str) {}
}
