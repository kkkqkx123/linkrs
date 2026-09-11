//! Database-level statements: CALL, EXPORT / IMPORT / ATTACH / DETACH DATABASE.

use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::clause_parser::ClauseParser;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

/// Parse `CALL <func>(<args>) [YIELD ...]`.
pub(super) fn parse_in_query_call_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    ctx.consume_keyword("CALL")?;

    let func_name = ctx.expect_identifier()?;
    ctx.expect_token(TokenKind::LParen)?;
    let mut args = Vec::new();
    if !ctx.check_token(TokenKind::RParen) {
        loop {
            let arg = super::misc::parse_expression(ctx)?;
            args.push(arg);
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }
    }
    ctx.expect_token(TokenKind::RParen)?;

    let yield_clause = if ctx.match_token(TokenKind::Yield) {
        Some(ClauseParser::new().parse_yield_clause(ctx)?)
    } else {
        None
    };

    let end_span = ctx.current_span();
    let span = ctx.merge_span(start_span.start, end_span.end);
    Ok(Stmt::InQueryCall(InQueryCallStmt {
        span,
        func_name,
        args,
        yield_clause,
    }))
}

/// Parse `EXPORT DATABASE '<path>' [WITH OPTIONS (key=value, ...)]`.
pub(super) fn parse_export_database_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    ctx.consume_keyword("EXPORT")?;
    ctx.consume_keyword("DATABASE")?;
    let path = ctx.expect_string_literal()?;

    let options = if ctx.check_keyword("WITH") {
        ctx.consume_keyword("WITH")?;
        ctx.consume_keyword("OPTIONS")?;
        ctx.expect_token(TokenKind::LParen)?;
        let mut opts = Vec::new();
        loop {
            let key = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::Assign)?;
            let value = ctx.expect_string_literal()?;
            opts.push(ExportOption { key, value });
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }
        ctx.expect_token(TokenKind::RParen)?;
        opts
    } else {
        vec![]
    };

    let end_span = ctx.current_span();
    let span = ctx.merge_span(start_span.start, end_span.end);
    Ok(Stmt::ExportDatabase(ExportDatabaseStmt {
        span,
        path,
        options,
    }))
}

/// Parse `IMPORT DATABASE '<path>'`.
pub(super) fn parse_import_database_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    ctx.consume_keyword("IMPORT")?;
    ctx.consume_keyword("DATABASE")?;
    let path = ctx.expect_string_literal()?;

    let end_span = ctx.current_span();
    let span = ctx.merge_span(start_span.start, end_span.end);
    Ok(Stmt::ImportDatabase(ImportDatabaseStmt { span, path }))
}

/// Parse `ATTACH '<path>' AS <alias> [(DBTYPE <type> | key = value, ...)]`.
pub(super) fn parse_attach_database_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    ctx.consume_keyword("ATTACH")?;
    let path = ctx.expect_string_literal()?;
    ctx.consume_keyword("AS")?;
    let alias = ctx.expect_identifier()?;

    let mut db_type = None;
    let mut options = Vec::new();
    if ctx.match_token(TokenKind::LParen) {
        if !ctx.check_token(TokenKind::RParen) {
            loop {
                if ctx.check_keyword("DBTYPE") || ctx.check_keyword("DATABASE_TYPE") {
                    ctx.next_token();
                    let value = parse_option_value(ctx)?;
                    db_type = Some(value);
                } else {
                    let key = ctx.expect_identifier()?;
                    if ctx.match_token(TokenKind::Assign) || ctx.match_token(TokenKind::Eq) {
                        let value = parse_option_value(ctx)?;
                        options.push((key, value));
                    } else {
                        // Bare token treated as `key value` (e.g. `DBTYPE KUZU`
                        // written without the keyword alias).
                        let value = parse_option_value(ctx)?;
                        options.push((key, value));
                    }
                }
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }
        ctx.expect_token(TokenKind::RParen)?;
    }

    let end_span = ctx.current_span();
    let span = ctx.merge_span(start_span.start, end_span.end);
    Ok(Stmt::AttachDatabase(AttachDatabaseStmt {
        span,
        path,
        alias,
        db_type,
        options,
    }))
}

/// Parse `DETACH <alias>` (the database-removal form, not `DETACH DELETE`).
pub(super) fn parse_detach_database_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    ctx.consume_keyword("DETACH")?;
    let alias = ctx.expect_identifier()?;
    let end_span = ctx.current_span();
    let span = ctx.merge_span(start_span.start, end_span.end);
    Ok(Stmt::DetachDatabase(DetachDatabaseStmt { span, alias }))
}

/// Read a single option value: string literal, identifier, or numeric/boolean literal.
fn parse_option_value(ctx: &mut ParseContext) -> Result<String, ParseError> {
    let token = ctx.current_token().clone();
    let value = match token.kind {
        TokenKind::StringLiteral(s) => s,
        TokenKind::Identifier(name) => name,
        TokenKind::IntegerLiteral(n) => n.to_string(),
        TokenKind::FloatLiteral(f) => f.to_string(),
        _ => {
            return Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!("Expected option value, got {:?}", token.kind),
                ctx.current_position(),
            ));
        }
    };
    ctx.next_token();
    Ok(value)
}
