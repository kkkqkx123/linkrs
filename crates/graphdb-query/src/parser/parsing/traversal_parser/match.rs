use crate::parser::ast::hint::JoinHintAst;
use crate::parser::ast::pattern::Pattern;
use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::clause_parser::ClauseParser;
use crate::parser::parsing::dml_parser::DmlParser;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

use super::{ParsedMatchClause, TraversalParser};

impl TraversalParser {
    /// Analyzing the MATCH statement
    pub fn parse_match_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();

        let mut optional = ctx.match_token(TokenKind::Optional);

        ctx.expect_token(TokenKind::Match)?;

        let first = self.parse_match_clause(ctx)?;
        let mut patterns = first.patterns;
        let mut join_hint = first.join_hint;
        let mut where_clause = first.where_clause;
        let mut where_explicit = first.where_explicit;
        let mut return_clause = first.return_clause;
        let mut delete_clause = first.delete_clause;

        while !optional
            && return_clause.is_none()
            && delete_clause.is_none()
            && (ctx.check_token(TokenKind::Match)
                || (ctx.check_token(TokenKind::Optional)
                    && ctx.peek_token().kind == TokenKind::Match))
        {
            if ctx.check_token(TokenKind::Optional) {
                ctx.expect_token(TokenKind::Optional)?;
                optional = true;
            }
            ctx.expect_token(TokenKind::Match)?;
            let next = self.parse_match_clause(ctx)?;
            patterns.extend(next.patterns);

            match (where_explicit, next.where_explicit) {
                (false, true) => where_clause = next.where_clause,
                (true, true) => {
                    if let (Some(left), Some(right)) = (&where_clause, &next.where_clause) {
                        if let Some(combined) = ctx.expression_context().and(left, right) {
                            where_clause = Some(combined);
                        }
                    }
                }
                _ => {}
            }
            where_explicit |= next.where_explicit;

            if let Some(rc) = next.return_clause {
                return_clause = Some(rc);
            }
            if let Some(dc) = next.delete_clause {
                delete_clause = Some(dc);
            }
            if next.join_hint.is_some() {
                join_hint = next.join_hint;
            }
        }

