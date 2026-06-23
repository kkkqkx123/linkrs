//! Expression Metadata Wrapper
//!
//! This module defines the ExpressionMeta type, which is a wrapper around the core Expression.
//! Contains metadata such as location information (Span) and expression ID.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::Expression;
use crate::core::types::{Position, Span};
use crate::core::Value;

/// Expression ID for caching and tracing
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExpressionId(pub u64);

impl ExpressionId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Expression Metadata Wrapper
///
/// Wrappers for core expressions are provided:
/// - Location information (for error reporting)
/// - Expression ID (for caching)
/// - Expression reuse (Arc)
///
/// # Examples
///
/// ```rust
/// use crate::core::types::{Expression, ExpressionMeta, Span, Position};
///
/// let expr = Expression::literal(42);
/// let meta = ExpressionMeta::with_span(expr, Span::new(Position::new(1, 1), Position::new(1, 2)));
/// assert!(meta.span().is_some());
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "ExpressionMetaSerde", into = "ExpressionMetaSerde")]
pub struct ExpressionMeta {
    pub(crate) inner: Arc<Expression>,
    span: Option<Span>,
    id: Option<ExpressionId>,
}

/// Serialized Auxiliary Structures
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ExpressionMetaSerde {
    inner: Expression,
    #[serde(skip_serializing_if = "Option::is_none")]
    span_line_start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span_col_start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span_line_end: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span_col_end: Option<usize>,
}

impl ExpressionMeta {
    /// Create a new expression metadata wrapper (without location information)
    ///
    /// # Example
    ///
    /// ```rust
    /// use crate::core::types::{Expression, ExpressionMeta};
    ///
    /// let expr = Expression::variable("x");
    /// let meta = ExpressionMeta::new(expr);
    /// assert!(meta.span().is_none());
    /// ```
    pub fn new(inner: Expression) -> Self {
        Self {
            inner: Arc::new(inner),
            span: None,
            id: None,
        }
    }

    /// Create and set location information
    ///
    /// # Example
    ///
    /// ```rust
    /// use crate::core::types::{Expression, ExpressionMeta, Span, Position};
    ///
    /// let expr = Expression::literal("test");
    /// let span = Span::new(Position::new(5, 10), Position::new(5, 14));
    /// let meta = ExpressionMeta::with_span(expr, span);
    /// ```
    pub fn with_span(inner: Expression, span: Span) -> Self {
        Self {
            inner: Arc::new(inner),
            span: Some(span),
            id: None,
        }
    }

    /// Creating and setting expression IDs
    pub fn with_id(mut self, id: ExpressionId) -> Self {
        self.id = Some(id);
        self
    }

    /// Getting Location Information
    pub fn span(&self) -> Option<&Span> {
        self.span.as_ref()
    }

    /// Get expression ID
    pub fn id(&self) -> Option<&ExpressionId> {
        self.id.as_ref()
    }

    /// Getting internal references
    pub fn inner(&self) -> &Expression {
        &self.inner
    }

    /// Cloning internal expressions (no metadata cloning)
    ///
    /// Note: This method clones the entire expression tree, so use caution if the expression is large.
    pub fn into_inner(self) -> Expression {
        self.inner.as_ref().clone()
    }

    /// Get variable internal references (clone if necessary)
    ///
    /// If Arc is a unique reference, return the mutable reference directly;
    /// Otherwise clone the internal expression
    pub fn make_mut(&mut self) -> &mut Expression {
        if Arc::get_mut(&mut self.inner).is_none() {
            let cloned = self.inner.as_ref().clone();
            self.inner = Arc::new(cloned);
        }
        Arc::get_mut(&mut self.inner).expect("Arc should be unique after cloning")
    }

    /// Check for literal quantities
    pub fn is_literal(&self) -> bool {
        self.inner.as_ref().is_literal()
    }

    /// Get Literals
    pub fn as_literal(&self) -> Option<&Value> {
        self.inner.as_ref().as_literal()
    }

    /// Check if the variable
    pub fn is_variable(&self) -> bool {
        self.inner.as_ref().is_variable()
    }

    /// Get variable name
    pub fn as_variable(&self) -> Option<&str> {
        self.inner.as_ref().as_variable()
    }

    /// Checking for Aggregate Expressions
    pub fn is_aggregate(&self) -> bool {
        self.inner.as_ref().is_aggregate()
    }

    /// Getting a list of variables
    pub fn get_variables(&self) -> Vec<String> {
        self.inner.as_ref().get_variables()
    }

    /// Convert to string representation
    pub fn to_expression_string(&self) -> String {
        self.inner.as_ref().to_expression_string()
    }

    /// Get all subexpressions
    pub fn children(&self) -> Vec<&Expression> {
        self.inner.as_ref().children()
    }

