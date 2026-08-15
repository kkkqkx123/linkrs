//! Expression parsing module
//!
//! Provides functions to parse expressions from token streams into
//! the core Expression representation.

use std::sync::Arc;

use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta, SubqueryBody};
use crate::core::types::operators::{BinaryOperator, UnaryOperator};
use crate::core::types::{DataType, Position, Span};
use crate::core::Value;
use crate::query::parser::core::error::{ParseError, ParseErrorKind};
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::TokenKind;

/// Expression parse result with span information.
pub struct ParseResult {
    pub expr: Expression,
    pub span: Span,
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

fn parse_or_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
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

fn parse_and_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
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

fn parse_not_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
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

fn parse_comparison_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
    let mut left = parse_additive_expression(ctx)?;

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

fn parse_comparison_op(ctx: &mut ParseContext<'_>) -> Option<BinaryOperator> {
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

fn parse_additive_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
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

fn parse_additive_op(ctx: &mut ParseContext<'_>) -> Option<BinaryOperator> {
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

fn parse_multiplicative_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
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

fn parse_multiplicative_op(ctx: &mut ParseContext<'_>) -> Option<BinaryOperator> {
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

fn parse_unary_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
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

fn parse_exponentiation_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
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

fn parse_postfix_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
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
                expr: Expression::property(expression.expr, property),
                span,
            };
        } else if ctx.match_token(TokenKind::DoubleColon) {
            let type_name = ctx.expect_identifier()?;
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
                let target_type = match type_name.to_uppercase().as_str() {
                    "BOOL" | "BOOLEAN" => DataType::Bool,
                    "INT" | "INTEGER" | "INT4" => DataType::Int,
                    "BIGINT" | "INT8" => DataType::BigInt,
                    "SMALLINT" | "INT2" => DataType::SmallInt,
                    "FLOAT" | "FLOAT4" => DataType::Float,
                    "DOUBLE" | "FLOAT8" | "DOUBLE PRECISION" => DataType::Double,
                    "STRING" | "TEXT" | "VARCHAR" => DataType::String,
                    "DATE" => DataType::Date,
                    "TIME" => DataType::Time,
                    "DATETIME" | "TIMESTAMP" => DataType::DateTime,
                    "LIST" => DataType::List,
                    "MAP" => DataType::Map,
                    "SET" => DataType::Set,
                    "JSON" => DataType::Json,
                    "JSONB" => DataType::JsonB,
                    "UUID" => DataType::Uuid,
                    "INTERVAL" => DataType::Interval,
                    "BLOB" => DataType::Blob,
                    "GEOGRAPHY" => DataType::Geography,
                    _ => {
                        return Err(ParseError::new(
                            ParseErrorKind::SyntaxError,
                            format!("Unknown type cast target: {}", type_name),
                            span.start,
                        ));
                    }
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
            && ctx.peek_token().kind == TokenKind::LBrace
        {
            let negated = ctx.match_token(TokenKind::NotIn);
            ctx.match_token(TokenKind::In);
            ctx.expect_token(TokenKind::LBrace)?;
            let subquery = parse_subquery_body(ctx)?;
            ctx.expect_token(TokenKind::RBrace)?;
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
        } else {
            break;
        }
    }

    Ok(expression)
}

fn parse_primary_expression(ctx: &mut ParseContext<'_>) -> Result<ParseResult, ParseError> {
    let token = ctx.current_token().clone();
    let start_pos = ctx.current_position();

    match token.kind {
        TokenKind::LParen => {
            ctx.next_token();
            let expression = parse_expression(ctx)?;
            ctx.expect_token(TokenKind::RParen)?;
            Ok(expression)
        }
        TokenKind::Identifier(name) => {
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
                expr: Expression::literal(Value::Null(crate::core::NullType::Null)),
                span,
            })
        }
        TokenKind::Count | TokenKind::Sum | TokenKind::Avg | TokenKind::Min | TokenKind::Max => {
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
        | TokenKind::Edges => {
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
                // LIST(...) used as a function call (e.g. COLLECT LIST(name)).
                let span = ctx.merge_span(start_pos, ctx.current_position());
                parse_function_call("list".to_string(), span, ctx)
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
                let elements = parse_expression_list(ctx)?;
                ctx.expect_token(TokenKind::RBracket)?;
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::list(elements.into_iter().map(|e| e.expr).collect()),
                    span,
                })
            }
        }
        TokenKind::Case => parse_case_expression(start_pos, ctx),
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
            let body = parse_subquery_body(ctx)?;
            ctx.expect_token(TokenKind::RBrace)?;
            let span = ctx.merge_span(start_pos, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::exists(body),
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

fn parse_function_call(
    name: String,
    span: Span,
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    let name_upper = name.to_uppercase();

    if ctx.match_token(TokenKind::Star) {
        ctx.expect_token(TokenKind::RParen)?;

        if name_upper == "COUNT" {
            return Ok(ParseResult {
                expr: Expression::Aggregate {
                    func: crate::core::types::operators::AggregateFunction::Count(None),
                    args: vec![Expression::Literal(crate::core::Value::string("*"))],
                    distinct: false,
                    filter: None,
                },
                span,
            });
        } else {
            return Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                format!("Could not apply aggregation function `{}` on `*`", name),
                ctx.current_position(),
            ));
        }
    }

    let args = if ctx.match_token(TokenKind::RParen) {
        Vec::new()
    } else {
        let args = parse_expression_list(ctx)?;
        ctx.expect_token(TokenKind::RParen)?;
        args
    };

    let is_aggregate = matches!(
        name_upper.as_str(),
        "COUNT"
            | "SUM"
            | "AVG"
            | "MIN"
            | "MAX"
            | "COLLECT"
            | "COLLECT_SET"
            | "STD"
            | "STDDEV_POP"
            | "STDDEV_SAMP"
            | "PRODUCT"
            | "PERCENTILE_CONT"
            | "VARIANCE"
            | "MEDIAN"
            | "MODE"
            | "BOOL_AND"
            | "BOOL_OR"
            | "PERCENTILE"
            | "DISTINCT"
            | "BIT_AND"
            | "BIT_OR"
            | "GROUP_CONCAT"
            | "VEC_SUM"
            | "VEC_AVG"
    );

    if is_aggregate {
        let distinct = ctx.match_token(TokenKind::Distinct);
        let arg = args.first().map(|a| a.expr.clone()).unwrap_or_else(|| {
            Expression::Literal(crate::core::Value::Null(crate::core::NullType::Null))
        });

        let field_name = match &arg {
            Expression::Variable(name) => name.clone(),
            Expression::Property { object, property } => {
                if let Expression::Variable(var_name) = object.as_ref() {
                    format!("{}.{}", var_name, property)
                } else {
                    property.clone()
                }
            }
            Expression::TagProperty { tag_name, property } => {
                format!("{}.{}", tag_name, property)
            }
            Expression::EdgeProperty {
                edge_name,
                property,
            } => {
                format!("{}.{}", edge_name, property)
            }
            _ => "_value".to_string(),
        };

        let func = match name_upper.as_str() {
            "COUNT" => crate::core::types::operators::AggregateFunction::Count(Some(field_name)),
            "SUM" => crate::core::types::operators::AggregateFunction::Sum(field_name),
            "AVG" => crate::core::types::operators::AggregateFunction::Avg(field_name),
            "MIN" => crate::core::types::operators::AggregateFunction::Min(field_name),
            "MAX" => crate::core::types::operators::AggregateFunction::Max(field_name),
            "COLLECT" => crate::core::types::operators::AggregateFunction::Collect(field_name),
            "COLLECT_SET" => {
                crate::core::types::operators::AggregateFunction::CollectSet(field_name)
            }
            "STD" => crate::core::types::operators::AggregateFunction::Std(field_name),
            "STDDEV_POP" => crate::core::types::operators::AggregateFunction::StddevPop(field_name),
            "STDDEV_SAMP" => {
                crate::core::types::operators::AggregateFunction::StddevSamp(field_name)
            }
            "PRODUCT" => crate::core::types::operators::AggregateFunction::Product(field_name),
            "PERCENTILE_CONT" => {
                let percentile = if args.len() > 1 {
                    match &args[1].expr {
                        Expression::Literal(crate::core::Value::Int(v)) => *v as f64,
                        Expression::Literal(crate::core::Value::BigInt(v)) => *v as f64,
                        Expression::Literal(crate::core::Value::Float(v)) => *v as f64,
                        Expression::Literal(crate::core::Value::Double(v)) => *v,
                        _ => 50.0,
                    }
                } else {
                    50.0
                };
                crate::core::types::operators::AggregateFunction::PercentileCont(
                    field_name, percentile,
                )
            }
            "VARIANCE" => crate::core::types::operators::AggregateFunction::Variance(field_name),
            "MEDIAN" => crate::core::types::operators::AggregateFunction::Median(field_name),
            "MODE" => crate::core::types::operators::AggregateFunction::Mode(field_name),
            "BOOL_AND" => crate::core::types::operators::AggregateFunction::BoolAnd(field_name),
            "BOOL_OR" => crate::core::types::operators::AggregateFunction::BoolOr(field_name),
            _ => crate::core::types::operators::AggregateFunction::Count(None),
        };

        let filter = if ctx.match_token(TokenKind::Filter) {
            ctx.expect_token(TokenKind::LParen)?;
            ctx.expect_token(TokenKind::Where)?;
            let filter_expr = parse_expression(ctx)?;
            ctx.expect_token(TokenKind::RParen)?;
            Some(Box::new(filter_expr.expr))
        } else {
            None
        };

        let span = ctx.merge_span(span.start, ctx.current_position());
        Ok(ParseResult {
            expr: Expression::Aggregate {
                func,
                args: vec![arg],
                distinct,
                filter,
            },
            span,
        })
    } else {
        let func_args: Vec<Expression> = args.into_iter().map(|e| e.expr).collect();
        if ctx.match_token(TokenKind::Over) {
            ctx.expect_token(TokenKind::LParen)?;
            let mut partition_by = Vec::new();
            let mut order_by = Vec::new();
            let mut order_desc = Vec::new();

            if match_identifier_token(ctx, "PARTITION") {
                ctx.expect_token(TokenKind::By)?;
                partition_by.push(parse_expression(ctx)?.expr);
                while ctx.match_token(TokenKind::Comma) {
                    partition_by.push(parse_expression(ctx)?.expr);
                }
            }

            if ctx.match_token(TokenKind::Order) {
                ctx.expect_token(TokenKind::By)?;
                let first_expr = parse_expression(ctx)?;
                let desc = if ctx.match_token(TokenKind::Desc) {
                    true
                } else {
                    ctx.match_token(TokenKind::Asc);
                    false
                };
                order_by.push(first_expr.expr);
                order_desc.push(desc);
                while ctx.match_token(TokenKind::Comma) {
                    let expr = parse_expression(ctx)?.expr;
                    let d = if ctx.match_token(TokenKind::Desc) {
                        true
                    } else {
                        ctx.match_token(TokenKind::Asc);
                        false
                    };
                    order_by.push(expr);
                    order_desc.push(d);
                }
            }

            ctx.expect_token(TokenKind::RParen)?;
            let span = ctx.merge_span(span.start, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::WindowFunction {
                    name,
                    args: func_args,
                    over_partition_by: partition_by,
                    over_order_by: order_by,
                    over_order_desc: order_desc,
                },
                span,
            })
        } else {
            Ok(ParseResult {
                expr: Expression::Function {
                    name,
                    args: func_args,
                },
                span,
            })
        }
    }
}

