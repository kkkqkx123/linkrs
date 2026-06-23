//! Expression Collector
//!
//! This module provides implementations of various expression collectors, which are used to extract specific information from expressions.
//!
//! # Available collectors
//!
//! - [`PropertyCollector`] - Collect all property names used in the expression
//! - [`VariableCollector`] - Collect all variable names used in the expression
//! - [`FunctionCollector`] - Collect all function names used in the expression

use crate::core::types::expr::visitor::ExpressionVisitor;
use crate::core::types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
use crate::core::types::DataType;
use crate::core::{Expression, Value};

/// Attribute Collector
///
/// Collect all the attribute names that are used in the expression.
///
/// # Example
///
/// ```rust
/// use crate::core::types::expr::visitor::PropertyCollector;
/// use crate::core::Expression;
///
/// let expr = Expression::property("a", "name");
/// let mut collector = PropertyCollector::new();
/// collector.visit(&expr);
/// assert_eq!(collector.properties, vec!["name".to_string()]);
/// ```
#[derive(Debug, Default)]
pub struct PropertyCollector {
    /// List of collected attribute names
    pub properties: Vec<String>,
}

impl PropertyCollector {
    /// Create a new attribute collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear the collector.
    pub fn clear(&mut self) {
        self.properties.clear();
    }
}

impl ExpressionVisitor for PropertyCollector {
    fn visit_literal(&mut self, _value: &crate::core::Value) {}

    fn visit_variable(&mut self, _name: &str) {}

    fn visit_property(&mut self, _object: &Expression, property: &str) {
        let prop_name = property.to_string();
        if !self.properties.contains(&prop_name) {
            self.properties.push(prop_name);
        }
    }

    fn visit_binary(&mut self, _op: BinaryOperator, left: &Expression, right: &Expression) {
        self.visit(left);
        self.visit(right);
    }

    fn visit_unary(&mut self, _op: UnaryOperator, operand: &Expression) {
        self.visit(operand);
    }

    fn visit_function(&mut self, _name: &str, args: &[Expression]) {
        for arg in args {
            self.visit(arg);
        }
    }

    fn visit_aggregate(&mut self, _func: &AggregateFunction, arg: &Expression, _distinct: bool) {
        self.visit(arg);
    }

    fn visit_case(
        &mut self,
        _test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        for (when, then) in conditions {
            self.visit(when);
            self.visit(then);
        }
        if let Some(default_expr) = default {
            self.visit(default_expr);
        }
    }

    fn visit_list(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        for (_, value) in entries {
            self.visit(value);
        }
    }

