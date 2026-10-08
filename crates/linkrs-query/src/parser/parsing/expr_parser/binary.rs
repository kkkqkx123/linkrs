use super::unary::{parse_not_expression, parse_unary_expression};
use super::{parse_postfix_expression, ParseResult};
use crate::parser::core::error::ParseError;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::BinaryOperator;

pub(crate) fn parse_or_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
    let mut left = parse_and_expression(ctx)?;

    while ctx.match_token(TokenKind::Or) {
        let op = BinaryOperator::Or;
        let right = parse_and_expression(ctx)?;
        let span = ctx.merge_span(left.span.start, right.span.end);
        left = ParseResult {
            expr: Expression::binary(left.expr, op, right.expr),
            span,
        };
    }

    Ok(left)
}

pub(crate) fn parse_and_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
    let mut left = parse_not_expression(ctx)?;

    while ctx.match_token(TokenKind::And) {
        let op = BinaryOperator::And;
        let right = parse_not_expression(ctx)?;
        let span = ctx.merge_span(left.span.start, right.span.end);
        left = ParseResult {
            expr: Expression::binary(left.expr, op, right.expr),
            span,
        };
    }

    Ok(left)
}

pub(crate) fn parse_comparison_expression(
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    let mut left = parse_bitwise_expression(ctx)?;

    if let Some(op) = parse_comparison_op(ctx) {
        let right = parse_additive_expression(ctx)?;
        let span = ctx.merge_span(left.span.start, right.span.end);
        left = ParseResult {
            expr: Expression::binary(left.expr, op, right.expr),
            span,
        };
    }

    Ok(left)
}

pub(crate) fn parse_comparison_op(ctx: &mut ParseContext<'_>) -> Option<BinaryOperator> {
    match ctx.current_token().kind {
        TokenKind::Eq | TokenKind::Assign => {
            ctx.next_token();
            Some(BinaryOperator::Equal)
        }
        TokenKind::Ne => {
            ctx.next_token();
            Some(BinaryOperator::NotEqual)
        }
        TokenKind::Lt => {
            ctx.next_token();
            Some(BinaryOperator::LessThan)
        }
        TokenKind::Le => {
            ctx.next_token();
            Some(BinaryOperator::LessThanOrEqual)
        }
        TokenKind::Gt => {
            ctx.next_token();
            Some(BinaryOperator::GreaterThan)
        }
        TokenKind::Ge => {
            ctx.next_token();
            Some(BinaryOperator::GreaterThanOrEqual)
        }
        TokenKind::Regex => {
            ctx.next_token();
            Some(BinaryOperator::Like)
        }
        TokenKind::Contains => {
            ctx.next_token();
            Some(BinaryOperator::Contains)
        }
        TokenKind::StartsWith => {
            ctx.next_token();
            ctx.match_token(TokenKind::With);
            Some(BinaryOperator::StartsWith)
        }
        TokenKind::EndsWith => {
            ctx.next_token();
            ctx.match_token(TokenKind::With);
            Some(BinaryOperator::EndsWith)
        }
        _ => None,
    }
}

pub(crate) fn parse_bitwise_expression(
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    let mut left = parse_additive_expression(ctx)?;

    while let Some(op) = parse_bitwise_op(ctx) {
        let right = parse_additive_expression(ctx)?;
        let span = ctx.merge_span(left.span.start, right.span.end);
        left = ParseResult {
            expr: Expression::binary(left.expr, op, right.expr),
            span,
        };
    }

    Ok(left)
}

pub(crate) fn parse_bitwise_op(ctx: &mut ParseContext<'_>) -> Option<BinaryOperator> {
    match ctx.current_token().kind {
        TokenKind::Pipe => {
            if ctx.is_pipe_or_suppressed() {
                return None;
            }
            ctx.next_token();
            Some(BinaryOperator::BitwiseOr)
        }
        TokenKind::Ampersand => {
            ctx.next_token();
            Some(BinaryOperator::BitwiseAnd)
        }
        TokenKind::ShiftLeft => {
            ctx.next_token();
            Some(BinaryOperator::ShiftLeft)
        }
        TokenKind::ShiftRight => {
            ctx.next_token();
            Some(BinaryOperator::ShiftRight)
        }
        _ => None,
    }
}

pub(crate) fn parse_additive_expression(
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    let mut left = parse_multiplicative_expression(ctx)?;

    while let Some(op) = parse_additive_op(ctx) {
        let right = parse_multiplicative_expression(ctx)?;
        let span = ctx.merge_span(left.span.start, right.span.end);
        left = ParseResult {
            expr: Expression::binary(left.expr, op, right.expr),
            span,
        };
    }

    Ok(left)
}

pub(crate) fn parse_additive_op(ctx: &mut ParseContext<'_>) -> Option<BinaryOperator> {
    match ctx.current_token().kind {
        TokenKind::Plus => {
            ctx.next_token();
            Some(BinaryOperator::Add)
        }
        TokenKind::Minus => {
            ctx.next_token();
            Some(BinaryOperator::Subtract)
        }
        _ => None,
    }
}

pub(crate) fn parse_multiplicative_expression(
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    let mut left = parse_unary_expression(ctx)?;

    while let Some(op) = parse_multiplicative_op(ctx) {
        let right = parse_unary_expression(ctx)?;
        let span = ctx.merge_span(left.span.start, right.span.end);
        left = ParseResult {
            expr: Expression::binary(left.expr, op, right.expr),
            span,
        };
    }

    Ok(left)
}

pub(crate) fn parse_multiplicative_op(ctx: &mut ParseContext<'_>) -> Option<BinaryOperator> {
    match ctx.current_token().kind {
        TokenKind::Star => {
            ctx.next_token();
            Some(BinaryOperator::Multiply)
        }
        TokenKind::Div => {
            ctx.next_token();
            Some(BinaryOperator::Divide)
        }
        TokenKind::Mod => {
            ctx.next_token();
            Some(BinaryOperator::Modulo)
        }
        _ => None,
    }
}

pub(crate) fn parse_exponentiation_expression(
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    let mut expression = parse_postfix_expression(ctx)?;

    if ctx.match_token(TokenKind::Exp) {
        let mut right_operands = Vec::new();

        while ctx.match_token(TokenKind::Exp) {
            right_operands.push(parse_unary_expression(ctx)?);
        }

        for operand in right_operands.into_iter().rev() {
            let span = ctx.merge_span(expression.span.start, operand.span.end);
            expression = ParseResult {
                expr: Expression::binary(expression.expr, BinaryOperator::Exponent, operand.expr),
                span,
            };
        }
    }

    Ok(expression)
}
