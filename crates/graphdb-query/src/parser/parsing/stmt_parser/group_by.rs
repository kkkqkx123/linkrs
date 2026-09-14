//! GROUP BY statement parsing, including ROLLUP / CUBE / GROUPING SETS.

use crate::parser::ast::stmt::{GroupByStmt, GroupingType, Stmt, YieldClause, YieldItem};
use crate::parser::core::error::ParseError;
use crate::parser::parsing::clause_parser::ClauseParser;
use crate::parser::parsing::expr_parser::parse_expression_with_context;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::Expression;

/// Analysis of the GROUP BY statement
pub(super) fn parse_group_by_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    ctx.expect_token(TokenKind::Group)?;
    ctx.expect_token(TokenKind::By)?;

    let (group_items, grouping_type) = if ctx.match_token(TokenKind::Rollup) {
        ctx.expect_token(TokenKind::LParen)?;
        let items = parse_grouping_set_items(ctx)?;
        ctx.expect_token(TokenKind::RParen)?;
        (items.clone(), GroupingType::Rollup(items))
    } else if ctx.match_token(TokenKind::Cube) {
        ctx.expect_token(TokenKind::LParen)?;
        let items = parse_grouping_set_items(ctx)?;
        ctx.expect_token(TokenKind::RParen)?;
        (items.clone(), GroupingType::Cube(items))
    } else if ctx.match_token(TokenKind::Grouping) {
        ctx.expect_token(TokenKind::Sets)?;
        ctx.expect_token(TokenKind::LParen)?;
        let mut sets = Vec::new();
        loop {
            ctx.expect_token(TokenKind::LParen)?;
            let items = parse_grouping_set_items(ctx)?;
            ctx.expect_token(TokenKind::RParen)?;
            sets.push(items);
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }
        ctx.expect_token(TokenKind::RParen)?;
        let all_items: Vec<_> = sets.iter().flatten().cloned().collect();
        (all_items, GroupingType::GroupingSets(sets))
    } else {
        let mut group_items = Vec::new();
        loop {
            let ident = ctx.expect_identifier()?;
            let expr = Expression::Variable(ident);
            let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
            let expr_id = ctx.expression_context().register_expression(expr_meta);
            let contextual_expr =
                ContextualExpression::new(expr_id, ctx.expression_context_clone());
            group_items.push(contextual_expr);
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }
        (group_items, GroupingType::Standard)
    };

    let yield_clause = if ctx.match_token(TokenKind::Yield) {
        ClauseParser::new().parse_yield_clause(ctx)?
    } else {
        let items: Vec<YieldItem> = group_items
            .iter()
            .enumerate()
            .map(|(i, expr)| YieldItem {
                expression: expr.clone(),
                alias: Some(format!("group_{}", i)),
            })
            .collect();
        YieldClause {
            span: start_span,
            distinct: false,
            items,
            where_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
        }
    };

    let having_clause = if ctx.match_token(TokenKind::Having) {
        ctx.recover_clause(
            |_| Ok(None),
            |c| parse_expression_with_context(c, c.expression_context_clone()).map(Some),
        )?
    } else {
        None
    };

    let end_span = ctx.current_span();
    let span = ctx.merge_span(start_span.start, end_span.end);

    Ok(Stmt::GroupBy(GroupByStmt {
        span,
        group_items,
        grouping_type,
        yield_clause,
        having_clause,
    }))
}

/// Parse grouping set items for ROLLUP, CUBE, GROUPING SETS
fn parse_grouping_set_items(
    ctx: &mut ParseContext,
) -> Result<Vec<ContextualExpression>, ParseError> {
    let mut items = Vec::new();
    loop {
        let ident = ctx.expect_identifier()?;
        let expr = Expression::Variable(ident);
        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
        let expr_id = ctx.expression_context().register_expression(expr_meta);
        let contextual_expr = ContextualExpression::new(expr_id, ctx.expression_context_clone());
        items.push(contextual_expr);
        if !ctx.match_token(TokenKind::Comma) {
            break;
        }
    }
    Ok(items)
}
