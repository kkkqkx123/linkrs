//! Tool Statement Parsing Module
//!
//! Responsible for parsing statements related to tool functions, including USE, SHOW, EXPLAIN, FETCH, LOOKUP, UNWIND, RETURN, WITH, YIELD, SET, REMOVE, etc.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::parser::ast::stmt::*;
use crate::query::parser::ast::types::OrderDirection;
use crate::query::parser::core::error::{ParseError, ParseErrorKind};
use crate::query::parser::parsing::clause_parser::ClauseParser;
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::parsing::ExprParser;
use crate::query::parser::TokenKind;

/// Tool Syntax Parser
pub struct UtilStmtParser;

impl UtilStmtParser {
    pub fn new() -> Self {
        Self
    }

    /// Analysis of the USE statement
    pub fn parse_use_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Use)?;

        let space = ctx.expect_identifier()?;

        Ok(Stmt::Use(UseStmt {
            span: start_span,
            space,
        }))
    }

    /// Analysis of the SHOW statement
    pub fn parse_show_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Show)?;

        // Check the “SHOW CREATE” command.
        if ctx.check_token(TokenKind::Create) {
            return self.parse_show_create_internal(ctx, start_span);
        }

        // Check the SHOW USERS command.
        if ctx.check_token(TokenKind::Users) {
            return self.parse_show_users_internal(ctx, start_span);
        }

        // Check “SHOW ROLES”.
        if ctx.check_token(TokenKind::Roles) {
            return self.parse_show_roles_internal(ctx, start_span);
        }

        let target = if ctx.match_token(TokenKind::Spaces) {
            ShowTarget::Spaces
        } else if ctx.match_token(TokenKind::Tags) {
            ShowTarget::Tags
        } else if ctx.match_token(TokenKind::Edges) {
            ShowTarget::Edges
        } else {
            ShowTarget::Spaces
        };

        Ok(Stmt::Show(ShowStmt {
            span: start_span,
            target,
        }))
    }

    /// Analyzing the internal methods of the SHOW CREATE statement
    pub fn parse_show_create_internal(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::query::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        ctx.expect_token(TokenKind::Create)?;

        let target = if ctx.match_token(TokenKind::Space) {
            ShowCreateTarget::Space(ctx.expect_identifier()?)
        } else if ctx.match_token(TokenKind::Tag) {
            ShowCreateTarget::Tag(ctx.expect_identifier()?)
        } else if ctx.match_token(TokenKind::Edge) {
            ShowCreateTarget::Edge(ctx.expect_identifier()?)
        } else if ctx.match_token(TokenKind::Index) {
            ShowCreateTarget::Index(ctx.expect_identifier()?)
        } else {
            return Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                "Expected SPACE, TAG, EDGE, or INDEX after SHOW CREATE".to_string(),
                ctx.current_position(),
            ));
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::ShowCreate(ShowCreateStmt { span, target }))
    }

    /// Analysis of the internal methods of the SHOW USERS command
    fn parse_show_users_internal(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::query::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        ctx.expect_token(TokenKind::Users)?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::ShowUsers(ShowUsersStmt { span }))
    }

    /// Analysis of the internal methods of the SHOW ROLES command
    fn parse_show_roles_internal(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::query::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        ctx.expect_token(TokenKind::Roles)?;

        // Optional IN <space_name> clause
        let space_name = if ctx.match_token(TokenKind::In) {
            Some(ctx.expect_identifier()?)
        } else {
            None
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::ShowRoles(ShowRolesStmt { span, space_name }))
    }

    /// Analyzing the EXPLAIN statement
    pub fn parse_explain_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let _start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Explain)?;

        // After “EXPLAIN”, a substatement needs to be parsed.
        // Here, we need to invoke the main parser, but due to circular dependency issues, we are returning a placeholder.
        // The actual parsing will be handled within the StmtParser.
        Err(ParseError::new(
            ParseErrorKind::SyntaxError,
            "EXPLAIN should be handled by main parser".to_string(),
            ctx.current_position(),
        ))
    }

    /// Analysis of the FETCH statement
    pub fn parse_fetch_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Fetch)?;

        // The FETCH PROP ON <tag> <ids> syntax is supported.
        let _with_props = ctx.match_token(TokenKind::Prop);

        let target = if ctx.match_token(TokenKind::On) {
            // Check for * (all tags)
            let tag_name = if ctx.match_token(TokenKind::Star) {
                None // None means all tags
            } else {
                Some(ctx.expect_identifier()?)
            };

            let first_expr = self.parse_expression(ctx)?;
            if ctx.check_token(TokenKind::Arrow) {
                ctx.expect_token(TokenKind::Arrow)?;
                let dst = self.parse_expression(ctx)?;
                let rank = if ctx.match_token(TokenKind::At) {
                    Some(self.parse_expression(ctx)?)
                } else {
                    None
                };
                FetchTarget::Edges {
                    src: first_expr,
                    dst,
                    edge_type: tag_name.expect("Edge type name is required"),
                    rank,
                    properties: None,
                }
            } else {
                let mut ids = vec![first_expr];
                while ctx.match_token(TokenKind::Comma) {
                    ids.push(self.parse_expression(ctx)?);
                }
                FetchTarget::Vertices {
                    tag_name,
                    ids,
                    properties: None,
                }
            }
        } else if ctx.match_token(TokenKind::Tag) {
            let tag_name = Some(ctx.expect_identifier()?);
            let ids = self.parse_expression_list(ctx)?;
            FetchTarget::Vertices {
                tag_name,
                ids,
                properties: None,
            }
        } else if ctx.match_token(TokenKind::Edge) {
            let edge_type = ctx.expect_identifier()?;
            let src = self.parse_expression(ctx)?;
            ctx.expect_token(TokenKind::Arrow)?;
            let dst = self.parse_expression(ctx)?;
            let rank = if ctx.match_token(TokenKind::At) {
                Some(self.parse_expression(ctx)?)
            } else {
                None
            };
            FetchTarget::Edges {
                src,
                dst,
                edge_type,
                rank,
                properties: None,
            }
        } else {
            let ids = self.parse_expression_list(ctx)?;
            FetchTarget::Vertices {
                tag_name: None,
                ids,
                properties: None,
            }
        };

        Ok(Stmt::Fetch(FetchStmt {
            span: start_span,
            target,
        }))
    }

    /// Analysis of the LOOKUP statement
    pub fn parse_lookup_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Lookup)?;

        let target = if ctx.match_token(TokenKind::On) {
            // Check if it's an EDGE or TAG keyword followed by identifier
            if ctx.current_token().kind == TokenKind::Edge {
                // LOOKUP ON EDGE <name>
                ctx.next_token(); // consume EDGE
                let name = ctx.expect_identifier()?;
                LookupTarget::Edge(name)
            } else if ctx.current_token().kind == TokenKind::Tag {
                // LOOKUP ON TAG <name>
                ctx.next_token(); // consume TAG
                let name = ctx.expect_identifier()?;
                LookupTarget::Tag(name)
            } else {
                // LOOKUP ON <name> - type will be resolved during validation
                let name = ctx.expect_identifier()?;
                LookupTarget::Unspecified(name)
            }
        } else {
            LookupTarget::Unspecified(String::new())
        };

        let where_clause = if ctx.match_token(TokenKind::Where) {
            Some(self.parse_expression(ctx)?)
        } else {
            None
        };

        let yield_clause = if ctx.match_token(TokenKind::Yield) {
            Some(ClauseParser::new().parse_yield_clause(ctx)?)
        } else {
            None
        };

        Ok(Stmt::Lookup(LookupStmt {
            span: start_span,
            target,
            where_clause,
            yield_clause,
        }))
    }

    /// Analysis of the UNWIND statement
    pub fn parse_unwind_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Unwind)?;

        let expression = self.parse_expression(ctx)?;

        ctx.match_token(TokenKind::As);

        let variable = ctx.expect_identifier()?;

        let return_clause = if ctx.match_token(TokenKind::Return) {
            Some(ClauseParser::new().parse_return_clause(ctx)?)
        } else {
            None
        };

        let order_by = None;
        let limit = None;
        let skip = None;

        Ok(Stmt::Unwind(UnwindStmt {
            span: start_span,
            expression,
            variable,
            return_clause,
            order_by,
            limit,
            skip,
        }))
    }

    /// Analysis of the RETURN statement
    pub fn parse_return_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Return)?;

        let return_clause = ClauseParser::new().parse_return_clause(ctx)?;

        Ok(Stmt::Return(ReturnStmt {
            span: start_span,
            items: return_clause.items,
            distinct: return_clause.distinct,
            order_by: return_clause.order_by,
            skip: return_clause.skip.map(|s| s.count),
            limit: return_clause.limit.map(|l| l.count),
        }))
    }

    /// Analysis of the WITH statement
    pub fn parse_with_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::With)?;

        let mut items = Vec::new();
        let distinct = ctx.match_token(TokenKind::Distinct);

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

        // Analysis of SKIP
        let skip = if ctx.match_token(TokenKind::Skip) {
            let count = ctx.expect_integer_literal()? as usize;
            Some(count)
        } else {
            None
        };

        // Analysis of the LIMIT clause
        let limit = if ctx.match_token(TokenKind::Limit) {
            let count = ctx.expect_integer_literal()? as usize;
            Some(count)
        } else {
            None
        };

        Ok(Stmt::With(WithStmt {
            span: start_span,
            items,
            where_clause,
            distinct,
            order_by,
            skip,
            limit,
        }))
    }

    /// Analysis of the YIELD statement
    pub fn parse_yield_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Yield)?;

        let yield_clause = ClauseParser::new().parse_yield_clause(ctx)?;

        Ok(Stmt::Yield(YieldStmt {
            span: start_span,
            items: yield_clause.items,
            where_clause: yield_clause.where_clause,
            distinct: false,
            order_by: yield_clause.order_by,
            skip: yield_clause.skip.map(|s| s.count),
            limit: yield_clause.limit.map(|l| l.count),
        }))
    }

    /// Analyzing the SET statement
    pub fn parse_set_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Set)?;

        let set_clause = ClauseParser::new().parse_set_clause(ctx)?;

        Ok(Stmt::Set(SetStmt {
            span: start_span,
            assignments: set_clause.assignments,
        }))
    }

    /// Analysis of the REMOVE statement
    pub fn parse_remove_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Remove)?;

        let mut items = Vec::new();
        loop {
            items.push(self.parse_expression(ctx)?);
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }

        Ok(Stmt::Remove(RemoveStmt {
            span: start_span,
            items,
        }))
    }

    /// Parse the list of expressions
    fn parse_expression_list(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Vec<ContextualExpression>, ParseError> {
        let mut expressions = Vec::new();

        loop {
            expressions.push(self.parse_expression(ctx)?);
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }

        Ok(expressions)
    }

    /// Analyzing the expression
    fn parse_expression(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ContextualExpression, ParseError> {
        let mut expr_parser = ExprParser::new(ctx);
        expr_parser.parse_expression_with_context(ctx, ctx.expression_context_clone())
    }

    /// Analysis of the ORDER BY clause
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
}

impl Default for UtilStmtParser {
    fn default() -> Self {
        Self::new()
    }
}
