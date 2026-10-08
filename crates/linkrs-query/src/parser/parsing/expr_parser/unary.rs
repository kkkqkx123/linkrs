use super::binary::{parse_comparison_expression, parse_exponentiation_expression};
use super::ParseResult;
use crate::parser::core::error::ParseError;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::UnaryOperator;

pub(crate) fn parse_not_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
    if ctx.match_token(TokenKind::Not) {
        let op = UnaryOperator::Not;
        let operand = parse_not_expression(ctx)?;
        let span = ctx.merge_span(operand.span.start, operand.span.end);
        Ok(ParseResult {
            expr: Expression::unary(op, operand.expr),
            span,
        })
    } else {
        parse_comparison_expression(ctx)
    }
}

pub(crate) fn parse_unary_expression(
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    if ctx.match_token(TokenKind::Minus) {
        let op = UnaryOperator::Minus;
        let operand = parse_unary_expression(ctx)?;
        let span = ctx.merge_span(operand.span.start, operand.span.end);
        Ok(ParseResult {
            expr: Expression::unary(op, operand.expr),
            span,
        })
    } else if ctx.match_token(TokenKind::Plus) {
        let op = UnaryOperator::Plus;
        let operand = parse_unary_expression(ctx)?;
        let span = ctx.merge_span(operand.span.start, operand.span.end);
        Ok(ParseResult {
            expr: Expression::unary(op, operand.expr),
            span,
        })
    } else if ctx.match_token(TokenKind::NotOp) {
        let op = UnaryOperator::Not;
        let operand = parse_unary_expression(ctx)?;
        let span = ctx.merge_span(operand.span.start, operand.span.end);
        Ok(ParseResult {
            expr: Expression::unary(op, operand.expr),
            span,
        })
    } else {
        parse_exponentiation_expression(ctx)
    }
}
