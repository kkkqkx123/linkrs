use super::property::try_parse_inline_pattern_predicate;
use super::{parse_expression, parse_function_call, ParseResult};
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::Position;
use graphdb_core::{ArrayValue, NullType, StructValue, Value};

pub(crate) fn parse_primary_expression(
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    let token = ctx.current_token().clone();
    let start_pos = ctx.current_position();

    match token.kind {
        TokenKind::LParen => {
            if let Some(inline) = try_parse_inline_pattern_predicate(ctx, start_pos)? {
                return Ok(inline);
            }
            ctx.next_token();

            let ckpt = ctx.checkpoint();
            if let TokenKind::Identifier(_) = ctx.current_token().kind {
                let first_name = ctx.current_token().lexeme.clone();
                ctx.next_token();

                if ctx.check_token(TokenKind::Arrow) {
                    ctx.next_token();
                    let body = parse_expression(ctx)?;
                    ctx.expect_token(TokenKind::RParen)?;
                    let span = ctx.merge_span(start_pos, ctx.current_position());
                    return Ok(ParseResult {
                        expr: Expression::lambda(vec![first_name], body.expr),
                        span,
                    });
                }

                let mut params = vec![first_name];
                while ctx.match_token(TokenKind::Comma) {
                    if let TokenKind::Identifier(ref name) = ctx.current_token().kind {
                        let name = name.clone();
                        ctx.next_token();
                        params.push(name);

                        if ctx.check_token(TokenKind::Arrow) {
                            ctx.next_token();
                            let body = parse_expression(ctx)?;
                            ctx.expect_token(TokenKind::RParen)?;
                            let span = ctx.merge_span(start_pos, ctx.current_position());
                            return Ok(ParseResult {
                                expr: Expression::lambda(params, body.expr),
                                span,
                            });
                        }
                    } else {
                        break;
                    }
                }
            }

            ctx.restore(ckpt);
            let expression = ctx.with_pipe_or_suppression(false, |ctx| parse_expression(ctx))?;
            ctx.expect_token(TokenKind::RParen)?;
            Ok(expression)
        }
        TokenKind::Identifier(name) => {
            ctx.next_token();
            let span = ctx.merge_span(start_pos, ctx.current_position());
            if ctx.match_token(TokenKind::LParen) {
                parse_function_call(name, span, ctx)
            } else if !ctx.is_edge_syntax_mode()
                && ctx.check_token(TokenKind::Arrow)
                && !matches!(ctx.peek_token().kind, TokenKind::StringLiteral(_))
            {
                ctx.next_token();
                let body = parse_expression(ctx)?;
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::lambda(vec![name], body.expr),
                    span,
                })
            } else {
                Ok(ParseResult {
                    expr: Expression::variable(name),
                    span,
                })
            }
        }
        TokenKind::Edge => {
            ctx.next_token();
            let span = ctx.merge_span(start_pos, ctx.current_position());
            let mut expr = Expression::variable("edge".to_string());
            if ctx.match_token(TokenKind::Dot) {
                let prop_name = ctx.expect_identifier()?;
                expr = Expression::property(expr, prop_name);
            }
            Ok(ParseResult { expr, span })
        }
        TokenKind::IntegerLiteral(n) => {
            ctx.next_token();
            let span = ctx.merge_span(start_pos, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::literal(Value::BigInt(n)),
                span,
            })
        }
        TokenKind::FloatLiteral(f) => {
            ctx.next_token();
            let span = ctx.merge_span(start_pos, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::literal(Value::Double(f)),
                span,
            })
        }
        TokenKind::StringLiteral(s) => {
            ctx.next_token();
            let span = ctx.merge_span(start_pos, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::literal(Value::string(s)),
                span,
            })
        }
        TokenKind::BooleanLiteral(b) => {
            ctx.next_token();
            let span = ctx.merge_span(start_pos, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::literal(Value::Bool(b)),
                span,
            })
        }
        TokenKind::Null => {
            ctx.next_token();
            let span = ctx.merge_span(start_pos, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::literal(Value::Null(NullType::Null)),
                span,
            })
        }
        TokenKind::Count => {
            let next = ctx.peek_token();
            if next.kind == TokenKind::LBrace {
                ctx.next_token();
                ctx.next_token();
                let body = super::parse_subquery_body(ctx)?;
                ctx.expect_token(TokenKind::RBrace)?;
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::count_subquery(body),
                    span,
                })
            } else {
                let func_name = token.lexeme.clone();
                ctx.next_token();
                let span = ctx.merge_span(start_pos, ctx.current_position());
                if ctx.match_token(TokenKind::LParen) {
                    parse_function_call(func_name, span, ctx)
                } else {
                    Ok(ParseResult {
                        expr: Expression::variable(func_name),
                        span,
                    })
                }
            }
        }
        TokenKind::Sum | TokenKind::Avg | TokenKind::Min | TokenKind::Max => {
            let func_name = token.lexeme.clone();
            ctx.next_token();
            let span = ctx.merge_span(start_pos, ctx.current_position());
            if ctx.match_token(TokenKind::LParen) {
                parse_function_call(func_name, span, ctx)
            } else {
                Ok(ParseResult {
                    expr: Expression::variable(func_name),
                    span,
                })
            }
        }
        TokenKind::User
        | TokenKind::Order
        | TokenKind::Status
        | TokenKind::Contains
        | TokenKind::Tag
        | TokenKind::Tags
        | TokenKind::Path
        | TokenKind::Vertex
        | TokenKind::Vertices
        | TokenKind::Edges
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
        | TokenKind::Geography => {
            let name = token.lexeme.clone();
            ctx.next_token();
            let span = ctx.merge_span(start_pos, ctx.current_position());
            if ctx.match_token(TokenKind::LParen) {
                parse_function_call(name, span, ctx)
            } else {
                Ok(ParseResult {
                    expr: Expression::variable(name),
                    span,
                })
            }
        }
        TokenKind::List => {
            ctx.next_token();
            if ctx.match_token(TokenKind::LParen) {
                let span = ctx.merge_span(start_pos, ctx.current_position());
                parse_function_call("list".into(), span, ctx)
            } else {
                let elements = parse_expression_list(ctx)?;
                ctx.expect_token(TokenKind::RBracket)?;
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::list(elements.into_iter().map(|e| e.expr).collect()),
                    span,
                })
            }
        }
        TokenKind::LBracket => {
            ctx.next_token();
            if ctx.is_identifier_or_in_token() {
                parse_list_comprehension(start_pos, ctx)
            } else if ctx.match_token(TokenKind::RBracket) {
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::list(Vec::new()),
                    span,
                })
            } else {
                let elements =
                    ctx.with_pipe_or_suppression(false, |ctx| parse_expression_list(ctx))?;
                ctx.expect_token(TokenKind::RBracket)?;
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::list(elements.into_iter().map(|e| e.expr).collect()),
                    span,
                })
            }
        }
        TokenKind::Case => parse_case_expression(start_pos, ctx),
        TokenKind::Struct => {
            ctx.next_token();
            ctx.expect_token(TokenKind::LBrace)?;
            let fields = parse_property_list(ctx)?;
            ctx.expect_token(TokenKind::RBrace)?;
            let span = ctx.merge_span(start_pos, ctx.current_position());
            let mut values = Vec::with_capacity(fields.len());
            for (name, result) in fields {
                let value = eval_literal_expression(&result.expr, span.start)?;
                values.push((name, value));
            }
            Ok(ParseResult {
                expr: Expression::literal(Value::Struct(Box::new(StructValue::new(values)))),
                span,
            })
        }
        TokenKind::Array => {
            ctx.next_token();
            ctx.expect_token(TokenKind::LBracket)?;
            let elements = parse_expression_list(ctx)?;
            ctx.expect_token(TokenKind::RBracket)?;
            let span = ctx.merge_span(start_pos, ctx.current_position());
            let mut values = Vec::with_capacity(elements.len());
            for result in elements {
                let value = eval_literal_expression(&result.expr, span.start)?;
                values.push(value);
            }
            Ok(ParseResult {
                expr: Expression::literal(Value::Array(Box::new(ArrayValue::new(values)))),
                span,
            })
        }
        TokenKind::Map => {
            ctx.next_token();
            ctx.expect_token(TokenKind::LBrace)?;
            let properties = parse_property_list(ctx)?;
            ctx.expect_token(TokenKind::RBrace)?;
            let span = ctx.merge_span(start_pos, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::map(properties.into_iter().map(|(k, v)| (k, v.expr)).collect()),
                span,
            })
        }
        TokenKind::LBrace => {
            ctx.next_token();
            let properties = parse_property_list(ctx)?;
            ctx.expect_token(TokenKind::RBrace)?;
            let span = ctx.merge_span(start_pos, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::map(properties.into_iter().map(|(k, v)| (k, v.expr)).collect()),
                span,
            })
        }
        TokenKind::InputRef => {
            ctx.next_token();
            let mut span = ctx.merge_span(start_pos, ctx.current_position());
            let mut expr = Expression::variable("$-");
            if ctx.match_token(TokenKind::Dot) {
                let prop_name = ctx.expect_identifier()?;
                expr = Expression::property(expr, prop_name);
                span = ctx.merge_span(start_pos, ctx.current_position());
            }
            Ok(ParseResult { expr, span })
        }
        TokenKind::SrcRef => {
            ctx.next_token();
            let mut span = ctx.merge_span(start_pos, ctx.current_position());
            let mut expr = Expression::variable("$^");
            if ctx.match_token(TokenKind::Dot) {
                let prop_name = ctx.expect_identifier()?;
                expr = Expression::property(expr, prop_name);
                span = ctx.merge_span(start_pos, ctx.current_position());
            }
            Ok(ParseResult { expr, span })
        }
        TokenKind::DstRef => {
            ctx.next_token();
            let mut span = ctx.merge_span(start_pos, ctx.current_position());
            let mut expr = Expression::variable("$$");
            if ctx.match_token(TokenKind::Dot) {
                let prop_name = ctx.expect_identifier()?;
                expr = Expression::property(expr, prop_name);
                span = ctx.merge_span(start_pos, ctx.current_position());
            }
            Ok(ParseResult { expr, span })
        }
        TokenKind::Dollar => {
            ctx.next_token();
            let var_name = ctx.expect_identifier()?;
            let mut span = ctx.merge_span(start_pos, ctx.current_position());
            let mut expr = Expression::session_variable(var_name);
            if ctx.match_token(TokenKind::Dot) {
                let prop_name = ctx.expect_identifier()?;
                expr = Expression::property(expr, prop_name);
                span = ctx.merge_span(start_pos, ctx.current_position());
            }
            Ok(ParseResult { expr, span })
        }
        TokenKind::At => {
            ctx.next_token();
            let param_name = ctx.expect_identifier()?;
            let mut span = ctx.merge_span(start_pos, ctx.current_position());
            let mut expr = Expression::parameter(param_name);
            if ctx.match_token(TokenKind::Dot) {
                let prop_name = ctx.expect_identifier()?;
                expr = Expression::property(expr, prop_name);
                span = ctx.merge_span(start_pos, ctx.current_position());
            }
            Ok(ParseResult { expr, span })
        }
        TokenKind::VectorLiteral(data) => {
            ctx.next_token();
            let span = ctx.merge_span(start_pos, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::vector(data),
                span,
            })
        }
        TokenKind::Exists => {
            ctx.next_token();
            ctx.expect_token(TokenKind::LBrace)?;
            let body = super::parse_subquery_body(ctx)?;
            ctx.expect_token(TokenKind::RBrace)?;
            let span = ctx.merge_span(start_pos, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::exists(body),
                span,
            })
        }
        TokenKind::Subquery => {
            ctx.next_token();
            ctx.expect_token(TokenKind::LBrace)?;
            let body = super::parse_subquery_body(ctx)?;
            ctx.expect_token(TokenKind::RBrace)?;
            if body.return_expr.is_none() {
                return Err(ParseError::new(
                    ParseErrorKind::SemanticError,
                    "SUBQUERY { ... } requires a RETURN clause with a single expression"
                        .to_string(),
                    start_pos,
                ));
            }
            let span = ctx.merge_span(start_pos, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::scalar_subquery(body),
                span,
            })
        }
        _ => Err(ParseError::new(
            ParseErrorKind::UnexpectedToken,
            format!("Unexpected token in expression: {:?}", token.kind),
            start_pos,
        )),
    }
}