fn parse_expression_list(ctx: &mut ParseContext<'_>) -> Result<Vec<ParseResult>, ParseError> {
    let mut expressions = Vec::new();
    expressions.push(parse_expression(ctx)?);
    while ctx.match_token(TokenKind::Comma) {
        expressions.push(parse_expression(ctx)?);
    }
    Ok(expressions)
}

fn parse_property_list(
    ctx: &mut ParseContext<'_>,
) -> Result<Vec<(String, ParseResult)>, ParseError> {
    let mut properties = Vec::new();
    while !ctx.match_token(TokenKind::RBrace) {
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

fn parse_case_expression(
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

fn parse_list_comprehension(
    start_pos: Position,
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    let variable = ctx.expect_identifier()?;
    ctx.expect_token(TokenKind::In)?;
    let source = parse_expression(ctx)?.expr;

    let (filter, map) = if ctx.match_token(TokenKind::Pipe) {
        let map_expr = parse_expression(ctx)?;
        (None, Some(map_expr.expr))
    } else if ctx.match_token(TokenKind::Where) {
        let filter_expr = parse_expression(ctx)?;
        let map_expr = if ctx.match_token(TokenKind::Pipe) {
            Some(parse_expression(ctx)?.expr)
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

fn parse_subquery_body(ctx: &mut ParseContext<'_>) -> Result<SubqueryBody, ParseError> {
    let mut patterns = Vec::new();
    let mut where_clause = None;
    let mut return_expr = None;

    if ctx.match_token(TokenKind::Match) {
        let pattern_str = parse_pattern_string(ctx)?;
        patterns.push(pattern_str);
    } else if !matches!(
        ctx.current_token().kind,
        TokenKind::Where | TokenKind::Return | TokenKind::RBrace
    ) {
        // Bare pattern without the MATCH keyword:
        // `EXISTS { a:person-[:knows]->b:person }`.
        let pattern_str = parse_pattern_string(ctx)?;
        patterns.push(pattern_str);
    }

    if ctx.match_token(TokenKind::Where) {
        let expr = parse_expression(ctx)?;
        where_clause = Some(Box::new(expr.expr));
    }

    if ctx.match_token(TokenKind::Return) {
        let expr = parse_expression(ctx)?;
        return_expr = Some(Box::new(expr.expr));
    }

    Ok(SubqueryBody {
        id: 0,
        patterns,
        where_clause,
        return_expr,
    })
}

fn parse_pattern_string(ctx: &mut ParseContext<'_>) -> Result<String, ParseError> {
    let start_pos = ctx.current_position();
    let mut pattern = String::new();

    // Collect pattern tokens until a subquery terminator (RBrace / WHERE /
    // RETURN / MATCH). The terminator is inspected WITHOUT consuming it so
    // `parse_subquery_body` can dispatch on it afterwards.
    let mut tokens = Vec::new();
    loop {
        let terminator = matches!(
            ctx.current_token().kind,
            TokenKind::RBrace | TokenKind::Where | TokenKind::Return | TokenKind::Match
        );
        if terminator {
            break;
        }
        tokens.push(ctx.current_token().clone());
        ctx.next_token();
    }

    if tokens.is_empty() {
        return Err(ParseError::new(
            ParseErrorKind::SyntaxError,
            "Empty pattern in subquery".to_string(),
            start_pos,
        ));
    }

    // Canonicalize the pattern string so it can be re-parsed by the
    // traversal parser later (planning time).
    //
    // Parenthesized patterns (e.g. `MATCH (q:person)`) pass through
    // unchanged. Bare patterns (`a:person-[:knows]->b:person`) have their
    // node segments wrapped in parentheses to yield the standard form
    // `(a:person)-[:knows]->(b:person)`.
    if matches!(tokens[0].kind, TokenKind::LParen) {
        for tok in &tokens {
            pattern.push_str(&tok.lexeme);
            pattern.push(' ');
        }
    } else {
        let mut in_node = false;
        let mut in_brackets = false;
        for tok in &tokens {
            match &tok.kind {
                TokenKind::Identifier(_) => {
                    if !in_node && !in_brackets {
                        pattern.push_str("( ");
                        in_node = true;
                    }
                    pattern.push_str(&tok.lexeme);
                    pattern.push(' ');
                }
                TokenKind::LBracket => {
                    in_brackets = true;
                    pattern.push('[');
                    pattern.push(' ');
                }
                TokenKind::RBracket => {
                    in_brackets = false;
                    pattern.push(']');
                    pattern.push(' ');
                }
                TokenKind::Minus
                | TokenKind::Arrow
                | TokenKind::BackArrow
                | TokenKind::LeftArrow
                | TokenKind::RightArrow => {
                    if in_node {
                        pattern.push_str(") ");
                        in_node = false;
                    }
                    pattern.push_str(&tok.lexeme);
                    pattern.push(' ');
                }
                _ => {
                    pattern.push_str(&tok.lexeme);
                    pattern.push(' ');
                }
            }
        }
        if in_node {
            pattern.push(')');
        }
    }

    Ok(pattern.trim().to_string())
}

fn match_identifier_token(ctx: &mut ParseContext<'_>, expected: &str) -> bool {
    if let TokenKind::Identifier(s) = &ctx.current_token().kind {
        if s.eq_ignore_ascii_case(expected) {
            ctx.next_token();
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_expression() {
        let input = "1 + 2 * 3";
        let ctx = &mut ParseContext::new(input);
        let result = parse_expression(ctx);
        assert!(result.is_ok());
        let parse_result = result.expect("Simple expression parsing should succeed");
        assert!(matches!(parse_result.expr, Expression::Binary { .. }));
    }

    #[test]
    fn test_parse_json_path_get() {
        // Regression: peek_token used to return the current token, so the
        // `expr -> 'key'` JSON access postfix never fired.
        for input in ["m->'key'", "m->>\"key\"", "m#>'a.b'", "m#>>\"a.b\""] {
            let ctx = &mut ParseContext::new(input);
            let result = parse_expression(ctx);
            assert!(
                result.is_ok(),
                "parse failed for {input}: {:?}",
                result.err()
            );
            let parse_result = result.expect("JSON access parsing should succeed");
            assert!(matches!(parse_result.expr, Expression::Binary { .. }));
        }
    }

    #[test]
    fn test_parse_in_subquery() {
        // Regression: peek_token used to return the current token, so
        // `x IN { subquery }` was never recognized and the tail was dropped.
        let input = "t.name IN { MATCH (p:person) RETURN p.name }";
        let ctx = &mut ParseContext::new(input);
        let result = parse_expression(ctx);
        assert!(result.is_ok(), "parse failed: {:?}", result.err());
        let parse_result = result.expect("IN subquery parsing should succeed");
        match parse_result.expr {
            Expression::In {
                expr,
                subquery,
                negated,
            } => {
                assert!(!negated);
                assert!(matches!(*expr, Expression::Property { .. }));
                assert_eq!(subquery.patterns.len(), 1);
                assert!(subquery.return_expr.is_some());
            }
            other => panic!("expected Expression::In, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_not_in_subquery() {
        // Regression: `NOT IN` lexes as a single NotIn token that the parser
        // used to drop, silently discarding the whole subquery.
        let input = "t.name NOT IN { MATCH (p:person) RETURN p.name }";
        let ctx = &mut ParseContext::new(input);
        let result = parse_expression(ctx);
        assert!(result.is_ok(), "parse failed: {:?}", result.err());
        let parse_result = result.expect("NOT IN subquery parsing should succeed");
        match parse_result.expr {
            Expression::In {
                expr,
                subquery,
                negated,
            } => {
                assert!(negated);
                assert!(matches!(*expr, Expression::Property { .. }));
                assert_eq!(subquery.patterns.len(), 1);
                assert!(subquery.return_expr.is_some());
            }
            other => panic!("expected negated Expression::In, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_parenthesized_expression() {
        let input = "(1 + 2) * 3";
        let ctx = &mut ParseContext::new(input);
        let result = parse_expression(ctx);
        assert!(result.is_ok());
        let parse_result = result.expect("Parsing a bracketed expression should succeed");
        assert!(matches!(parse_result.expr, Expression::Binary { .. }));
    }

    #[test]
    fn test_parse_exists_with_where_clause() {
        // Regression: parse_pattern_string used to consume the WHERE / RETURN
        // terminators, making `EXISTS { MATCH ... WHERE ... }` fail with
        // "Expected RBrace, found Identifier(...)".
        let input = "EXISTS { MATCH (q:person) WHERE q.age > 30 }";
        let ctx = &mut ParseContext::new(input);
        let result = parse_expression(ctx);
        assert!(result.is_ok(), "parse failed: {:?}", result.err());
        let parse_result = result.expect("EXISTS parsing should succeed");
        match parse_result.expr {
            Expression::Exists { body } => {
                assert_eq!(body.patterns.len(), 1, "one pattern expected");
                assert_eq!(body.patterns[0], "( q : person )");
                assert!(
                    body.where_clause.is_some(),
                    "WHERE clause must be preserved"
                );
            }
            other => panic!("expected Expression::Exists, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_exists_with_return_expr() {
        let input = "EXISTS { MATCH (q:person) WHERE q.age > 30 RETURN q.name }";
        let ctx = &mut ParseContext::new(input);
        let result = parse_expression(ctx);
        assert!(result.is_ok(), "parse failed: {:?}", result.err());
        let parse_result = result.expect("EXISTS parsing should succeed");
        match parse_result.expr {
            Expression::Exists { body } => {
                assert_eq!(body.patterns.len(), 1);
                assert!(body.where_clause.is_some());
                assert!(body.return_expr.is_some(), "RETURN expr must be preserved");
            }
            other => panic!("expected Expression::Exists, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_exists_bare_pattern() {
        let input = "EXISTS { a:person-[:knows]->b:person }";
        let ctx = &mut ParseContext::new(input);
        let result = parse_expression(ctx);
        assert!(result.is_ok(), "parse failed: {:?}", result.err());
        let parse_result = result.expect("EXISTS parsing should succeed");
        match parse_result.expr {
            Expression::Exists { body } => {
                assert_eq!(body.patterns.len(), 1);
                assert_eq!(
                    body.patterns[0],
                    "( a : person ) - [ : knows ] -> ( b : person )"
                );
                assert!(body.where_clause.is_none());
            }
            other => panic!("expected Expression::Exists, got {:?}", other),
        }
    }

    #[test]
    fn test_subquery_pattern_round_trip_reparse() {
        // The stored pattern strings must be re-parseable by the traversal
        // parser, which the exists planner relies on to build the subquery
        // plan. Both the parenthesized (MATCH) and the bare forms are
        // canonicalized at parse time.
        for input in [
            "EXISTS { MATCH (q:person) }",
            "EXISTS { a:person-[:knows]->b:person }",
            "EXISTS { MATCH (a:person)-[:knows]->(b:person) WHERE b.age > 18 }",
        ] {
            let ctx = &mut ParseContext::new(input);
            let result = parse_expression(ctx);
            assert!(
                result.is_ok(),
                "parse failed for {input}: {:?}",
                result.err()
            );
            let parse_result = result.expect("EXISTS parsing should succeed");
            let body = match parse_result.expr {
                Expression::Exists { body } => body,
                other => panic!("expected Expression::Exists, got {:?}", other),
            };
            for pattern_str in &body.patterns {
                let mut ctx = &mut ParseContext::new(pattern_str);
                let mut parser =
                    crate::query::parser::parsing::traversal_parser::TraversalParser::new();
                let pattern = parser.parse_pattern(&mut ctx);
                assert!(
                    pattern.is_ok(),
                    "stored pattern `{pattern_str}` must be re-parseable: {:?}",
                    pattern.err()
                );
            }
        }
    }
}
