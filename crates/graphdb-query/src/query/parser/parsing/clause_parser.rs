//! Sentence Parsing Module
//!
//! Responsible for parsing various shared clauses, including RETURN, YIELD, SET, OVER, WHERE, etc.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression as CoreExpression;
use crate::core::types::graph_schema::EdgeDirection;
use crate::query::parser::ast::stmt::*;
use crate::query::parser::ast::types::{LimitClause, OrderDirection, SkipClause};
use crate::query::parser::core::error::{ParseError, ParseErrorKind};
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::parsing::ExprParser;
use crate::query::parser::TokenKind;

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
                let expr = self.parse_expression(ctx)?;
                let alias = if ctx.match_token(TokenKind::As) {
                    Some(ctx.expect_identifier()?)
                } else {
                    None
                };
                items.push(ReturnItem::Expression {
                    expression: expr,
                    alias,
                });
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }

        // Explanation of `ORDER BY`
        let order_by = if ctx.match_token(TokenKind::Order) {
            ctx.expect_token(TokenKind::By)?;
            Some(self.parse_order_by_clause(ctx)?)
        } else {
            None
        };

        // Analysis of SKIP and LIMIT (support both orders: SKIP before LIMIT or LIMIT before SKIP)
        let mut limit: Option<LimitClause> = None;
        let mut skip: Option<SkipClause> = None;

        // First, try to parse SKIP if present
        if ctx.match_token(TokenKind::Skip) {
            let count = ctx.expect_integer_literal()? as usize;
            skip = Some(SkipClause {
                span: ctx.current_span(),
                count,
            });
        }

        // Then, try to parse LIMIT if present
        if ctx.match_token(TokenKind::Limit) {
            let count = ctx.expect_integer_literal()? as usize;
            limit = Some(LimitClause {
                span: ctx.current_span(),
                count,
            });
        }

        // If SKIP wasn't parsed yet, try again (handles LIMIT before SKIP case)
        if skip.is_none() && ctx.match_token(TokenKind::Skip) {
            let count = ctx.expect_integer_literal()? as usize;
            skip = Some(SkipClause {
                span: ctx.current_span(),
                count,
            });
        }

        // Consume GROUP BY items if present (group keys are extracted from non-aggregate return columns)
        if ctx.match_token(TokenKind::Group) {
            ctx.expect_token(TokenKind::By)?;
            loop {
                self.parse_expression(ctx)?;
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }

        // Parse optional HAVING clause
        let having_clause = if ctx.match_token(TokenKind::Having) {
            Some(self.parse_expression(ctx)?)
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
            sample: None,
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
                let expr = self.parse_expression(ctx)?;
                let alias = if ctx.match_token(TokenKind::As) {
                    Some(ctx.expect_identifier()?)
                } else {
                    None
                };
                items.push(YieldItem {
                    expression: expr,
                    alias,
                });
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }

        // Analyzing the WHERE clause
        let where_clause = if ctx.match_token(TokenKind::Where) {
            Some(self.parse_expression(ctx)?)
        } else {
            None
        };

        // Explanation of `ORDER BY`
        let order_by = if ctx.match_token(TokenKind::Order) {
            ctx.expect_token(TokenKind::By)?;
            Some(self.parse_order_by_clause(ctx)?)
        } else {
            None
        };

        // Analysis of the LIMIT clause
        let limit = if ctx.match_token(TokenKind::Limit) {
            let count = ctx.expect_integer_literal()? as usize;
            Some(LimitClause {
                span: ctx.current_span(),
                count,
            })
        } else {
            None
        };

        // Analysis of SKIP
        let skip = if ctx.match_token(TokenKind::Skip) {
            let count = ctx.expect_integer_literal()? as usize;
            Some(SkipClause {
                span: ctx.current_span(),
                count,
            })
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
            sample: None,
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
            let property_expr = self.parse_expression(ctx)?;
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

            assignments.push(Assignment {
                property,
                value,
                target,
                object: None,
            });
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }
        Ok(assignments)
    }

    /// Analysis of the OVER clause
    pub fn parse_over_clause(&mut self, ctx: &mut ParseContext) -> Result<OverClause, ParseError> {
        let span = ctx.current_span();

        let edge_types = self.parse_edge_types(ctx)?;

        // Analysis direction (optional)
        let direction = if ctx.match_token(TokenKind::In) || ctx.match_token(TokenKind::Reversely) {
            EdgeDirection::In
        } else if ctx.match_token(TokenKind::Bidirect) {
            EdgeDirection::Both
        } else {
            EdgeDirection::Out
        };

        Ok(OverClause {
            span,
            edge_types,
            direction,
        })
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
        let mut expr_parser = ExprParser::new(ctx);
        expr_parser.parse_expression_with_context(ctx, ctx.expression_context_clone())
    }
}

impl Default for ClauseParser {
    fn default() -> Self {
        Self::new()
    }
}