    fn visit_type_cast(
        &mut self,
        expression: &Expression,
        _target_type: &crate::core::types::DataType,
    ) {
        self.visit(expression);
    }

    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        self.visit(collection);
        self.visit(index);
    }

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

    fn visit_path(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
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
        self.visit(source);
        if let Some(filter_expr) = filter {
            self.visit(filter_expr);
        }
        if let Some(map_expr) = map {
            self.visit(map_expr);
        }
    }

    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        self.visit(tag);
    }

    fn visit_tag_property(&mut self, _tag_name: &str, _property: &str) {}

    fn visit_edge_property(&mut self, _edge_name: &str, _property: &str) {}

    fn visit_predicate(&mut self, _func: &str, args: &[Expression]) {
        for arg in args {
            self.visit(arg);
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
        self.visit(initial);
        self.visit(source);
        self.visit(mapping);
    }

    fn visit_path_build(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    fn visit_parameter(&mut self, _name: &str) {}
}

/// OR Condition Collector
///
/// Collect all the OR conditions in the expression and check whether they can be converted into IN conditions.
///
/// # Examples
///
/// ```rust
/// use crate::core::types::expr::visitor::OrConditionCollector;
/// use crate::core::Expression;
///
/// let expr = Expression::Binary {
///     left: Box::new(Expression::Binary {
///         left: Box::new(Expression::property(Expression::variable("n"), "age")),
///         op: BinaryOperator::Equal,
///         right: Box::new(Expression::literal(10)),
///     }),
///     op: BinaryOperator::Or,
///     right: Box::new(Expression::Binary {
///         left: Box::new(Expression::property(Expression::variable("n"), "age")),
///         op: BinaryOperator::Equal,
///         right: Box::new(Expression::literal(20)),
///     }),
/// };
///
/// let mut collector = OrConditionCollector::new();
/// collector.visit(&expr);
///
/// assert_eq!(collector.can_convert_to_in(), true);
/// assert_eq!(collector.property_name(), Some("age".to_string()));
/// assert_eq!(collector.values(), vec![Value::Int(10), Value::Int(20)]);
/// ```
#[derive(Debug)]
pub struct OrConditionCollector {
    is_or: bool,
    property_name: Option<String>,
    values: Vec<Value>,
    can_convert: bool,
}

impl Default for OrConditionCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl OrConditionCollector {
    pub fn new() -> Self {
        Self {
            is_or: false,
            property_name: None,
            values: Vec::new(),
            can_convert: true,
        }
    }

    pub fn clear(&mut self) {
        self.is_or = false;
        self.property_name = None;
        self.values.clear();
        self.can_convert = true;
    }

    pub fn is_or(&self) -> bool {
        self.is_or
    }

    pub fn property_name(&self) -> Option<&String> {
        self.property_name.as_ref()
    }

    pub fn values(&self) -> &[Value] {
        &self.values
    }

    pub fn can_convert_to_in(&self) -> bool {
        self.can_convert && self.property_name.is_some() && !self.values.is_empty()
    }
}

impl ExpressionVisitor for OrConditionCollector {
    fn visit_literal(&mut self, _value: &Value) {}

    fn visit_variable(&mut self, _name: &str) {}

    fn visit_property(&mut self, _object: &Expression, property: &str) {
        if !self.is_or {
            self.can_convert = false;
            return;
        }
        if self.property_name.is_none() {
            self.property_name = Some(property.to_string());
        } else if self.property_name.as_ref() != Some(&property.to_string()) {
            self.can_convert = false;
        }
    }

    fn visit_binary(&mut self, op: BinaryOperator, left: &Expression, right: &Expression) {
        match op {
            BinaryOperator::Or => {
                self.is_or = true;
                self.visit(left);
                self.visit(right);
            }
            BinaryOperator::Equal => {
                if !self.is_or {
                    self.can_convert = false;
                }
                self.visit(left);
                self.visit(right);
                if self.can_convert {
                    if let Expression::Literal(value) = right {
                        self.values.push(value.clone());
                    }
                }
            }
            _ => {
                self.can_convert = false;
                self.visit(left);
                self.visit(right);
            }
        }
    }

    fn visit_unary(&mut self, _op: UnaryOperator, operand: &Expression) {
        self.visit(operand);
    }

    fn visit_function(&mut self, _name: &str, args: &[Expression]) {
        for arg in args {
            self.visit(arg);
        }
    }

    fn visit_aggregate(&mut self, _func: &AggregateFunction, arg: &Expression, _distinct: bool) {
        self.visit(arg);
    }

    fn visit_case(
        &mut self,
        _test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        for (when, then) in conditions {
            self.visit(when);
            self.visit(then);
        }
        if let Some(default_expr) = default {
            self.visit(default_expr);
        }
    }

    fn visit_list(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        for (_, value) in entries {
            self.visit(value);
        }
    }

    fn visit_type_cast(&mut self, expression: &Expression, _target_type: &DataType) {
        self.visit(expression);
    }

    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        self.visit(collection);
        self.visit(index);
    }

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

    fn visit_path(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
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
        self.visit(source);
        if let Some(filter_expr) = filter {
            self.visit(filter_expr);
        }
        if let Some(map_expr) = map {
            self.visit(map_expr);
        }
    }

    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        self.visit(tag);
    }

    fn visit_tag_property(&mut self, _tag_name: &str, _property: &str) {}

    fn visit_edge_property(&mut self, _edge_name: &str, _property: &str) {}

    fn visit_predicate(&mut self, _func: &str, args: &[Expression]) {
        for arg in args {
            self.visit(arg);
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
        self.visit(initial);
        self.visit(source);
        self.visit(mapping);
    }

    fn visit_path_build(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    fn visit_parameter(&mut self, _name: &str) {}
}

/// Attribute Predicate Collector
///
/// Collect all attribute predicates from the expression (attribute + operator + value).
///
/// # Examples
///
/// ```rust
/// use crate::core::types::expr::visitor::PropertyPredicateCollector;
/// use crate::core::Expression;
///
/// let expr = Expression::Binary {
///     left: Box::new(Expression::Binary {
///         left: Box::new(Expression::property(Expression::variable("n"), "age")),
///         op: BinaryOperator::Equal,
///         right: Box::new(Expression::literal(10)),
///     }),
///     op: BinaryOperator::And,
///     right: Box::new(Expression::Binary {
///         left: Box::new(Expression::property(Expression::variable("n"), "name")),
///         op: BinaryOperator::GreaterThan,
///         right: Box::new(Expression::literal("Alice")),
///     }),
/// };
///
/// let mut collector = PropertyPredicateCollector::new();
/// collector.visit(&expr);
///
/// assert_eq!(collector.predicates().len(), 2);
/// ```
#[derive(Debug, Default)]
pub struct PropertyPredicateCollector {
    predicates: Vec<PropertyPredicate>,
    current_property: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PropertyPredicate {
    pub property: String,
    pub operator: BinaryOperator,
    pub value: Value,
}

impl PropertyPredicateCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.predicates.clear();
        self.current_property = None;
    }

    pub fn predicates(&self) -> &[PropertyPredicate] {
        &self.predicates
    }

    pub fn predicates_for_property(&self, property: &str) -> Vec<&PropertyPredicate> {
        self.predicates
            .iter()
            .filter(|p| p.property == property)
            .collect()
    }
}

