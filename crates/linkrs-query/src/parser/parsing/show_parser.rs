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

impl Default for ShowParser {
    fn default() -> Self {
        Self
    }
}

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
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::ShowSessions(ShowSessionsStmt { span }))
        } else if ctx.check_token(TokenKind::Queries) {
            ctx.expect_token(TokenKind::Queries)?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::ShowQueries(ShowQueriesStmt { span }))
        } else if ctx.check_token(TokenKind::Configs) {
            ctx.expect_token(TokenKind::Configs)?;
            let module = if ctx.is_identifier_or_in_token() {
                Some(ctx.expect_identifier()?)
            } else {
                None
            };
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::ShowConfigs(ShowConfigsStmt { span, module }))
        } else if ctx.check_token(TokenKind::Spaces) {
            ctx.expect_token(TokenKind::Spaces)?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Spaces,
            }))
        } else if ctx.check_token(TokenKind::Tags) {
            ctx.expect_token(TokenKind::Tags)?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Tags,
            }))
        } else if ctx.check_token(TokenKind::Edges) {
            ctx.expect_token(TokenKind::Edges)?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Edges,
            }))
        } else if ctx.check_token(TokenKind::Indexes) {
            ctx.expect_token(TokenKind::Indexes)?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Indexes,
            }))
        } else if ctx.check_token(TokenKind::Index) {
            ctx.expect_token(TokenKind::Index)?;
            let index_name = ctx.expect_identifier()?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Index(index_name),
            }))
        } else if ctx.check_token(TokenKind::Stats) {
            ctx.expect_token(TokenKind::Stats)?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Stats,
            }))
        } else if ctx.check_token(TokenKind::Users) {
            ctx.expect_token(TokenKind::Users)?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::ShowUsers(ShowUsersStmt { span }))
        } else if ctx.check_token(TokenKind::Roles) {
            UtilStmtParser::new().parse_show_roles_internal(ctx, start_span)
        } else if ctx.check_token(TokenKind::Create) {
            UtilStmtParser::new().parse_show_create_internal(ctx, start_span)
        } else if ctx.check_token(TokenKind::Functions) {
            ctx.expect_token(TokenKind::Functions)?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Functions,
            }))
        } else if ctx.check_token(TokenKind::Graphs) {
            ctx.expect_token(TokenKind::Graphs)?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Graphs,
            }))
        } else if ctx.check_token(TokenKind::Macros) {
            ctx.expect_token(TokenKind::Macros)?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Macros,
            }))
        } else if ctx.check_keyword("ATTACHED") {
            ctx.consume_keyword("ATTACHED")?;
            ctx.consume_keyword("DATABASES")?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::AttachedDatabases,
            }))
        } else if ctx.check_keyword("EXTENSIONS") {
            ctx.consume_keyword("EXTENSIONS")?;
            let span = ctx.span_since(start_span.start);
            Ok(Stmt::Show(ShowStmt {
                span,
                target: ShowTarget::Extensions,
            }))
        } else {
            Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                format!(
                    "Unknown SHOW target: {:?}; supported targets: SESSIONS, QUERIES, CONFIGS, \
                     SPACES, TAGS, EDGES, INDEXES, INDEX, STATS, USERS, ROLES, CREATE, \
                     FUNCTIONS, GRAPHS, MACROS, ATTACHED DATABASES, EXTENSIONS",
                    ctx.peek_token().kind
                ),
                ctx.current_position(),
            ))
        }
    }
}
