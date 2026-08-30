//! Transaction Statement Parsing Module
//!
//! Responsible for parsing transaction-related statements: BEGIN, COMMIT,
//! ROLLBACK, SAVEPOINT, and RELEASE SAVEPOINT.

use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

/// Transaction Parser
pub struct TransactionParser;

impl TransactionParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse BEGIN TRANSACTION statement
    ///
    /// Grammar: `BEGIN [TRANSACTION] [READ ONLY | READ WRITE]`
    pub fn parse_begin_transaction(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Begin)?;

        if ctx.check_token(TokenKind::Transaction) {
            ctx.expect_token(TokenKind::Transaction)?;
        }

        let mut read_only = None;
        if ctx.check_token(TokenKind::Read) {
            ctx.expect_token(TokenKind::Read)?;
            let mode = if ctx.check_token(TokenKind::Only) {
                ctx.expect_token(TokenKind::Only)?;
                true
            } else if ctx.check_token(TokenKind::Write) {
                ctx.expect_token(TokenKind::Write)?;
                false
            } else {
                let pos = ctx.current_position();
                return Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken,
                    format!(
                        "Expected READ ONLY or READ WRITE, found {:?}",
                        ctx.current_token().kind
                    ),
                    pos,
                )
                .with_expected_tokens(vec!["READ ONLY".to_string(), "READ WRITE".to_string()]));
            };
            read_only = Some(mode);
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::BeginTransaction(BeginTransactionStmt {
            span,
            read_only,
        }))
    }

    /// Parse COMMIT TRANSACTION statement
    pub fn parse_commit_transaction(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Commit)?;

        if ctx.check_token(TokenKind::Transaction) {
            ctx.expect_token(TokenKind::Transaction)?;
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::CommitTransaction(CommitTransactionStmt { span }))
    }

    /// Parse ROLLBACK TRANSACTION statement
    ///
    /// Grammar: `ROLLBACK [TRANSACTION] [TO <savepoint-name>]`
    pub fn parse_rollback_transaction(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Rollback)?;

        if ctx.check_token(TokenKind::Transaction) {
            ctx.expect_token(TokenKind::Transaction)?;
        }

        let savepoint_name = if ctx.check_token(TokenKind::To) {
            ctx.expect_token(TokenKind::To)?;
            Some(ctx.expect_identifier()?)
        } else {
            None
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::RollbackTransaction(RollbackTransactionStmt {
            span,
            savepoint_name,
        }))
    }

    /// Parse SAVEPOINT statement
    ///
    /// Grammar: `SAVEPOINT <savepoint-name>`
    pub fn parse_savepoint_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Savepoint)?;
        let name = ctx.expect_identifier()?;
        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);
        Ok(Stmt::Savepoint(SavepointStmt { span, name }))
    }

    /// Parse RELEASE SAVEPOINT statement
    ///
    /// Grammar: `RELEASE SAVEPOINT <savepoint-name>`
    pub fn parse_release_savepoint(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Release)?;
        ctx.expect_token(TokenKind::Savepoint)?;
        let name = ctx.expect_identifier()?;
        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);
        Ok(Stmt::ReleaseSavepoint(ReleaseSavepointStmt { span, name }))
    }
}
