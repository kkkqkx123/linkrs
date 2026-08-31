use super::subquery::match_identifier_token;
use super::{parse_expression, parse_expression_list, ParseResult};
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::Span;


pub(crate) fn parse_function_call(
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
                    func: graphdb_core::types::operators::AggregateFunction::Count,
                    args: vec![Expression::Literal(graphdb_core::Value::string("*"))],
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
        let mut agg_args: Vec<Expression> = args.iter().map(|a| a.expr.clone()).collect();
        if agg_args.is_empty() {
            agg_args.push(Expression::Literal(graphdb_core::Value::Null(
                graphdb_core::NullType::Null,
            )));
        }

        // Parameterized aggregates carry their extra parameters (e.g. the
        // percentile fraction of PERCENTILE_CONT) in `args` after the field.
        let func = match name_upper.as_str() {
            "COUNT" => graphdb_core::types::operators::AggregateFunction::Count,
            "SUM" => graphdb_core::types::operators::AggregateFunction::Sum,
            "AVG" => graphdb_core::types::operators::AggregateFunction::Avg,
            "MIN" => graphdb_core::types::operators::AggregateFunction::Min,
            "MAX" => graphdb_core::types::operators::AggregateFunction::Max,
            "COLLECT" => graphdb_core::types::operators::AggregateFunction::Collect,
            "COLLECT_SET" => graphdb_core::types::operators::AggregateFunction::CollectSet,
            "STD" => graphdb_core::types::operators::AggregateFunction::Std,
            "STDDEV_POP" => graphdb_core::types::operators::AggregateFunction::StddevPop,
            "STDDEV_SAMP" => graphdb_core::types::operators::AggregateFunction::StddevSamp,
            "PRODUCT" => graphdb_core::types::operators::AggregateFunction::Product,
            "PERCENTILE_CONT" | "PERCENTILE" => {
                if args.len() < 2 {
                    agg_args.push(Expression::Literal(graphdb_core::Value::Double(50.0)));
                }
                graphdb_core::types::operators::AggregateFunction::PercentileCont
            }
            "VARIANCE" => graphdb_core::types::operators::AggregateFunction::Variance,
            "MEDIAN" => graphdb_core::types::operators::AggregateFunction::Median,
            "MODE" => graphdb_core::types::operators::AggregateFunction::Mode,
            "BOOL_AND" => graphdb_core::types::operators::AggregateFunction::BoolAnd,
            "BOOL_OR" => graphdb_core::types::operators::AggregateFunction::BoolOr,
            _ => graphdb_core::types::operators::AggregateFunction::Count,
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
                args: agg_args,
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
