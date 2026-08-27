//! Sentence Parsing Module
//!
//! Responsible for parsing various shared clauses, including RETURN, YIELD, SET, OVER, WHERE, etc.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression as CoreExpression;
use crate::core::types::graph_schema::EdgeDirection;
use crate::parser::ast::stmt::*;
use crate::parser::ast::types::{LimitClause, OrderDirection, SkipClause};
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::expr_parser::parse_expression_with_context;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

/// Sentence parser
pub struct ClauseParser;

impl ClauseParser {
    pub fn new() -> Self {
        Self
    }

    /// Analysis of the RETURN clause
    pub fn parse_return_clause(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ReturnClause, ParseError> {
        let span = ctx.current_span();

        let distinct = ctx.match_token(TokenKind::Distinct);

        let mut items = Vec::new();

        // Check whether it is *
        if ctx.match_token(TokenKind::Star) {
            let expr = CoreExpression::variable("*");
            let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
            let id = ctx.expression_context().register_expression(expr_meta);
            let ctx_expr = ContextualExpression::new(id, ctx.expression_context_clone());
            items.push(ReturnItem::Expression {
                expression: ctx_expr,
                alias: None,
            });
        } else {
            loop {
                let Some(item) = ctx.recover_clause(
                    |_| Ok(None),
                    |c| {
                        let expr = self.parse_expression(c)?;
                        let alias = if c.match_token(TokenKind::As) {
                            Some(c.expect_identifier()?)
                        } else {
                            None
                        };
                        Ok(Some(ReturnItem::Expression {
                            expression: expr,
                            alias,
                        }))
                    },
                )?
                else {
                    break;
                };
                items.push(item);
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }

        // Explanation of `ORDER BY`
        let mut order_by = if ctx.match_token(TokenKind::Order) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| {
                    c.expect_token(TokenKind::By)?;
                    self.parse_order_by_clause(c).map(Some)
                },
            )?
        } else {
            None
        };

        // Analysis of SKIP and LIMIT (support both orders: SKIP before LIMIT or LIMIT before SKIP)
        let mut limit: Option<LimitClause> = None;
        let mut skip: Option<SkipClause> = None;

        // First, try to parse SKIP if present
        if ctx.match_token(TokenKind::Skip) {
            skip = ctx.recover_clause(|_| Ok(None), |c| self.parse_skip_clause(c))?;
        }

        // Then, try to parse LIMIT if present
        if ctx.match_token(TokenKind::Limit) {
            limit = ctx.recover_clause(|_| Ok(None), |c| self.parse_limit_clause(c))?;
        }

        // If SKIP wasn't parsed yet, try again (handles LIMIT before SKIP case)
        if skip.is_none() && ctx.match_token(TokenKind::Skip) {
            skip = ctx.recover_clause(|_| Ok(None), |c| self.parse_skip_clause(c))?;
        }

        // Consume GROUP BY items if present (group keys are extracted from non-aggregate return columns)
        // Supports ROLLUP, CUBE, and GROUPING SETS syntax
        if ctx.match_token(TokenKind::Group) {
            ctx.recover_clause(|_| Ok(()), |c| self.parse_return_group_by(c))?;
        }

        // Accept an ORDER BY clause after GROUP BY (`GROUP BY ... ORDER BY ...`).
        if order_by.is_none() && ctx.match_token(TokenKind::Order) {
            order_by = ctx.recover_clause(
                |_| Ok(None),
                |c| {
                    c.expect_token(TokenKind::By)?;
                    self.parse_order_by_clause(c).map(Some)
                },
            )?;
        }

        // Parse optional SAMPLE clause
        let sample = if ctx.match_token(TokenKind::Sample) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| {
                    let count = c.expect_integer_literal()? as usize;
                    Ok(Some(crate::parser::ast::types::SampleClause {
                        span: c.current_span(),
                        count,
                        percentage: None,
                    }))
                },
            )?
        } else {
            None
        };

        // Parse optional HAVING clause
        let having_clause = if ctx.match_token(TokenKind::Having) {
            ctx.recover_clause(|_| Ok(None), |c| self.parse_expression(c).map(Some))?
        } else {
            None
        };

        Ok(ReturnClause {
            span,
            items,
            distinct,
            order_by,
            limit,
            skip,
            sample,
            having_clause,
        })
    }

    /// Analyzing the YIELD clause
    ///
    /// Assuming that the YIELD token has been consumed by the caller, this method will only parse the subsequent list of expressions, as well as subqueries such as WHERE, LIMIT, and SKIP.
    pub fn parse_yield_clause(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<YieldClause, ParseError> {
        let start_span = ctx.current_span();

        let mut items = Vec::new();

        // Check whether it is *.
        if ctx.match_token(TokenKind::Star) {
            // “YIELD *” indicates that all columns should be returned.
        } else {
            loop {
                let Some(item) = ctx.recover_clause(
                    |_| Ok(None),
                    |c| {
                        let expr = self.parse_expression(c)?;
                        let alias = if c.match_token(TokenKind::As) {
                            Some(c.expect_identifier()?)
                        } else {
                            None
                        };
                        Ok(Some(YieldItem {
                            expression: expr,
                            alias,
                        }))
                    },
                )?
                else {
                    break;
                };
                items.push(item);
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }

        // Analyzing the WHERE clause
        let where_clause = if ctx.match_token(TokenKind::Where) {
            ctx.recover_clause(|_| Ok(None), |c| self.parse_expression(c).map(Some))?
        } else {
            None
        };

        // Explanation of `ORDER BY`
        let order_by = if ctx.match_token(TokenKind::Order) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| {
                    c.expect_token(TokenKind::By)?;
                    self.parse_order_by_clause(c).map(Some)
                },
            )?
        } else {
            None
        };

        // Analysis of the LIMIT clause
        let limit = if ctx.match_token(TokenKind::Limit) {
            ctx.recover_clause(|_| Ok(None), |c| self.parse_limit_clause(c))?
        } else {
            None
        };

        // Analysis of SKIP
        let skip = if ctx.match_token(TokenKind::Skip) {
            ctx.recover_clause(|_| Ok(None), |c| self.parse_skip_clause(c))?
        } else {
            None
        };

        // Parse optional SAMPLE clause
        let sample = if ctx.match_token(TokenKind::Sample) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| {
                    let count = c.expect_integer_literal()? as usize;
                    Ok(Some(crate::parser::ast::types::SampleClause {
                        span: c.current_span(),
                        count,
                        percentage: None,
                    }))
                },
            )?
        } else {
            None
        };

        let end_span = ctx.current_span();

        Ok(YieldClause {
            span: ctx.merge_span(start_span.start, end_span.end),
            items,
            where_clause,
            order_by,
            limit,
            skip,
            sample,
        })
    }

    /// Analyzing the SET clause
    pub fn parse_set_clause(&mut self, ctx: &mut ParseContext) -> Result<SetClause, ParseError> {
        let span = ctx.current_span();
        let assignments = self.parse_set_assignments(ctx)?;
        Ok(SetClause { span, assignments })
    }

    /// Analyzing the SET assignment list
    pub fn parse_set_assignments(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Vec<Assignment>, ParseError> {
        let mut assignments = Vec::new();
        loop {
            let Some(assignment) =
                ctx.recover_clause(|_| Ok(None), |c| self.parse_assignment(c).map(Some))?
            else {
                break;
            };
            assignments.push(assignment);
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }
        Ok(assignments)
    }

    /// Parse a single SET assignment: `<property path> = <expression>`.
    fn parse_assignment(&mut self, ctx: &mut ParseContext) -> Result<Assignment, ParseError> {
        // Parse the LHS as a property path (not a full expression) so the
        // assignment `=` is not consumed as an equality comparison.
        let property_expr =
            crate::parser::parsing::expr_parser::parse_property_path_with_context(
                ctx,
                ctx.expression_context_clone(),
            )?;
        ctx.expect_token(TokenKind::Assign)?;
        let value = self.parse_expression(ctx)?;

        let (property, target) = match property_expr.expression() {
            Some(expr) => match expr.inner() {
                CoreExpression::Property { object, property } => {
                    // Check if object is a literal (e.g., 1.age) or a variable (e.g., p.age)
                    let target = match object.as_ref() {
                        CoreExpression::Literal(_) => Some(property_expr.clone()),
                        CoreExpression::Variable(_) => None, // Variable-based property access
                        _ => Some(property_expr.clone()),
                    };
                    (property.clone(), target)
                }
                CoreExpression::Variable(name) => (name.clone(), None),
                _ => {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        "SET assignment requires a property path (e.g., p.age)".to_string(),
                        ctx.current_position(),
                    ));
                }
            },
            None => {
                return Err(ParseError::new(
                    ParseErrorKind::SyntaxError,
                    "Expression not registered in context".to_string(),
                    ctx.current_position(),
                ));
            }
        };

        Ok(Assignment {
            property,
            value,
            target,
            object: None,
        })
    }

    /// Analysis of the OVER clause
    pub fn parse_over_clause(&mut self, ctx: &mut ParseContext) -> Result<OverClause, ParseError> {
        let span = ctx.current_span();

        ctx.recover_clause(
            |c| {
                Ok(OverClause {
                    span: c.current_span(),
                    edge_types: Vec::new(),
                    direction: EdgeDirection::Out,
                })
            },
            |c| {
                let edge_types = self.parse_edge_types(c)?;

                // Analysis direction (optional)
                let direction =
                    if c.match_token(TokenKind::In) || c.match_token(TokenKind::Reversely) {
                        EdgeDirection::In
                    } else if c.match_token(TokenKind::Bidirect) {
                        EdgeDirection::Both
                    } else {
                        EdgeDirection::Out
                    };

                Ok(OverClause {
                    span,
                    edge_types,
                    direction,
                })
            },
        )
    }

    /// Analyzing the list of edge types
    fn parse_edge_types(&mut self, ctx: &mut ParseContext) -> Result<Vec<String>, ParseError> {
        let mut types = Vec::new();
        types.push(ctx.expect_identifier()?);
        while ctx.match_token(TokenKind::Comma) {
            types.push(ctx.expect_identifier()?);
        }
        Ok(types)
    }

    /// Parse a SKIP clause body (the SKIP token has already been consumed).
    fn parse_skip_clause(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Option<SkipClause>, ParseError> {
        let count = ctx.expect_integer_literal()? as usize;
        Ok(Some(SkipClause {
            span: ctx.current_span(),
            count,
        }))
    }

    /// Parse a LIMIT clause body (the LIMIT token has already been consumed).
    fn parse_limit_clause(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Option<LimitClause>, ParseError> {
        let count = ctx.expect_integer_literal()? as usize;
        Ok(Some(LimitClause {
            span: ctx.current_span(),
            count,
        }))
    }

    /// Parse the GROUP BY body of a RETURN clause (GROUP has been consumed).
    /// Supports ROLLUP, CUBE, and GROUPING SETS syntax.
    fn parse_return_group_by(&mut self, ctx: &mut ParseContext) -> Result<(), ParseError> {
        ctx.expect_token(TokenKind::By)?;

        if ctx.match_token(TokenKind::Rollup) {
            // GROUP BY ROLLUP(...)
            ctx.expect_token(TokenKind::LParen)?;
            loop {
                self.parse_expression(ctx)?;
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
        } else if ctx.match_token(TokenKind::Cube) {
            // GROUP BY CUBE(...)
            ctx.expect_token(TokenKind::LParen)?;
            loop {
                self.parse_expression(ctx)?;
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
        } else if ctx.match_token(TokenKind::Grouping) {
            // GROUP BY GROUPING SETS((...), (...))
            ctx.expect_token(TokenKind::Sets)?;
            ctx.expect_token(TokenKind::LParen)?;
            loop {
                ctx.expect_token(TokenKind::LParen)?;
                loop {
                    self.parse_expression(ctx)?;
                    if !ctx.match_token(TokenKind::Comma) {
                        break;
                    }
                }
                ctx.expect_token(TokenKind::RParen)?;
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
        } else {
            // Standard GROUP BY expr, expr, ...
            loop {
                self.parse_expression(ctx)?;
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Analyzing the ORDER BY clause
    fn parse_order_by_clause(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<OrderByClause, ParseError> {
        let span = ctx.current_span();
        let mut items = Vec::new();

        loop {
            let expr = self.parse_expression(ctx)?;
            let direction = if ctx.match_token(TokenKind::Asc) {
                OrderDirection::Asc
            } else if ctx.match_token(TokenKind::Desc) {
                OrderDirection::Desc
            } else {
                OrderDirection::Asc
            };
            items.push(OrderByItem {
                expression: expr,
                direction,
            });
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }

        Ok(OrderByClause { span, items })
    }

    /// Analyzing the expression
    fn parse_expression(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ContextualExpression, ParseError> {
        parse_expression_with_context(ctx, ctx.expression_context_clone())
    }
}

impl Default for ClauseParser {
    fn default() -> Self {
        Self::new()
    }
}
