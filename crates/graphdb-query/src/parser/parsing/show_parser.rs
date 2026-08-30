//! SHOW Statement Parsing Module
//!
//! Responsible for parsing SHOW statements, including SHOW SESSIONS, SHOW QUERIES,
//! SHOW CONFIGS, SHOW SPACES, SHOW TAGS, SHOW EDGES, SHOW USERS, SHOW ROLES,
//! and SHOW CREATE.

use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::parsing::util_stmt_parser::UtilStmtParser;
use crate::parser::TokenKind;

/// SHOW Statement Parser
pub struct ShowParser;

impl ShowParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse extended SHOW statements (SESSIONS, QUERIES, CONFIGS, SPACES, etc.)
    pub fn parse_show_statement_extended(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Show)?;

        if ctx.check_token(TokenKind::Sessions) {
            ctx.expect_token(TokenKind::Sessions)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::ShowSessions(ShowSessionsStmt { span }))
        } else if ctx.check_token(TokenKind::Queries) {
            ctx.expect_token(TokenKind::Queries)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::ShowQueries(ShowQueriesStmt { span }))
        } else if ctx.check_token(TokenKind::Configs) {
            ctx.expect_token(TokenKind::Configs)?;
            let module = if ctx.is_identifier_or_in_token() {
                Some(ctx.expect_identifier()?)
            } else {
                None
            };
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::ShowConfigs(ShowConfigsStmt { span, module }))
        } else if ctx.check_token(TokenKind::Spaces) {
            ctx.expect_token(TokenKind::Spaces)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Spaces,
            }))
        } else if ctx.check_token(TokenKind::Tags) {
            ctx.expect_token(TokenKind::Tags)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Tags,
            }))
        } else if ctx.check_token(TokenKind::Edges) {
            ctx.expect_token(TokenKind::Edges)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Edges,
            }))
        } else if ctx.check_token(TokenKind::Hosts) {
            ctx.expect_token(TokenKind::Hosts)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Spaces,
            }))
        } else if ctx.check_token(TokenKind::Parts) {
            ctx.expect_token(TokenKind::Parts)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Spaces,
            }))
        } else if ctx.check_token(TokenKind::Users) {
            ctx.expect_token(TokenKind::Users)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::ShowUsers(ShowUsersStmt { span }))
        } else if ctx.check_token(TokenKind::Roles) {
            UtilStmtParser::new().parse_show_roles_internal(ctx, start_span)
        } else if ctx.check_token(TokenKind::Create) {
            UtilStmtParser::new().parse_show_create_internal(ctx, start_span)
        } else {
            Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                format!("Unknown SHOW Target: {:?}", ctx.peek_token().kind),
                ctx.current_position(),
            ))
        }
    }
}