pub(crate) fn parse_expression_list(
    ctx: &mut ParseContext<'_>,
) -> Result<Vec<ParseResult>, ParseError> {
    let mut expressions = Vec::new();
    expressions.push(parse_expression(ctx)?);
    while ctx.match_token(TokenKind::Comma) {
        expressions.push(parse_expression(ctx)?);
    }
    Ok(expressions)
}

pub(crate) fn parse_property_list(
    ctx: &mut ParseContext<'_>,
) -> Result<Vec<(String, ParseResult)>, ParseError> {
    let mut properties = Vec::new();
    while !ctx.check_token(TokenKind::RBrace) {
        let key = ctx.expect_identifier()?;
        ctx.expect_token(TokenKind::Colon)?;
        let value = parse_expression(ctx)?;
        properties.push((key, value));
        if !ctx.match_token(TokenKind::Comma) {
            break;
        }
    }
    Ok(properties)
}

pub(crate) fn eval_literal_expression(
    expr: &Expression,
    position: Position,
) -> Result<Value, ParseError> {
    use crate::executor::expression::evaluation_context::DefaultExpressionContext;
    use crate::executor::expression::evaluator::ExpressionEvaluator;

    let mut eval_ctx = DefaultExpressionContext::new();
    ExpressionEvaluator::evaluate(expr, &mut eval_ctx).map_err(|e| {
        ParseError::new(
            ParseErrorKind::SemanticError,
            format!("STRUCT/ARRAY literal elements must be constants: {}", e),
            position,
        )
    })
}