impl ExpressionVisitor for PropertyPredicateCollector {
    fn visit_literal(&mut self, _value: &Value) {}

    fn visit_variable(&mut self, _name: &str) {}

    fn visit_property(&mut self, _object: &Expression, property: &str) {
        self.current_property = Some(property.to_string());
    }

    fn visit_binary(&mut self, op: BinaryOperator, left: &Expression, right: &Expression) {
        if matches!(
            op,
            BinaryOperator::Equal
                | BinaryOperator::NotEqual
                | BinaryOperator::LessThan
                | BinaryOperator::LessThanOrEqual
                | BinaryOperator::GreaterThan
                | BinaryOperator::GreaterThanOrEqual
        ) {
            if let Some(property) = &self.current_property {
                if let Expression::Literal(value) = right {
                    self.predicates.push(PropertyPredicate {
                        property: property.clone(),
                        operator: op,
                        value: value.clone(),
                    });
                }
            }
        }
        self.current_property = None;
        self.visit(left);
        self.visit(right);
    }

    fn visit_unary(&mut self, _op: UnaryOperator, operand: &Expression) {
        self.visit(operand);
    }

    fn visit_function(&mut self, _name: &str, args: &[Expression]) {
        for arg in args {
            self.visit(arg);
        }
    }

    fn visit_aggregate(&mut self, _func: &AggregateFunction, arg: &Expression, _distinct: bool) {
        self.visit(arg);
    }

    fn visit_case(
        &mut self,
        _test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        for (when, then) in conditions {
            self.visit(when);
            self.visit(then);
        }
        if let Some(default_expr) = default {
            self.visit(default_expr);
        }
    }

    fn visit_list(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        for (_, value) in entries {
            self.visit(value);
        }
    }

