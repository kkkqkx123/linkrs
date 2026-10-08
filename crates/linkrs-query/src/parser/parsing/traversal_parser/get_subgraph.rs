use crate::parser::ast::stmt::*;
use crate::parser::core::error::ParseError;
use crate::parser::parsing::clause_parser::ClauseParser;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

use super::TraversalParser;

impl TraversalParser {
    /// Analysis of the GET SUBGRAPH statement
    pub fn parse_subgraph_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Get)?;

        ctx.expect_token(TokenKind::Subgraph)?;

        let with_prop = ctx.match_token(TokenKind::With) && ctx.match_token(TokenKind::Prop);

        let _with_edge = if !with_prop {
            ctx.match_token(TokenKind::With) && ctx.match_token(TokenKind::Edge)
        } else {
            false
        };

        let steps = if matches!(ctx.current_token().kind, TokenKind::IntegerLiteral(_)) {
            let n = ctx.expect_integer_literal()?;
            ctx.expect_token(TokenKind::Step)?;
            Steps::Fixed(n as usize)
        } else if ctx.match_token(TokenKind::Step) {
            self.parse_steps(ctx)?
        } else {
            Steps::Fixed(1)
        };

        ctx.expect_token(TokenKind::From)?;
        let from_span = ctx.current_span();
        let from_clause = ctx.recover_clause(
            |c| {
                Ok(FromClause {
                    span: c.current_span(),
                    vertices: Vec::new(),
                })
            },
            |c| {
                let vertices = self.parse_expression_list(c)?;
                Ok(FromClause {
                    span: from_span,
                    vertices,
                })
            },
        )?;

        let over = if ctx.match_token(TokenKind::Over) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| ClauseParser::new().parse_over_clause(c).map(Some),
            )?
        } else {
            None
        };

        let where_clause = if ctx.match_token(TokenKind::Where) {
            Some(ctx.recover_clause(Self::create_true_expression, |c| self.parse_expression(c))?)
        } else {
            Some(Self::create_true_expression(ctx)?)
        };

        let yield_clause = if ctx.match_token(TokenKind::Yield) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| ClauseParser::new().parse_yield_clause(c).map(Some),
            )?
        } else {
            None
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Subgraph(SubgraphStmt {
            span,
            steps,
            from: from_clause,
            over,
            where_clause,
            yield_clause,
        }))
    }
}