    /// Checking for the inclusion of aggregate functions
    pub fn contains_aggregate(&self) -> bool {
        self.inner.as_ref().contains_aggregate()
    }

    /// Getting references to internal expressions
    pub fn expression(&self) -> &Expression {
        &self.inner
    }

    /// Check if it is a function call
    pub fn is_function(&self) -> bool {
        self.inner.as_ref().is_function()
    }

    /// Checks if it is a path expression
    pub fn is_path(&self) -> bool {
        self.inner.as_ref().is_path()
    }

    /// Checks if it is a path building expression
    pub fn is_path_build(&self) -> bool {
        self.inner.as_ref().is_path_build()
    }

    /// Checks if it's a tag expression
    pub fn is_label(&self) -> bool {
        self.inner.as_ref().is_label()
    }

    /// Checking for binary expressions
    pub fn is_binary(&self) -> bool {
        self.inner.as_ref().is_binary()
    }

    /// Checks if it is a unary expression
    pub fn is_unary(&self) -> bool {
        self.inner.as_ref().is_unary()
    }

    /// Checking for type conversion expressions
    pub fn is_type_cast(&self) -> bool {
        self.inner.as_ref().is_type_cast()
    }

    /// Checking for subscript access expressions
    pub fn is_subscript(&self) -> bool {
        self.inner.as_ref().is_subscript()
    }

    /// Checks if it is a range expression
    pub fn is_range(&self) -> bool {
        self.inner.as_ref().is_range()
    }

    /// Checks if it is a list expression
    pub fn is_list(&self) -> bool {
        self.inner.as_ref().is_list()
    }

    /// Checks if it is a mapping expression
    pub fn is_map(&self) -> bool {
        self.inner.as_ref().is_map()
    }

    /// Checks if it is a Case expression
    pub fn is_case(&self) -> bool {
        self.inner.as_ref().is_case()
    }

    /// Checks for a Reduce expression
    pub fn is_reduce(&self) -> bool {
        self.inner.as_ref().is_reduce()
    }

    /// Checks if it is a parameter expression
    pub fn is_parameter(&self) -> bool {
        self.inner.as_ref().is_parameter()
    }

    /// Checking for list derivatives
    pub fn is_list_comprehension(&self) -> bool {
        self.inner.as_ref().is_list_comprehension()
    }

    /// Get the function name (if it's a function call)
    pub fn as_function_name(&self) -> Option<String> {
        self.inner.as_ref().as_function_name()
    }

    /// Obtain the attribute name (in the case of attribute access)
    pub fn as_property_name(&self) -> Option<String> {
        self.inner.as_ref().as_property_name()
    }

    /// Obtain the tag name (if it is a tag expression).
    pub fn as_label_name(&self) -> Option<String> {
        self.inner.as_ref().as_label_name()
    }

    /// Obtain the parameter name (if it is a parameter expression).
    pub fn as_parameter_name(&self) -> Option<String> {
        self.inner.as_ref().as_parameter_name()
    }
}

/// Extract the core expressions from ExpressionMeta.
impl From<ExpressionMeta> for Expression {
    fn from(meta: ExpressionMeta) -> Self {
        meta.into_inner()
    }
}

/// Create from the core expression
impl From<Expression> for ExpressionMeta {
    fn from(expr: Expression) -> Self {
        ExpressionMeta::new(expr)
    }
}

/// Extract the core expressions from Arc ExpressionMeta.
impl From<Arc<ExpressionMeta>> for Expression {
    fn from(meta: Arc<ExpressionMeta>) -> Self {
        meta.inner().clone()
    }
}