    fn visit_type_cast(&mut self, expression: &Expression, _target_type: &DataType) {
        self.visit(expression);
    }

    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        self.visit(collection);
        self.visit(index);
    }

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

    fn visit_path(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
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
        self.visit(source);
        if let Some(filter_expr) = filter {
            self.visit(filter_expr);
        }
        if let Some(map_expr) = map {
            self.visit(map_expr);
        }
    }

    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        self.visit(tag);
    }

    fn visit_tag_property(&mut self, _tag_name: &str, _property: &str) {}

    fn visit_edge_property(&mut self, _edge_name: &str, _property: &str) {}

    fn visit_predicate(&mut self, _func: &str, args: &[Expression]) {
        for arg in args {
            self.visit(arg);
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
        self.visit(initial);
        self.visit(source);
        self.visit(mapping);
    }

    fn visit_path_build(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    fn visit_parameter(&mut self, _name: &str) {}
}

/// Variable Collector
///
/// Collect all the variable names that are used in the expression.
///
/// # Examples
///
/// ```rust
/// use crate::core::types::expr::visitor::VariableCollector;
/// use crate::core::Expression;
///
/// let expr = Expression::variable("a");
/// let mut collector = VariableCollector::new();
/// collector.visit(&expr);
/// assert_eq!(collector.variables, vec!["a".to_string()]);
/// ```
#[derive(Debug, Default)]
pub struct VariableCollector {
    /// List of variable names collected
    pub variables: Vec<String>,
}

impl VariableCollector {
    /// Create a new variable collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear the collector
    pub fn clear(&mut self) {
        self.variables.clear();
    }
}

impl ExpressionVisitor for VariableCollector {
    fn visit_literal(&mut self, _value: &crate::core::Value) {}

    fn visit_variable(&mut self, name: &str) {
        let var_name = name.to_string();
        if !self.variables.contains(&var_name) {
            self.variables.push(var_name);
        }
    }

    fn visit_property(&mut self, object: &Expression, _property: &str) {
        self.visit(object);
    }

    fn visit_binary(&mut self, _op: BinaryOperator, left: &Expression, right: &Expression) {
        self.visit(left);
        self.visit(right);
    }

    fn visit_unary(&mut self, _op: UnaryOperator, operand: &Expression) {
        self.visit(operand);
    }

    fn visit_function(&mut self, _name: &str, args: &[Expression]) {
        for arg in args {
            self.visit(arg);
        }
    }

    fn visit_aggregate(&mut self, _func: &AggregateFunction, arg: &Expression, _distinct: bool) {
        self.visit(arg);
    }

    fn visit_case(
        &mut self,
        _test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        for (when, then) in conditions {
            self.visit(when);
            self.visit(then);
        }
        if let Some(default_expr) = default {
            self.visit(default_expr);
        }
    }

    fn visit_list(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        for (_, value) in entries {
            self.visit(value);
        }
    }

    fn visit_type_cast(
        &mut self,
        expression: &Expression,
        _target_type: &crate::core::types::DataType,
    ) {
        self.visit(expression);
    }

    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        self.visit(collection);
        self.visit(index);
    }

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

    fn visit_path(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
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
        self.visit(source);
        if let Some(filter_expr) = filter {
            self.visit(filter_expr);
        }
        if let Some(map_expr) = map {
            self.visit(map_expr);
        }
    }

    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        self.visit(tag);
    }

    fn visit_tag_property(&mut self, _tag_name: &str, _property: &str) {}

    fn visit_edge_property(&mut self, _edge_name: &str, _property: &str) {}

    fn visit_predicate(&mut self, _func: &str, args: &[Expression]) {
        for arg in args {
            self.visit(arg);
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
        self.visit(initial);
        self.visit(source);
        self.visit(mapping);
    }

    fn visit_path_build(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    fn visit_parameter(&mut self, _name: &str) {}
}

/// Function collector
///
/// Collect all the function names that are used in the expression.
///
/// # Examples
///
/// ```rust
/// use crate::core::types::expr::visitor::FunctionCollector;
/// use crate::core::Expression;
///
/// let expr = Expression::function("count", vec![Expression::variable("a")]);
/// let mut collector = FunctionCollector::new();
/// collector.visit(&expr);
/// assert!(collector.functions.contains(&"count".to_string()));
/// ```
#[derive(Debug, Default)]
pub struct FunctionCollector {
    /// List of function names collected
    pub functions: Vec<String>,
}

impl FunctionCollector {
    /// Create a new function collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear the collector
    pub fn clear(&mut self) {
        self.functions.clear();
    }
}

impl ExpressionVisitor for FunctionCollector {
    fn visit_literal(&mut self, _value: &crate::core::Value) {}

    fn visit_variable(&mut self, _name: &str) {}

    fn visit_property(&mut self, object: &Expression, _property: &str) {
        self.visit(object);
    }

    fn visit_binary(
        &mut self,
        _op: crate::core::types::operators::BinaryOperator,
        left: &Expression,
        right: &Expression,
    ) {
        self.visit(left);
        self.visit(right);
    }

    fn visit_unary(
        &mut self,
        _op: crate::core::types::operators::UnaryOperator,
        operand: &Expression,
    ) {
        self.visit(operand);
    }

    fn visit_function(&mut self, name: &str, args: &[Expression]) {
        let func_name = name.to_string();
        if !self.functions.contains(&func_name) {
            self.functions.push(func_name);
        }
        for arg in args {
            self.visit(arg);
        }
    }

    fn visit_aggregate(&mut self, func: &AggregateFunction, arg: &Expression, _distinct: bool) {
        let func_name = format!("{:?}", func);
        if !self.functions.contains(&func_name) {
            self.functions.push(func_name);
        }
        self.visit(arg);
    }

    fn visit_case(
        &mut self,
        _test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        for (when, then) in conditions {
            self.visit(when);
            self.visit(then);
        }
        if let Some(default_expr) = default {
            self.visit(default_expr);
        }
    }

    fn visit_list(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        for (_, value) in entries {
            self.visit(value);
        }
    }

    fn visit_type_cast(&mut self, expression: &Expression, _target_type: &DataType) {
        self.visit(expression);
    }

    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        self.visit(collection);
        self.visit(index);
    }

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

    fn visit_path(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
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
        self.visit(source);
        if let Some(filter_expr) = filter {
            self.visit(filter_expr);
        }
        if let Some(map_expr) = map {
            self.visit(map_expr);
        }
    }

    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        self.visit(tag);
    }

    fn visit_tag_property(&mut self, _tag_name: &str, _property: &str) {}

    fn visit_edge_property(&mut self, _edge_name: &str, _property: &str) {}

    fn visit_predicate(&mut self, func: &str, args: &[Expression]) {
        let func_name = func.to_string();
        if !self.functions.contains(&func_name) {
            self.functions.push(func_name);
        }
        for arg in args {
            self.visit(arg);
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
        self.visit(initial);
        self.visit(source);
        self.visit(mapping);
    }

    fn visit_path_build(&mut self, items: &[Expression]) {
        for item in items {
            self.visit(item);
        }
    }

    fn visit_parameter(&mut self, _name: &str) {}
}
