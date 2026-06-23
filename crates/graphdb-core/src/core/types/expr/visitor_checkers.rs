//! Expression checker
//!
//! This module provides implementations of various expression checkers, which are used to determine whether an expression meets certain conditions.
//!
//! # Available checkers
//!
//! - [`ConstantChecker`] - Check if the expression is a constant expression
//! - [`PropertyContainsChecker`] - Check if the expression contains the specified property name
//! - [`WildcardReplacer`] - Replace wildcard variables in expressions
//! - [`AggregateFunctionChecker`] - Check if the expression contains aggregate functions
//! - [`VariableContainsChecker`] - Check if the expression contains the specified variable
//! - [`PathBuildContainsChecker`] - Check if the expression contains PathBuild

use crate::core::types::expr::visitor::ExpressionVisitor;
use crate::core::types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
use crate::core::Expression;

/// Constant Checker
///
/// Check whether the expression is a constant expression (it does not contain any variables or properties).
///
/// # Examples
///
/// ```rust
/// use crate::core::types::expr::visitor::ConstantChecker;
/// use crate::core::Expression;
///
/// let expr = Expression::literal(42);
/// assert!(ConstantChecker::check(&expr));
///
/// let expr = Expression::variable("a");
/// assert!(!ConstantChecker::check(&expr));
/// ```
#[derive(Debug, Default)]
pub struct ConstantChecker {
    /// Is it a constant expression?
    pub is_constant: bool,
}

impl ConstantChecker {
    /// Create a new constant checker.
    pub fn new() -> Self {
        Self { is_constant: true }
    }

    /// Check whether the expression is a constant expression.
    ///
    /// # Parameters
    /// - `expr`: expression to be examined
    ///
    /// # Back
    /// `true`: The expression is a constant expression.
    /// `false`: The expression contains variables or properties.
    pub fn check(expr: &Expression) -> bool {
        let mut checker = Self::new();
        checker.visit(expr);
        checker.is_constant
    }
}

impl ExpressionVisitor for ConstantChecker {
    fn visit_literal(&mut self, _value: &crate::core::Value) {}

    fn visit_variable(&mut self, _name: &str) {
        self.is_constant = false;
    }

    fn visit_property(&mut self, _object: &Expression, _property: &str) {
        self.is_constant = false;
    }

    fn visit_binary(&mut self, _op: BinaryOperator, left: &Expression, right: &Expression) {
        if self.is_constant {
            self.visit(left);
        }
        if self.is_constant {
            self.visit(right);
        }
    }

    fn visit_unary(&mut self, _op: UnaryOperator, operand: &Expression) {
        if self.is_constant {
            self.visit(operand);
        }
    }

    fn visit_function(&mut self, _name: &str, args: &[Expression]) {
        if self.is_constant {
            for arg in args {
                self.visit(arg);
                if !self.is_constant {
                    break;
                }
            }
        }
    }

    fn visit_aggregate(&mut self, _func: &AggregateFunction, arg: &Expression, _distinct: bool) {
        if self.is_constant {
            self.visit(arg);
        }
    }

    fn visit_case(
        &mut self,
        test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        if self.is_constant {
            if let Some(test) = test_expr {
                self.visit(test);
                if !self.is_constant {
                    return;
                }
            }
            for (when, then) in conditions {
                self.visit(when);
                if !self.is_constant {
                    return;
                }
                self.visit(then);
                if !self.is_constant {
                    return;
                }
            }
            if let Some(default_expr) = default {
                self.visit(default_expr);
            }
        }
    }

