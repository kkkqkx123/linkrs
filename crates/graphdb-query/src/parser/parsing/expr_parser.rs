//! Expression parsing module
//!
//! Provides functions to parse expressions from token streams into
//! the core Expression representation.

use std::sync::Arc;

use crate::parser::core::error::ParseError;
use crate::parser::parsing::parse_context::ParseContext;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::{ContextualExpression, Expression, ExpressionMeta, SubqueryBody};

mod binary;
mod container;
mod function;
mod property;
mod subquery;
#[cfg(test)]
mod tests;
mod unary;

pub(crate) use binary::parse_or_expression;
pub(crate) use property::parse_postfix_expression;

/// Expression parse result with span information.
pub struct ParseResult {
    pub expr: Expression,
    pub span: graphdb_core::types::Span,
}

/// Parse an expression and return the result with span.
pub fn parse_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
    parse_or_expression(ctx)
}

/// Parse an expression and return the ContextualExpression.
pub fn parse_expression_with_context(
    ctx: &mut ParseContext<'_>,
    expr_ctx: Arc<ExpressionAnalysisContext>,
) -> Result<ContextualExpression, ParseError> {
    let result = parse_expression(ctx)?;
    let expr_meta = ExpressionMeta::with_span(result.expr, result.span);
    let id = expr_ctx.register_expression(expr_meta);
    Ok(ContextualExpression::new(id, expr_ctx))
}

/// Parse a property-path expression (identifier or literal with optional
/// `.property` access) and return the ContextualExpression.
///
/// Unlike [`parse_expression_with_context`], this does NOT treat `=` as a
/// comparison operator.  It is used where `=` is the assignment separator
/// rather than an equality comparison (e.g. the LHS of SET / UPDATE
/// assignments such as `SET p.age = 30`), so the LHS expression stops at
/// the `=` token.
pub fn parse_property_path_with_context(
    ctx: &mut ParseContext<'_>,
    expr_ctx: Arc<ExpressionAnalysisContext>,
) -> Result<ContextualExpression, ParseError> {
    let result = parse_postfix_expression(ctx)?;
    let expr_meta = ExpressionMeta::with_span(result.expr, result.span);
    let id = expr_ctx.register_expression(expr_meta);
    Ok(ContextualExpression::new(id, expr_ctx))
}

pub(crate) fn parse_function_call(
    name: String,
    span: graphdb_core::types::Span,
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    function::parse_function_call(name, span, ctx)
}

pub(crate) fn parse_sql_subquery_body(
    ctx: &mut ParseContext<'_>,
) -> Result<SubqueryBody, ParseError> {
    subquery::parse_sql_subquery_body(ctx)
}

pub(crate) fn parse_subquery_body(ctx: &mut ParseContext<'_>) -> Result<SubqueryBody, ParseError> {
    subquery::parse_subquery_body(ctx)
}
