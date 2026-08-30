//! Expression parsing module
//!
//! Provides functions to parse expressions from token streams into
//! the core Expression representation.

use std::sync::Arc;

use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::{ContextualExpression, Expression, ExpressionMeta, SubqueryBody};
use graphdb_core::types::operators::{BinaryOperator, UnaryOperator};
use graphdb_core::types::{DataType, Position, Span};
use graphdb_core::{StructValue, Value};

mod function;
mod subquery;
#[cfg(test)]
mod tests;

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
            // `p.addr.city` / `STRUCT{...}.city`: a dot on a base that is
            // already a resolved access (Property/StructField/Subscript) is a
            // STRUCT field access; a dot on a bare variable stays a vertex
            // property access.
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
                let target_type = type_name.parse::<DataType>().map_err(|e| {
                    ParseError::new(
                        ParseErrorKind::SyntaxError,
                        format!("Unknown type cast target: {}", e),
                        span.start,
                    )
                })?;
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
                expr: Expression::literal(Value::Null(graphdb_core::NullType::Null)),
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
        TokenKind::Struct => {
            ctx.next_token();
            ctx.expect_token(TokenKind::LBrace)?;
            let fields = parse_property_list(ctx)?;
            ctx.expect_token(TokenKind::RBrace)?;
            let span = ctx.merge_span(start_pos, ctx.current_position());
            // STRUCT literals must be constants: evaluate each field value at
            // parse time (mirrors the DEFAULT expression path).
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
            // ARRAY literals must be constants: evaluate each element at
            // parse time.
            let mut values = Vec::with_capacity(elements.len());
            for result in elements {
                let value = eval_literal_expression(&result.expr, span.start)?;
                values.push(value);
            }
            Ok(ParseResult {
                expr: Expression::literal(Value::Array(Box::new(graphdb_core::ArrayValue::new(
                    values,
                )))),
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

pub(crate) fn parse_function_call(
    name: String,
    span: Span,
    ctx: &mut ParseContext<'_>,
) -> Result<ParseResult, ParseError> {
    function::parse_function_call(name, span, ctx)
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

/// Evaluate an expression to a constant value at parse time.
///
/// Used by STRUCT/ARRAY literal folding; variable references are rejected
/// because composite literals carry concrete values, not expressions.
fn eval_literal_expression(expr: &Expression, position: Position) -> Result<Value, ParseError> {
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

/// Parse a cast target type name after `::` — accepts an identifier or the
/// type-name keywords (INT, STRING, LIST, MAP, STRUCT, ARRAY, ...).
fn expect_cast_type_name(ctx: &mut ParseContext<'_>) -> Result<String, ParseError> {
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

pub(crate) fn parse_sql_subquery_body(
    ctx: &mut ParseContext<'_>,
) -> Result<SubqueryBody, ParseError> {
    subquery::parse_sql_subquery_body(ctx)
}

pub(crate) fn parse_subquery_body(ctx: &mut ParseContext<'_>) -> Result<SubqueryBody, ParseError> {
    subquery::parse_subquery_body(ctx)
}