pub(crate) fn parse_case_expression(
    start_pos: Position,
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    ctx.expect_token(TokenKind::Case)?;

    let test_expr = if ctx.current_token().kind != TokenKind::When {
        Some(parse_expression(ctx)?.expr)
    } else {
        None
    };

    let mut conditions = Vec::new();
    while ctx.match_token(TokenKind::When) {
        let when_expr = parse_expression(ctx)?;
        ctx.expect_token(TokenKind::Then)?;
        let then_expr = parse_expression(ctx)?;
        conditions.push((when_expr.expr, then_expr.expr));
    }

    let default = if ctx.match_token(TokenKind::Else) {
        Some(parse_expression(ctx)?.expr)
    } else {
        None
    };

    ctx.expect_token(TokenKind::End)?;

    let span = ctx.merge_span(start_pos, ctx.current_position());
    Ok(ParseResult {
        expr: Expression::case(test_expr, conditions, default),
        span,
    })
}

pub(crate) fn parse_list_comprehension(
    start_pos: Position,
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    let variable = ctx.expect_identifier()?;
    ctx.expect_token(TokenKind::In)?;
    let source = ctx
        .with_pipe_or_suppression(true, |ctx| parse_expression(ctx))?
        .expr;

    let (filter, map) = if ctx.match_token(TokenKind::Pipe) {
        let map_expr = ctx.with_pipe_or_suppression(false, |ctx| parse_expression(ctx))?;
        (None, Some(map_expr.expr))
    } else if ctx.match_token(TokenKind::Where) {
        let filter_expr = ctx.with_pipe_or_suppression(true, |ctx| parse_expression(ctx))?;
        let map_expr = if ctx.match_token(TokenKind::Pipe) {
            Some(
                ctx.with_pipe_or_suppression(false, |ctx| parse_expression(ctx))?
                    .expr,
            )
        } else {
            None
        };
        (Some(filter_expr.expr), map_expr)
    } else {
        (None, None)
    };

    ctx.expect_token(TokenKind::RBracket)?;

    let span = ctx.merge_span(start_pos, ctx.current_position());
    Ok(ParseResult {
        expr: Expression::list_comprehension(variable, source, filter, map),
        span,
    })
}
