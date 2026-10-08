use super::container::parse_primary_expression;
use super::{parse_expression, parse_sql_subquery_body, parse_subquery_body, ParseResult};
use crate::parser::ast::pattern::Pattern;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::parsing::traversal_parser::TraversalParser;
use crate::parser::TokenKind;
use linkrs_core::types::expr::{Expression, SubqueryBody};
use linkrs_core::types::operators::{BinaryOperator, UnaryOperator};
use linkrs_core::types::{DataType, Position};
use linkrs_core::Value;

pub(crate) fn parse_postfix_expression(
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    let mut expression = parse_primary_expression(ctx)?;

    loop {
        if ctx.match_token(TokenKind::LBracket) {
            let index = parse_expression(ctx)?;
            ctx.expect_token(TokenKind::RBracket)?;
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: Expression::subscript(expression.expr, index.expr),
                span,
            };
        } else if ctx.match_token(TokenKind::Dot) {
            let property = ctx.expect_identifier()?;
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: if matches!(
                    expression.expr,
                    Expression::Property { .. }
                        | Expression::StructField { .. }
                        | Expression::Subscript { .. }
                ) {
                    Expression::struct_field(expression.expr, property)
                } else {
                    Expression::property(expression.expr, property)
                },
                span,
            };
        } else if ctx.match_token(TokenKind::IsNull) {
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: Expression::unary(UnaryOperator::IsNull, expression.expr),
                span,
            };
        } else if ctx.match_token(TokenKind::IsNotNull) {
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: Expression::unary(UnaryOperator::IsNotNull, expression.expr),
                span,
            };
        } else if ctx.match_token(TokenKind::IsEmpty) {
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: Expression::unary(UnaryOperator::IsEmpty, expression.expr),
                span,
            };
        } else if ctx.match_token(TokenKind::IsNotEmpty) {
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: Expression::unary(UnaryOperator::IsNotEmpty, expression.expr),
                span,
            };
        } else if ctx.match_token(TokenKind::DoubleColon) {
            let type_name = expect_cast_type_name(ctx)?;
            let span = ctx.merge_span(expression.span.start, ctx.current_position());

            if type_name.to_uppercase() == "VECTOR" {
                if let Expression::List(elements) = expression.expr.clone() {
                    let mut vector_data = Vec::with_capacity(elements.len());
                    for elem in elements {
                        if let Expression::Literal(Value::Double(f)) = elem {
                            vector_data.push(f as f32);
                        } else if let Expression::Literal(Value::Float(f)) = elem {
                            vector_data.push(f);
                        } else if let Expression::Literal(Value::Int(i)) = elem {
                            vector_data.push(i as f32);
                        } else if let Expression::Literal(Value::BigInt(i)) = elem {
                            vector_data.push(i as f32);
                        } else {
                            return Err(ParseError::new(
                                ParseErrorKind::SemanticError,
                                "Vector elements must be numeric literals".to_string(),
                                span.start,
                            ));
                        }
                    }
                    expression = ParseResult {
                        expr: Expression::vector(vector_data),
                        span,
                    };
                } else {
                    return Err(ParseError::new(
                        ParseErrorKind::SemanticError,
                        "Can only cast list literals to VECTOR".to_string(),
                        span.start,
                    ));
                }
            } else {
                let target_type = match type_name.parse::<DataType>() {
                    Ok(parsed) => parsed,
                    Err(e) => match ctx.resolve_named_type(&type_name) {
                        Some(resolved) if resolved != DataType::Unknown => resolved,
                        _ => {
                            return Err(ParseError::new(
                                ParseErrorKind::SyntaxError,
                                format!("Unknown type cast target: {}", e),
                                span.start,
                            ));
                        }
                    },
                };
                expression = ParseResult {
                    expr: Expression::TypeCast {
                        expression: Box::new(expression.expr),
                        target_type,
                    },
                    span,
                };
            }
        } else if (ctx.check_token(TokenKind::In) || ctx.check_token(TokenKind::NotIn))
            && (ctx.peek_token().kind == TokenKind::LBrace
                || ctx.peek_token().kind == TokenKind::LParen)
        {
            let negated = ctx.match_token(TokenKind::NotIn);
            ctx.match_token(TokenKind::In);
            let subquery = if ctx.match_token(TokenKind::LBrace) {
                let body = parse_subquery_body(ctx)?;
                ctx.expect_token(TokenKind::RBrace)?;
                body
            } else {
                ctx.expect_token(TokenKind::LParen)?;
                if !matches!(
                    ctx.current_token().kind,
                    TokenKind::Identifier(ref s) if s.eq_ignore_ascii_case("SELECT")
                ) {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        "Expected SELECT after IN (".to_string(),
                        ctx.current_position(),
                    ));
                }
                let body = parse_sql_subquery_body(ctx)?;
                ctx.expect_token(TokenKind::RParen)?;
                body
            };
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: Expression::in_subquery(expression.expr, subquery, negated),
                span,
            };
        } else if !ctx.is_edge_syntax_mode()
            && ctx.check_token(TokenKind::Arrow)
            && matches!(ctx.peek_token().kind, TokenKind::StringLiteral(_))
        {
            ctx.match_token(TokenKind::Arrow);
            let key = ctx.expect_string_literal()?;
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: Expression::binary(
                    expression.expr,
                    BinaryOperator::JsonGet,
                    Expression::literal(key),
                ),
                span,
            };
        } else if !ctx.is_edge_syntax_mode()
            && ctx.check_token(TokenKind::ArrowRight)
            && matches!(ctx.peek_token().kind, TokenKind::StringLiteral(_))
        {
            ctx.match_token(TokenKind::ArrowRight);
            let key = ctx.expect_string_literal()?;
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: Expression::binary(
                    expression.expr,
                    BinaryOperator::JsonGetText,
                    Expression::literal(key),
                ),
                span,
            };
        } else if !ctx.is_edge_syntax_mode()
            && ctx.check_token(TokenKind::HashArrow)
            && matches!(ctx.peek_token().kind, TokenKind::StringLiteral(_))
        {
            ctx.match_token(TokenKind::HashArrow);
            let path = ctx.expect_string_literal()?;
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: Expression::binary(
                    expression.expr,
                    BinaryOperator::JsonPathGet,
                    Expression::literal(path),
                ),
                span,
            };
        } else if !ctx.is_edge_syntax_mode()
            && ctx.check_token(TokenKind::HashArrowRight)
            && matches!(ctx.peek_token().kind, TokenKind::StringLiteral(_))
        {
            ctx.match_token(TokenKind::HashArrowRight);
            let path = ctx.expect_string_literal()?;
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: Expression::binary(
                    expression.expr,
                    BinaryOperator::JsonPathGetText,
                    Expression::literal(path),
                ),
                span,
            };
        } else if ctx.check_token(TokenKind::NotOp) {
            ctx.next_token();
            let span = ctx.merge_span(expression.span.start, ctx.current_position());
            expression = ParseResult {
                expr: Expression::function("factorial", vec![expression.expr]),
                span,
            };
        } else {
            break;
        }
    }

    Ok(expression)
}