/// Implementing the PartialEq method for ExpressionMeta (comparing internal expressions)
impl PartialEq for ExpressionMeta {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

/// Serialization implementation
impl From<ExpressionMetaSerde> for ExpressionMeta {
    fn from(s: ExpressionMetaSerde) -> Self {
        let span = s.span_line_start.and_then(|start_line| {
            let start_col = s.span_col_start?;
            let end_line = s.span_line_end?;
            let end_col = s.span_col_end?;
            Some(Span::new(
                Position::new(start_line, start_col),
                Position::new(end_line, end_col),
            ))
        });
        Self {
            inner: Arc::new(s.inner),
            span,
            id: None,
        }
    }
}

impl From<ExpressionMeta> for ExpressionMetaSerde {
    fn from(m: ExpressionMeta) -> Self {
        Self {
            inner: m.inner.as_ref().clone(),
            span_line_start: m.span.as_ref().map(|s| s.start.line),
            span_col_start: m.span.as_ref().map(|s| s.start.column),
            span_line_end: m.span.as_ref().map(|s| s.end.line),
            span_col_end: m.span.as_ref().map(|s| s.end.column),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::BinaryOperator;

    #[test]
    fn test_expression_meta_creation() {
        let expr = Expression::literal(42);
        let meta = ExpressionMeta::new(expr);
        assert!(meta.span().is_none());
        assert!(meta.id().is_none());
    }

    #[test]
    fn test_expression_meta_with_span() {
        let expr = Expression::variable("x");
        let span = Span::new(Position::new(1, 0), Position::new(1, 1));
        let meta = ExpressionMeta::with_span(expr, span);
        assert!(meta.span().is_some());
        assert_eq!(meta.span().expect("Expected span to exist").start.line, 1);
    }

    #[test]
    fn test_expression_meta_with_id() {
        let expr = Expression::literal(true);
        let meta = ExpressionMeta::new(expr).with_id(ExpressionId::new(42));
        assert!(meta.id().is_some());
        assert_eq!(meta.id().expect("Expected id to exist").0, 42);
    }

    #[test]
    fn test_expression_meta_into_inner() {
        let expr = Expression::literal("test");
        let meta = ExpressionMeta::new(expr);
        let inner: Expression = meta.into();
        assert!(matches!(inner, Expression::Literal(_)));
    }

    #[test]
    fn test_expression_meta_make_mut_shared() {
        let expr = Expression::variable("a");
        let mut meta1 = ExpressionMeta::new(expr);
        let _meta2 = meta1.clone();

        let inner_arc = &meta1.inner;
        assert!(Arc::strong_count(inner_arc) > 1);

        let _ = meta1.make_mut();
        let inner_arc = &meta1.inner;
        assert_eq!(Arc::strong_count(inner_arc), 1);
    }

    #[test]
    fn test_expression_meta_is_literal() {
        let expr = Expression::literal(42);
        let meta = ExpressionMeta::new(expr);
        assert!(meta.is_literal());

        let expr = Expression::variable("x");
        let meta = ExpressionMeta::new(expr);
        assert!(!meta.is_literal());
    }

    #[test]
    fn test_expression_meta_as_literal() {
        let expr = Expression::literal(42);
        let meta = ExpressionMeta::new(expr);
        assert!(meta.as_literal().is_some());
    }

    #[test]
    fn test_expression_meta_is_variable() {
        let expr = Expression::variable("x");
        let meta = ExpressionMeta::new(expr);
        assert!(meta.is_variable());

        let expr = Expression::literal(42);
        let meta = ExpressionMeta::new(expr);
        assert!(!meta.is_variable());
    }

    #[test]
    fn test_expression_meta_as_variable() {
        let expr = Expression::variable("count");
        let meta = ExpressionMeta::new(expr);
        assert_eq!(meta.as_variable(), Some("count"));
    }

    #[test]
    fn test_expression_meta_partial_eq() {
        let expr1 = Expression::literal(42);
        let expr2 = Expression::literal(42);
        let meta1 = ExpressionMeta::new(expr1);
        let meta2 = ExpressionMeta::new(expr2);
        assert_eq!(meta1, meta2);

        let expr3 = Expression::literal(100);
        let meta3 = ExpressionMeta::new(expr3);
        assert_ne!(meta1, meta3);
    }

    #[test]
    fn test_expression_meta_serde() {
        let expr = Expression::literal("test");
        let meta = ExpressionMeta::new(expr);
        let json = serde_json::to_string(&meta).expect("Serialization should succeed");
        let decoded: ExpressionMeta =
            serde_json::from_str(&json).expect("Deserialization should succeed");
        assert_eq!(meta, decoded);
    }

    #[test]
    fn test_expression_meta_serde_with_span() {
        let expr = Expression::literal(42);
        let span = Span::new(Position::new(1, 5), Position::new(1, 10));
        let meta = ExpressionMeta::with_span(expr, span);
        let json = serde_json::to_string(&meta).expect("Serialization should succeed");
        let decoded: ExpressionMeta =
            serde_json::from_str(&json).expect("Deserialization should succeed");
        assert!(decoded.span().is_some());
        assert_eq!(
            decoded.span().expect("Expected span to exist").start.line,
            1
        );
    }

    #[test]
    fn test_expression_meta_get_variables() {
        let expr = Expression::binary(
            Expression::variable("a"),
            BinaryOperator::Add,
            Expression::variable("b"),
        );
        let meta = ExpressionMeta::new(expr);
        let vars = meta.get_variables();
        assert_eq!(vars, vec!["a", "b"]);
    }

    #[test]
    fn test_expression_meta_to_string() {
        let expr = Expression::binary(
            Expression::variable("x"),
            BinaryOperator::Add,
            Expression::literal(1),
        );
        let meta = ExpressionMeta::new(expr);
        let s = meta.to_expression_string();
        assert!(s.contains("+"));
    }
}
