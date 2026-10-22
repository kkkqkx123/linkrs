//! Database-level statements: CALL, EXPORT / IMPORT / ATTACH / DETACH DATABASE.

use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;
use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::types::expr::Expression as CoreExpression;

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
        Some(parse_call_yield_clause(ctx)?)
    } else {
        None
    };

    let span = ctx.span_since(start_span.start);
    Ok(Stmt::InQueryCall(InQueryCallStmt {
        span,
        func_name,
        args,
        yield_clause,
    }))
}

/// Parse `YIELD col [AS alias], ...` for `CALL`.
///
/// The call operator produces columns by name, so the projection is a plain
/// column list. Expressions and `WHERE` / `ORDER BY` / `LIMIT` / `SKIP` /
/// `SAMPLE` cannot be executed there and are rejected instead of parsed and
/// dropped.
fn parse_call_yield_clause(ctx: &mut ParseContext) -> Result<YieldClause, ParseError> {
    let start_span = ctx.current_span();
    if ctx.check_token(TokenKind::Distinct) {
        return Err(ParseError::new(
            ParseErrorKind::SyntaxError,
            "CALL YIELD does not support DISTINCT".to_string(),
            ctx.current_position(),
        ));
    }

    let mut items = Vec::new();
    loop {
        let name = ctx.expect_identifier()?;
        let alias = if ctx.match_token(TokenKind::As) {
            Some(ctx.expect_identifier()?)
        } else {
            None
        };

        let expression = CoreExpression::variable(&name);
        let meta = linkrs_core::types::expr::ExpressionMeta::new(expression);
        let id = ctx.expression_context().register_expression(meta);
        let expression = ContextualExpression::new(id, ctx.expression_context_clone());
        items.push(YieldItem { expression, alias });

        if !ctx.match_token(TokenKind::Comma) {
            break;
        }
    }

    // A pipe, WITH or RETURN continues the statement; anything else here
    // would have been a YIELD expression or filter clause.
    let trailing = &ctx.current_token().kind;
    let continues_statement = matches!(
        trailing,
        TokenKind::Eof
            | TokenKind::Semicolon
            | TokenKind::Pipe
            | TokenKind::With
            | TokenKind::Return
    );
    if !continues_statement {
        return Err(ParseError::new(
            ParseErrorKind::SyntaxError,
            "CALL YIELD accepts column names only; expressions, WHERE, ORDER BY, LIMIT, SKIP and SAMPLE are not supported"
                .to_string(),
            ctx.current_position(),
        ));
    }

    Ok(YieldClause {
        span: ctx.span_since(start_span.start),
        distinct: false,
        items,
        where_clause: None,
        order_by: None,
        limit: None,
        skip: None,
        sample: None,
    })
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

    let span = ctx.span_since(start_span.start);
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

    let span = ctx.span_since(start_span.start);
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

    let span = ctx.span_since(start_span.start);
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
    let span = ctx.span_since(start_span.start);
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