    fn visit_list(&mut self, items: &[Expression]) {
        if self.is_constant {
            for item in items {
                self.visit(item);
                if !self.is_constant {
                    break;
                }
            }
        }
    }

    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        if self.is_constant {
            for (_, value) in entries {
                self.visit(value);
                if !self.is_constant {
                    break;
                }
            }
        }
    }

    fn visit_type_cast(
        &mut self,
        expression: &Expression,
        _target_type: &crate::core::types::DataType,
    ) {
        if self.is_constant {
            self.visit(expression);
        }
    }

    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        if self.is_constant {
            self.visit(collection);
            if self.is_constant {
                self.visit(index);
            }
        }
    }

    fn visit_range(
        &mut self,
        collection: &Expression,
        start: Option<&Expression>,
        end: Option<&Expression>,
    ) {
        if self.is_constant {
            self.visit(collection);
            if self.is_constant {
                if let Some(start_expr) = start {
                    self.visit(start_expr);
                    if !self.is_constant {
                        return;
                    }
                }
                if let Some(end_expr) = end {
                    self.visit(end_expr);
                }
            }
        }
    }

    fn visit_path(&mut self, items: &[Expression]) {
        if self.is_constant {
            for item in items {
                self.visit(item);
                if !self.is_constant {
                    break;
                }
            }
        }
    }

    fn visit_label(&mut self, _label: &str) {}

    fn visit_list_comprehension(
        &mut self,
        _variable: &str,
        source: &Expression,
        filter: Option<&Expression>,
        map: Option<&Expression>,
    ) {
        if self.is_constant {
            self.visit(source);
            if self.is_constant {
                if let Some(filter_expr) = filter {
                    self.visit(filter_expr);
                    if !self.is_constant {
                        return;
                    }
                }
                if let Some(map_expr) = map {
                    self.visit(map_expr);
                }
            }
        }
    }

    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        if self.is_constant {
            self.visit(tag);
        }
    }

    fn visit_tag_property(&mut self, _tag_name: &str, _property: &str) {
        self.is_constant = false;
    }

    fn visit_edge_property(&mut self, _edge_name: &str, _property: &str) {
        self.is_constant = false;
    }

    fn visit_predicate(&mut self, _func: &str, args: &[Expression]) {
        if self.is_constant {
            for arg in args {
                self.visit(arg);
                if !self.is_constant {
                    break;
                }
            }
        }
    }

    fn visit_reduce(
        &mut self,
        _accumulator: &str,
        initial: &Expression,
        _variable: &str,
        source: &Expression,
        mapping: &Expression,
    ) {
        if self.is_constant {
            self.visit(initial);
            if self.is_constant {
                self.visit(source);
                if self.is_constant {
                    self.visit(mapping);
                }
            }
        }
    }

    fn visit_path_build(&mut self, items: &[Expression]) {
        if self.is_constant {
            for item in items {
                self.visit(item);
                if !self.is_constant {
                    break;
                }
            }
        }
    }

    fn visit_parameter(&mut self, _name: &str) {}
}

/// The attribute contains a checker.
///
/// Check whether the expression contains the specified attribute name.
///
/// # Examples
///
/// ```rust
/// use crate::core::types::expr::visitor::PropertyContainsChecker;
/// use crate::core::Expression;
///
/// let expr = Expression::property("a", "name");
/// assert!(PropertyContainsChecker::check(&expr, &["name".to_string()]));
///
/// assert!(!PropertyContainsChecker::check(&expr, &["age".to_string()]));
/// ```
#[derive(Debug)]
pub struct PropertyContainsChecker {
    /// List of attribute names to be checked
    pub property_names: Vec<String>,
    /// Does it contain the specified attribute?
    pub contains: bool,
}

impl PropertyContainsChecker {
    /// Creating a new attribute that includes a checker
    ///
    /// # Parameters
    /// `property_names`: A list of property names to be checked.
    pub fn new(property_names: Vec<String>) -> Self {
        Self {
            property_names,
            contains: false,
        }
    }

    /// Check whether the expression contains the specified attribute name.
    ///
    /// # Parameters
    /// - `expr`: expression to be checked
    /// - `property_names`: list of property names to be checked
    ///
    /// # Returns
    /// `true`: The expression contains the specified attribute.
    /// `false`: The expression does not contain the specified attribute.
    pub fn check(expr: &Expression, property_names: &[String]) -> bool {
        let mut checker = Self::new(property_names.to_vec());
        checker.visit(expr);
        checker.contains
    }
}

impl ExpressionVisitor for PropertyContainsChecker {
    fn visit_literal(&mut self, _value: &crate::core::Value) {}

    fn visit_variable(&mut self, _name: &str) {}

    fn visit_property(&mut self, _object: &Expression, property: &str) {
        if self.property_names.contains(&property.to_string()) {
            self.contains = true;
        }
    }

    fn visit_binary(&mut self, _op: BinaryOperator, left: &Expression, right: &Expression) {
        if !self.contains {
            self.visit(left);
        }
        if !self.contains {
            self.visit(right);
        }
    }

    fn visit_unary(&mut self, _op: UnaryOperator, operand: &Expression) {
        if !self.contains {
            self.visit(operand);
        }
    }