pub(crate) fn expect_cast_type_name(ctx: &mut ParseContext<'_>) -> Result<String, ParseError> {
    let token = ctx.current_token().clone();
    match &token.kind {
        TokenKind::Identifier(_)
        | TokenKind::Bool
        | TokenKind::Int
        | TokenKind::Int8
        | TokenKind::Int16
        | TokenKind::Int32
        | TokenKind::Int64
        | TokenKind::Float
        | TokenKind::Double
        | TokenKind::String
        | TokenKind::FixedString
        | TokenKind::Timestamp
        | TokenKind::Date
        | TokenKind::Time
        | TokenKind::Datetime
        | TokenKind::Serial
        | TokenKind::Geography
        | TokenKind::List
        | TokenKind::Map
        | TokenKind::Struct
        | TokenKind::Array
        | TokenKind::UUID
        | TokenKind::Duration
        | TokenKind::KeywordVector => {
            let name = token.lexeme.clone();
            ctx.next_token();
            Ok(name)
        }
        _ => {
            let pos = ctx.current_position();
            Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!("Expected cast type name, found {:?}", token.kind),
                pos,
            )
            .with_expected_tokens(vec!["type name".to_string()]))
        }
    }
}

pub(crate) fn try_parse_inline_pattern_predicate(
    ctx: &mut ParseContext<'_>,
    start_pos: Position,
) -> Result<Option<ParseResult>, ParseError> {
    let ckpt = ctx.checkpoint();
    let pattern = match TraversalParser::new().parse_pattern(ctx) {
        Ok(Pattern::Path(path)) => Pattern::Path(path),
        _ => {
            ctx.take_recursive_comprehension();
            ctx.restore(ckpt);
            return Ok(None);
        }
    };
    if !is_pattern_predicate_follower(ctx) {
        ctx.restore(ckpt);
        return Ok(None);
    }
    let Some(pattern_str) = pattern.to_pattern_string() else {
        ctx.restore(ckpt);
        return Ok(None);
    };
    let span = ctx.merge_span(start_pos, ctx.current_position());
    let body = SubqueryBody {
        id: 0,
        patterns: vec![pattern_str],
        where_clause: None,
        return_expr: None,
    };
    Ok(Some(ParseResult {
        expr: Expression::exists(body),
        span,
    }))
}

pub(crate) fn is_pattern_predicate_follower(ctx: &ParseContext<'_>) -> bool {
    !matches!(
        ctx.current_token().kind,
        TokenKind::Eq
            | TokenKind::Assign
            | TokenKind::Ne
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::Regex
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Div
            | TokenKind::Mod
            | TokenKind::Exp
            | TokenKind::Ampersand
            | TokenKind::ShiftLeft
            | TokenKind::ShiftRight
            | TokenKind::NotOp
            | TokenKind::Dot
            | TokenKind::DoubleColon
            | TokenKind::Colon
            | TokenKind::DotDot
            | TokenKind::LBracket
            | TokenKind::LParen
            | TokenKind::Arrow
            | TokenKind::BackArrow
            | TokenKind::RightArrow
            | TokenKind::LeftArrow
            | TokenKind::ArrowRight
            | TokenKind::HashArrow
            | TokenKind::HashArrowRight
            | TokenKind::At
            | TokenKind::QMark
            | TokenKind::Question
            | TokenKind::In
            | TokenKind::NotIn
            | TokenKind::Is
            | TokenKind::IsNull
            | TokenKind::IsNotNull
            | TokenKind::IsEmpty
            | TokenKind::IsNotEmpty
            | TokenKind::Between
            | TokenKind::Contains
            | TokenKind::StartsWith
            | TokenKind::EndsWith
            | TokenKind::Not
    )
}