        let (order_by, limit, skip) = if let Some(ref rc) = return_clause {
            (rc.order_by.clone(), rc.limit.clone(), rc.skip.clone())
        } else {
            (None, None, None)
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Match(MatchStmt {
            span,
            patterns,
            join_hint,
            where_clause,
            return_clause,
            order_by,
            limit,
            skip,
            optional,
            delete_clause,
        }))
    }

    pub(crate) fn parse_match_clause(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ParsedMatchClause, ParseError> {
        let mut patterns = Vec::new();
        loop {
            let Some(pattern) = ctx.recover_clause(
                |c| {
                    Ok(Some(Pattern::Variable(VariablePattern {
                        span: c.current_span(),
                        name: String::new(),
                    })))
                },
                |c| self.parse_pattern(c).map(Some),
            )?
            else {
                break;
            };
            patterns.push(pattern);
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }

        let join_hint = if ctx.check_keyword_sequence(&["USING", "JOIN"]) {
            Some(self.parse_join_hint(ctx)?)
        } else {
            None
        };

        let (mut where_clause, mut where_explicit) = if ctx.match_token(TokenKind::Where) {
            (
                Some(
                    ctx.recover_clause(Self::create_true_expression, |c| self.parse_expression(c))?,
                ),
                true,
            )
        } else {
            (Some(Self::create_true_expression(ctx)?), false)
        };

        let return_clause = if ctx.match_token(TokenKind::Return) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| ClauseParser::new().parse_return_clause(c).map(Some),
            )?
        } else {
            None
        };

        let delete_clause = if ctx.match_token(TokenKind::Delete) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| self.parse_match_delete_clause(c).map(Some),
            )?
        } else {
            None
        };

        if ctx.match_token(TokenKind::Where) {
            let post =
                ctx.recover_clause(Self::create_true_expression, |c| self.parse_expression(c))?;
            if let Some(ref pre) = where_clause {
                if let Some(combined) = ctx.expression_context().and(pre, &post) {
                    where_clause = Some(combined);
                } else {
                    where_clause = Some(post);
                }
            } else {
                where_clause = Some(post);
            }
            where_explicit = true;
        }

        if ctx.check_token(TokenKind::Merge) {
            DmlParser::new().parse_merge_statement(ctx)?;
        }

        Ok(ParsedMatchClause {
            patterns,
            join_hint,
            where_clause,
            where_explicit,
            return_clause,
            delete_clause,
        })
    }

    fn parse_join_hint(&mut self, ctx: &mut ParseContext) -> Result<JoinHintAst, ParseError> {
        ctx.consume_keyword("USING")?;
        ctx.consume_keyword("JOIN")?;
        if ctx.check_keyword("BINARY") {
            ctx.consume_keyword("BINARY")?;
            ctx.expect_token(TokenKind::LParen)?;
            let left = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::Comma)?;
            let right = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::RParen)?;
            Ok(JoinHintAst::Binary { left, right })
        } else if ctx.check_keyword("MULTIWAY") {
            ctx.consume_keyword("MULTIWAY")?;
            ctx.expect_token(TokenKind::LParen)?;
            let probe = ctx.expect_identifier()?;
            let mut builds = Vec::new();
            while ctx.match_token(TokenKind::Comma) {
                builds.push(ctx.expect_identifier()?);
            }
            ctx.expect_token(TokenKind::RParen)?;
            if builds.is_empty() {
                let pos = ctx.current_position();
                return Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken,
                    "MULTIWAY needs at least a probe and one build variable".to_string(),
                    pos,
                ));
            }
            Ok(JoinHintAst::Multiway { probe, builds })
        } else {
            let pos = ctx.current_position();
            Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                "Expected BINARY or MULTIWAY after USING JOIN".to_string(),
                pos,
            )
            .with_expected_tokens(vec!["BINARY".to_string(), "MULTIWAY".to_string()]))
        }
    }

    fn parse_match_delete_clause(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<MatchDeleteClause, ParseError> {
        ctx.with_edge_syntax_mode(|ctx| {
            let start_span = ctx.current_span();

            let target = if ctx.match_token(TokenKind::Vertex) {
                let vertex_ids = self.parse_expression_list(ctx)?;
                MatchDeleteTarget::Vertices(vertex_ids)
            } else if ctx.match_token(TokenKind::Edge) {
                let first_expr = self.parse_expression(ctx)?;
                if ctx.check_token(TokenKind::Arrow) {
                    let mut edge_refs = Vec::new();
                    let mut current_src = first_expr;
                    loop {
                        ctx.expect_token(TokenKind::Arrow)?;
                        let dst = self.parse_expression(ctx)?;
                        let rank = if ctx.match_token(TokenKind::At) {
                            Some(self.parse_expression(ctx)?)
                        } else {
                            None
                        };
                        edge_refs.push((current_src, dst, rank));
                        if ctx.match_token(TokenKind::Comma) {
                            current_src = self.parse_expression(ctx)?;
                        } else {
                            break;
                        }
                    }
                    MatchDeleteTarget::EdgeRefs(edge_refs)
                } else {
                    let mut edge_refs = vec![first_expr];
                    while ctx.match_token(TokenKind::Comma) {
                        edge_refs.push(self.parse_expression(ctx)?);
                    }
                    MatchDeleteTarget::Edges(edge_refs)
                }
            } else {
                return Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken,
                    "Expected VERTEX or EDGE after DELETE".to_string(),
                    ctx.current_position(),
                ));
            };

            let with_edge = if ctx.match_token(TokenKind::With) {
                ctx.expect_token(TokenKind::Edge)?;
                true
            } else {
                false
            };

            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            Ok(MatchDeleteClause {
                span,
                target,
                with_edge,
            })
        })
    }
}