    fn visit_function(&mut self, _name: &str, args: &[Expression]) {
        if !self.contains {
            for arg in args {
                self.visit(arg);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_aggregate(&mut self, _func: &AggregateFunction, arg: &Expression, _distinct: bool) {
        if !self.contains {
            self.visit(arg);
        }
    }

    fn visit_case(
        &mut self,
        test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        if !self.contains {
            if let Some(test) = test_expr {
                self.visit(test);
                if self.contains {
                    return;
                }
            }
            for (when, then) in conditions {
                self.visit(when);
                if self.contains {
                    return;
                }
                self.visit(then);
                if self.contains {
                    return;
                }
            }
            if let Some(default_expr) = default {
                self.visit(default_expr);
            }
        }
    }

    fn visit_list(&mut self, items: &[Expression]) {
        if !self.contains {
            for item in items {
                self.visit(item);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        if !self.contains {
            for (_, value) in entries {
                self.visit(value);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_type_cast(
        &mut self,
        expression: &Expression,
        _target_type: &crate::core::types::DataType,
    ) {
        if !self.contains {
            self.visit(expression);
        }
    }

    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        if !self.contains {
            self.visit(collection);
            if !self.contains {
                self.visit(index);
            }
        }
    }

    fn visit_range(
        &mut self,
        collection: &Expression,
        start: Option<&Expression>,
        end: Option<&Expression>,
    ) {
        if !self.contains {
            self.visit(collection);
            if !self.contains {
                if let Some(start_expr) = start {
                    self.visit(start_expr);
                    if self.contains {
                        return;
                    }
                }
                if let Some(end_expr) = end {
                    self.visit(end_expr);
                }
            }
        }
    }

    fn visit_path(&mut self, items: &[Expression]) {
        if !self.contains {
            for item in items {
                self.visit(item);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_label(&mut self, _label: &str) {}

    fn visit_list_comprehension(
        &mut self,
        _variable: &str,
        source: &Expression,
        filter: Option<&Expression>,
        map: Option<&Expression>,
    ) {
        if !self.contains {
            self.visit(source);
            if !self.contains {
                if let Some(filter_expr) = filter {
                    self.visit(filter_expr);
                    if self.contains {
                        return;
                    }
                }
                if let Some(map_expr) = map {
                    self.visit(map_expr);
                }
            }
        }
    }

    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        if !self.contains {
            self.visit(tag);
        }
    }

    fn visit_tag_property(&mut self, _tag_name: &str, property: &str) {
        if self.property_names.contains(&property.to_string()) {
            self.contains = true;
        }
    }

    fn visit_edge_property(&mut self, _edge_name: &str, property: &str) {
        if self.property_names.contains(&property.to_string()) {
            self.contains = true;
        }
    }

    fn visit_predicate(&mut self, _func: &str, args: &[Expression]) {
        if !self.contains {
            for arg in args {
                self.visit(arg);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_reduce(
        &mut self,
        _accumulator: &str,
        initial: &Expression,
        _variable: &str,
        source: &Expression,
        mapping: &Expression,
    ) {
        if !self.contains {
            self.visit(initial);
            if !self.contains {
                self.visit(source);
                if !self.contains {
                    self.visit(mapping);
                }
            }
        }
    }

    fn visit_path_build(&mut self, items: &[Expression]) {
        if !self.contains {
            for item in items {
                self.visit(item);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_parameter(&mut self, _name: &str) {}
}

/// Wildcard Replacer
///
/// Replace the wildcard variables (`*` or `_`) in the expression with specific aliases.
///
/// # Examples
///
/// ```rust
/// use crate::core::types::expr::visitor::WildcardReplacer;
/// use crate::core::Expression;
///
/// let expr = Expression::property("*", "name");
/// let mut replacer = WildcardReplacer::new("v");
/// let replaced = replacer.replace(&expr);
/// ```
#[derive(Debug)]
pub struct WildcardReplacer {
    /// Replace the target alias
    pub alias: String,
}

impl WildcardReplacer {
    /// Create a new wildcard replacer.
    ///
    /// # Parameters
    /// `alias`: A synonym used to replace wildcards.
    pub fn new(alias: &str) -> Self {
        Self {
            alias: alias.to_string(),
        }
    }

    /// Replace the wildcards in the expression.
    ///
    /// # Parameters
    /// `expr`: The expression that needs to be replaced.
    ///
    /// # Returns
    /// The replaced expression
    pub fn replace(&self, expr: &Expression) -> Expression {
        self.replace_internal(expr)
    }

    fn replace_internal(&self, expr: &Expression) -> Expression {
        match expr {
            Expression::Literal(value) => Expression::Literal(value.clone()),
            Expression::Variable(name) => {
                if name == "*" || name == "_" {
                    Expression::Variable(self.alias.clone())
                } else {
                    Expression::Variable(name.clone())
                }
            }
            Expression::Property { object, property } => Expression::Property {
                object: Box::new(self.replace_internal(object)),
                property: property.clone(),
            },
            Expression::Binary { left, op, right } => Expression::Binary {
                left: Box::new(self.replace_internal(left)),
                op: *op,
                right: Box::new(self.replace_internal(right)),
            },
            Expression::Unary { op, operand } => Expression::Unary {
                op: *op,
                operand: Box::new(self.replace_internal(operand)),
            },
            Expression::Function { name, args } => Expression::Function {
                name: name.clone(),
                args: args.iter().map(|arg| self.replace_internal(arg)).collect(),
            },
            Expression::Aggregate {
                func,
                arg,
                distinct,
            } => Expression::Aggregate {
                func: func.clone(),
                arg: Box::new(self.replace_internal(arg)),
                distinct: *distinct,
            },
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => Expression::Case {
                test_expr: test_expr
                    .as_ref()
                    .map(|e| Box::new(self.replace_internal(e))),
                conditions: conditions
                    .iter()
                    .map(|(w, t)| (self.replace_internal(w), self.replace_internal(t)))
                    .collect(),
                default: default.as_ref().map(|e| Box::new(self.replace_internal(e))),
            },
            Expression::List(items) => Expression::List(
                items
                    .iter()
                    .map(|item| self.replace_internal(item))
                    .collect(),
            ),
            Expression::Map(entries) => Expression::Map(
                entries
                    .iter()
                    .map(|(k, v)| (k.clone(), self.replace_internal(v)))
                    .collect(),
            ),
            Expression::TypeCast {
                expression,
                target_type,
            } => Expression::TypeCast {
                expression: Box::new(self.replace_internal(expression)),
                target_type: target_type.clone(),
            },
            Expression::Subscript { collection, index } => Expression::Subscript {
                collection: Box::new(self.replace_internal(collection)),
                index: Box::new(self.replace_internal(index)),
            },
            Expression::Range {
                collection,
                start,
                end,
            } => Expression::Range {
                collection: Box::new(self.replace_internal(collection)),
                start: start.as_ref().map(|e| Box::new(self.replace_internal(e))),
                end: end.as_ref().map(|e| Box::new(self.replace_internal(e))),
            },
            Expression::Path(items) => Expression::Path(
                items
                    .iter()
                    .map(|item| self.replace_internal(item))
                    .collect(),
            ),
            Expression::Label(label) => Expression::Label(label.clone()),
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => Expression::ListComprehension {
                variable: variable.clone(),
                source: Box::new(self.replace_internal(source)),
                filter: filter.as_ref().map(|e| Box::new(self.replace_internal(e))),
                map: map.as_ref().map(|e| Box::new(self.replace_internal(e))),
            },
            Expression::LabelTagProperty { tag, property } => Expression::LabelTagProperty {
                tag: Box::new(self.replace_internal(tag)),
                property: property.clone(),
            },
            Expression::TagProperty { tag_name, property } => Expression::TagProperty {
                tag_name: tag_name.clone(),
                property: property.clone(),
            },
            Expression::EdgeProperty {
                edge_name,
                property,
            } => Expression::EdgeProperty {
                edge_name: edge_name.clone(),
                property: property.clone(),
            },
            Expression::Predicate { func, args } => Expression::Predicate {
                func: func.clone(),
                args: args.iter().map(|arg| self.replace_internal(arg)).collect(),
            },
            Expression::Reduce {
                accumulator,
                initial,
                variable,
                source,
                mapping,
            } => Expression::Reduce {
                accumulator: accumulator.clone(),
                initial: Box::new(self.replace_internal(initial)),
                variable: variable.clone(),
                source: Box::new(self.replace_internal(source)),
                mapping: Box::new(self.replace_internal(mapping)),
            },
            Expression::PathBuild(items) => Expression::PathBuild(
                items
                    .iter()
                    .map(|item| self.replace_internal(item))
                    .collect(),
            ),
            Expression::Parameter(name) => Expression::Parameter(name.clone()),
            Expression::Vector(data) => Expression::Vector(data.clone()),
        }
    }
}

/// Aggregate Function Checker
///
/// Check whether the expression contains aggregate functions.
///
/// # Examples
///
/// ```rust
/// use crate::core::types::expr::visitor::AggregateFunctionChecker;
/// use crate::core::Expression;
///
/// let expr = Expression::aggregate("count", Expression::variable("v"), false);
/// assert!(AggregateFunctionChecker::check(&expr));
///
/// let expr = Expression::variable("a");
/// assert!(!AggregateFunctionChecker::check(&expr));
/// ```
#[derive(Debug, Default)]
pub struct AggregateFunctionChecker {
    /// Does it contain aggregate functions?
    pub contains_aggregate: bool,
}

impl AggregateFunctionChecker {
    /// Create a new aggregate function checker.
    pub fn new() -> Self {
        Self {
            contains_aggregate: false,
        }
    }

    /// Check whether the expression contains aggregate functions.
    ///
    /// # Parameters
    /// - `expr`: expression to be checked
    ///
    /// # Returns
    /// - `true`: expression contains aggregate functions
    /// - `false`: expression does not contain an aggregate function
    pub fn check(expr: &Expression) -> bool {
        let mut checker = Self::new();
        checker.visit(expr);
        checker.contains_aggregate
    }
}

impl ExpressionVisitor for AggregateFunctionChecker {
    fn visit_literal(&mut self, _value: &crate::core::Value) {}

    fn visit_variable(&mut self, _name: &str) {}

    fn visit_property(&mut self, object: &Expression, _property: &str) {
        if !self.contains_aggregate {
            self.visit(object);
        }
    }

    fn visit_binary(&mut self, _op: BinaryOperator, left: &Expression, right: &Expression) {
        if !self.contains_aggregate {
            self.visit(left);
        }
        if !self.contains_aggregate {
            self.visit(right);
        }
    }

    fn visit_unary(&mut self, _op: UnaryOperator, operand: &Expression) {
        if !self.contains_aggregate {
            self.visit(operand);
        }
    }

    fn visit_function(&mut self, _name: &str, args: &[Expression]) {
        if !self.contains_aggregate {
            for arg in args {
                self.visit(arg);
                if self.contains_aggregate {
                    break;
                }
            }
        }
    }

    fn visit_aggregate(&mut self, _func: &AggregateFunction, _arg: &Expression, _distinct: bool) {
        self.contains_aggregate = true;
    }

    fn visit_case(
        &mut self,
        test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        if !self.contains_aggregate {
            if let Some(test) = test_expr {
                self.visit(test);
                if self.contains_aggregate {
                    return;
                }
            }
            for (when, then) in conditions {
                self.visit(when);
                if self.contains_aggregate {
                    return;
                }
                self.visit(then);
                if self.contains_aggregate {
                    return;
                }
            }
            if let Some(default_expr) = default {
                self.visit(default_expr);
            }
        }
    }

    fn visit_list(&mut self, items: &[Expression]) {
        if !self.contains_aggregate {
            for item in items {
                self.visit(item);
                if self.contains_aggregate {
                    break;
                }
            }
        }
    }

    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        if !self.contains_aggregate {
            for (_, value) in entries {
                self.visit(value);
                if self.contains_aggregate {
                    break;
                }
            }
        }
    }

    fn visit_type_cast(
        &mut self,
        expression: &Expression,
        _target_type: &crate::core::types::DataType,
    ) {
        if !self.contains_aggregate {
            self.visit(expression);
        }
    }

    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        if !self.contains_aggregate {
            self.visit(collection);
            if !self.contains_aggregate {
                self.visit(index);
            }
        }
    }

    fn visit_range(
        &mut self,
        collection: &Expression,
        start: Option<&Expression>,
        end: Option<&Expression>,
    ) {
        if !self.contains_aggregate {
            self.visit(collection);
            if !self.contains_aggregate {
                if let Some(start_expr) = start {
                    self.visit(start_expr);
                    if self.contains_aggregate {
                        return;
                    }
                }
                if let Some(end_expr) = end {
                    self.visit(end_expr);
                }
            }
        }
    }

    fn visit_path(&mut self, items: &[Expression]) {
        if !self.contains_aggregate {
            for item in items {
                self.visit(item);
                if self.contains_aggregate {
                    break;
                }
            }
        }
    }

    fn visit_label(&mut self, _label: &str) {}

    fn visit_list_comprehension(
        &mut self,
        _variable: &str,
        source: &Expression,
        filter: Option<&Expression>,
        map: Option<&Expression>,
    ) {
        if !self.contains_aggregate {
            self.visit(source);
            if !self.contains_aggregate {
                if let Some(filter_expr) = filter {
                    self.visit(filter_expr);
                    if self.contains_aggregate {
                        return;
                    }
                }
                if let Some(map_expr) = map {
                    self.visit(map_expr);
                }
            }
        }
    }

    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        if !self.contains_aggregate {
            self.visit(tag);
        }
    }

    fn visit_tag_property(&mut self, _tag_name: &str, _property: &str) {}

    fn visit_edge_property(&mut self, _edge_name: &str, _property: &str) {}

    fn visit_predicate(&mut self, _func: &str, args: &[Expression]) {
        if !self.contains_aggregate {
            for arg in args {
                self.visit(arg);
                if self.contains_aggregate {
                    break;
                }
            }
        }
    }

    fn visit_reduce(
        &mut self,
        _accumulator: &str,
        initial: &Expression,
        _variable: &str,
        source: &Expression,
        mapping: &Expression,
    ) {
        if !self.contains_aggregate {
            self.visit(initial);
            if !self.contains_aggregate {
                self.visit(source);
                if !self.contains_aggregate {
                    self.visit(mapping);
                }
            }
        }
    }

    fn visit_path_build(&mut self, items: &[Expression]) {
        if !self.contains_aggregate {
            for item in items {
                self.visit(item);
                if self.contains_aggregate {
                    break;
                }
            }
        }
    }

    fn visit_parameter(&mut self, _name: &str) {}
}

/// Variable Inclusion Checker
///
/// Checks if the expression contains the specified variable name.
///
/// # Examples
///
/// ```rust
/// use crate::core::types::expr::visitor::VariableContainsChecker;
/// use crate::core::Expression;
///
/// let expr = Expression::property("a", "name");
/// assert!(VariableContainsChecker::check(&expr, "a"));
///
/// assert!(!VariableContainsChecker::check(&expr, "b"));
/// ```
#[derive(Debug)]
pub struct VariableContainsChecker {
    /// The name of the variable to be checked
    pub variable_name: String,
    /// Whether to include the specified variable
    pub contains: bool,
}

impl VariableContainsChecker {
    /// Create a new variable inclusion checker
    ///
    /// # Parameters
    /// - `variable_name`: Name of the variable to be checked
    pub fn new(variable_name: &str) -> Self {
        Self {
            variable_name: variable_name.to_string(),
            contains: false,
        }
    }

    /// Checks if the expression contains the specified variable name
    ///
    /// # Parameters
    /// - `expr`: expression to be checked
    /// - `variable_name`: variable name to be checked
    ///
    /// # Returns
    /// - `true`: The expression contains the specified variable.
    /// - `false`: The expression does not contain the specified variable.
    pub fn check(expr: &Expression, variable_name: &str) -> bool {
        let mut checker = Self::new(variable_name);
        checker.visit(expr);
        checker.contains
    }
}

impl ExpressionVisitor for VariableContainsChecker {
    fn visit_literal(&mut self, _value: &crate::core::Value) {}

    fn visit_variable(&mut self, name: &str) {
        if name == self.variable_name {
            self.contains = true;
        }
    }

    fn visit_property(&mut self, object: &Expression, _property: &str) {
        if !self.contains {
            self.visit(object);
        }
    }

    fn visit_binary(&mut self, _op: BinaryOperator, left: &Expression, right: &Expression) {
        if !self.contains {
            self.visit(left);
        }
        if !self.contains {
            self.visit(right);
        }
    }

    fn visit_unary(&mut self, _op: UnaryOperator, operand: &Expression) {
        if !self.contains {
            self.visit(operand);
        }
    }

    fn visit_function(&mut self, _name: &str, args: &[Expression]) {
        if !self.contains {
            for arg in args {
                self.visit(arg);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_aggregate(&mut self, _func: &AggregateFunction, arg: &Expression, _distinct: bool) {
        if !self.contains {
            self.visit(arg);
        }
    }

    fn visit_case(
        &mut self,
        test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        if !self.contains {
            if let Some(test) = test_expr {
                self.visit(test);
                if self.contains {
                    return;
                }
            }
            for (when, then) in conditions {
                self.visit(when);
                if self.contains {
                    return;
                }
                self.visit(then);
                if self.contains {
                    return;
                }
            }
            if let Some(default_expr) = default {
                self.visit(default_expr);
            }
        }
    }

    fn visit_list(&mut self, items: &[Expression]) {
        if !self.contains {
            for item in items {
                self.visit(item);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        if !self.contains {
            for (_, value) in entries {
                self.visit(value);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_type_cast(
        &mut self,
        expression: &Expression,
        _target_type: &crate::core::types::DataType,
    ) {
        if !self.contains {
            self.visit(expression);
        }
    }

    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        if !self.contains {
            self.visit(collection);
            if !self.contains {
                self.visit(index);
            }
        }
    }

    fn visit_range(
        &mut self,
        collection: &Expression,
        start: Option<&Expression>,
        end: Option<&Expression>,
    ) {
        if !self.contains {
            self.visit(collection);
            if !self.contains {
                if let Some(start_expr) = start {
                    self.visit(start_expr);
                    if self.contains {
                        return;
                    }
                }
                if let Some(end_expr) = end {
                    self.visit(end_expr);
                }
            }
        }
    }

    fn visit_path(&mut self, items: &[Expression]) {
        if !self.contains {
            for item in items {
                self.visit(item);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_label(&mut self, _label: &str) {}

    fn visit_list_comprehension(
        &mut self,
        _variable: &str,
        source: &Expression,
        filter: Option<&Expression>,
        map: Option<&Expression>,
    ) {
        if !self.contains {
            self.visit(source);
            if !self.contains {
                if let Some(filter_expr) = filter {
                    self.visit(filter_expr);
                    if self.contains {
                        return;
                    }
                }
                if let Some(map_expr) = map {
                    self.visit(map_expr);
                }
            }
        }
    }

    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        if !self.contains {
            self.visit(tag);
        }
    }

    fn visit_tag_property(&mut self, _tag_name: &str, _property: &str) {}

    fn visit_edge_property(&mut self, _edge_name: &str, _property: &str) {}

    fn visit_predicate(&mut self, _func: &str, args: &[Expression]) {
        if !self.contains {
            for arg in args {
                self.visit(arg);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_reduce(
        &mut self,
        _accumulator: &str,
        initial: &Expression,
        _variable: &str,
        source: &Expression,
        mapping: &Expression,
    ) {
        if !self.contains {
            self.visit(initial);
            if !self.contains {
                self.visit(source);
                if !self.contains {
                    self.visit(mapping);
                }
            }
        }
    }

    fn visit_path_build(&mut self, items: &[Expression]) {
        if !self.contains {
            for item in items {
                self.visit(item);
                if self.contains {
                    break;
                }
            }
        }
    }

    fn visit_parameter(&mut self, _name: &str) {}
}

/// PathBuild Includes Inspector
///
/// Checks if the expression contains a PathBuild expression.
///
/// # Examples
///
/// ```rust
/// use crate::core::types::expr::visitor::PathBuildContainsChecker;
/// use crate::core::Expression;
///
/// let expr = Expression::path_build(vec![Expression::variable("a")]);
/// assert!(PathBuildContainsChecker::check(&expr));
///
/// let expr = Expression::variable("a");
/// assert!(!PathBuildContainsChecker::check(&expr));
/// ```
#[derive(Debug, Default)]
pub struct PathBuildContainsChecker {
    /// Whether to include PathBuild
    pub contains_path_build: bool,
}

impl PathBuildContainsChecker {
    /// Creating a new PathBuild Inclusion Checker
    pub fn new() -> Self {
        Self {
            contains_path_build: false,
        }
    }

    /// Checks if the expression contains PathBuild.
    ///
    /// # Parameters
    /// - `expr`: expression to be checked
    ///
    /// # Returns
    /// - `true`: The expression contains PathBuild.
    /// - `false`: The expression does not contain PathBuild.
    pub fn check(expr: &Expression) -> bool {
        let mut checker = Self::new();
        checker.visit(expr);
        checker.contains_path_build
    }
}

impl ExpressionVisitor for PathBuildContainsChecker {
    fn visit_literal(&mut self, _value: &crate::core::Value) {}

    fn visit_variable(&mut self, _name: &str) {}

    fn visit_property(&mut self, object: &Expression, _property: &str) {
        if !self.contains_path_build {
            self.visit(object);
        }
    }

    fn visit_binary(&mut self, _op: BinaryOperator, left: &Expression, right: &Expression) {
        if !self.contains_path_build {
            self.visit(left);
        }
        if !self.contains_path_build {
            self.visit(right);
        }
    }

    fn visit_unary(&mut self, _op: UnaryOperator, operand: &Expression) {
        if !self.contains_path_build {
            self.visit(operand);
        }
    }

    fn visit_function(&mut self, _name: &str, args: &[Expression]) {
        if !self.contains_path_build {
            for arg in args {
                self.visit(arg);
                if self.contains_path_build {
                    break;
                }
            }
        }
    }

    fn visit_aggregate(&mut self, _func: &AggregateFunction, arg: &Expression, _distinct: bool) {
        if !self.contains_path_build {
            self.visit(arg);
        }
    }

    fn visit_case(
        &mut self,
        test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        if !self.contains_path_build {
            if let Some(test) = test_expr {
                self.visit(test);
                if self.contains_path_build {
                    return;
                }
            }
            for (when, then) in conditions {
                self.visit(when);
                if self.contains_path_build {
                    return;
                }
                self.visit(then);
                if self.contains_path_build {
                    return;
                }
            }
            if let Some(default_expr) = default {
                self.visit(default_expr);
            }
        }
    }

    fn visit_list(&mut self, items: &[Expression]) {
        if !self.contains_path_build {
            for item in items {
                self.visit(item);
                if self.contains_path_build {
                    break;
                }
            }
        }
    }

    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        if !self.contains_path_build {
            for (_, value) in entries {
                self.visit(value);
                if self.contains_path_build {
                    break;
                }
            }
        }
    }

    fn visit_type_cast(
        &mut self,
        expression: &Expression,
        _target_type: &crate::core::types::DataType,
    ) {
        if !self.contains_path_build {
            self.visit(expression);
        }
    }

    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        if !self.contains_path_build {
            self.visit(collection);
            if !self.contains_path_build {
                self.visit(index);
            }
        }
    }

    fn visit_range(
        &mut self,
        collection: &Expression,
        start: Option<&Expression>,
        end: Option<&Expression>,
    ) {
        if !self.contains_path_build {
            self.visit(collection);
            if !self.contains_path_build {
                if let Some(start_expr) = start {
                    self.visit(start_expr);
                    if self.contains_path_build {
                        return;
                    }
                }
                if let Some(end_expr) = end {
                    self.visit(end_expr);
                }
            }
        }
    }

    fn visit_path(&mut self, items: &[Expression]) {
        if !self.contains_path_build {
            for item in items {
                self.visit(item);
                if self.contains_path_build {
                    break;
                }
            }
        }
    }

    fn visit_label(&mut self, _label: &str) {}

    fn visit_list_comprehension(
        &mut self,
        _variable: &str,
        source: &Expression,
        filter: Option<&Expression>,
        map: Option<&Expression>,
    ) {
        if !self.contains_path_build {
            self.visit(source);
            if !self.contains_path_build {
                if let Some(filter_expr) = filter {
                    self.visit(filter_expr);
                    if self.contains_path_build {
                        return;
                    }
                }
                if let Some(map_expr) = map {
                    self.visit(map_expr);
                }
            }
        }
    }

    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        if !self.contains_path_build {
            self.visit(tag);
        }
    }

    fn visit_tag_property(&mut self, _tag_name: &str, _property: &str) {}

    fn visit_edge_property(&mut self, _edge_name: &str, _property: &str) {}

    fn visit_predicate(&mut self, _func: &str, args: &[Expression]) {
        if !self.contains_path_build {
            for arg in args {
                self.visit(arg);
                if self.contains_path_build {
                    break;
                }
            }
        }
    }

    fn visit_reduce(
        &mut self,
        _accumulator: &str,
        initial: &Expression,
        _variable: &str,
        source: &Expression,
        mapping: &Expression,
    ) {
        if !self.contains_path_build {
            self.visit(initial);
            if !self.contains_path_build {
                self.visit(source);
                if !self.contains_path_build {
                    self.visit(mapping);
                }
            }
        }
    }

    fn visit_path_build(&mut self, _items: &[Expression]) {
        self.contains_path_build = true;
    }

    fn visit_parameter(&mut self, _name: &str) {}
}
