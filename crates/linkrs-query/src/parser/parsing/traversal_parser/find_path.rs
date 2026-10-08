use crate::parser::ast::stmt::*;
use crate::parser::core::error::ParseError;
use crate::parser::parsing::clause_parser::ClauseParser;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;
use linkrs_core::types::graph_schema::EdgeDirection;

use super::TraversalParser;

impl TraversalParser {
    /// Analysis of the FIND PATH statement
    pub fn parse_find_path_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Find)?;

        let shortest = if ctx.match_token(TokenKind::Shortest) {
            true
        } else {
            !ctx.match_token(TokenKind::All)
        };

        ctx.expect_token(TokenKind::Path)?;

        let mut with_loop = false;
        let mut with_cycle = false;
        while ctx.match_token(TokenKind::With) {
            if ctx.match_token(TokenKind::Loop) {
                with_loop = true;
            } else if ctx.match_token(TokenKind::Cycle) {
                with_cycle = true;
            }
        }

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

        let to_present = ctx.recover_clause(
            |_| Ok(false),
            |c| {
                c.expect_token(TokenKind::To)?;
                Ok(true)
            },
        )?;
        let to_vertex = if to_present {
            ctx.recover_clause(Self::create_true_expression, |c| self.parse_expression(c))?
        } else {
            Self::create_true_expression(ctx)?
        };

        let over_present = ctx.recover_clause(
            |_| Ok(false),
            |c| {
                c.expect_token(TokenKind::Over)?;
                Ok(true)
            },
        )?;
        let over = if over_present {
            ctx.recover_clause(
                |c| {
                    Ok(OverClause {
                        span: c.current_span(),
                        edge_types: Vec::new(),
                        direction: EdgeDirection::Out,
                    })
                },
                |c| ClauseParser::new().parse_over_clause(c),
            )?
        } else {
            OverClause {
                span: ctx.current_span(),
                edge_types: Vec::new(),
                direction: EdgeDirection::Out,
            }
        };

        let mut max_steps = None;
        if ctx.match_token(TokenKind::Upto) {
            let steps = ctx.recover_clause(
                |_| Ok(None),
                |c| c.expect_integer_literal().map(|n| n as usize).map(Some),
            )?;
            if let Some(n) = steps {
                max_steps = Some(n);
                ctx.expect_token(TokenKind::Step)?;
            }
        }

        let weight_expression = if ctx.match_token(TokenKind::Weight) {
            ctx.recover_clause(|_| Ok(None), |c| c.expect_identifier().map(Some))?
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

        Ok(Stmt::FindPath(FindPathStmt {
            span,
            from: from_clause,
            to: to_vertex,
            over: Some(over),
            where_clause,
            shortest,
            max_steps,
            limit: None,
            skip: None,
            yield_clause,
            weight_expression,
            heuristic_expression: None,
            with_loop,
            with_cycle,
        }))
    }
}
