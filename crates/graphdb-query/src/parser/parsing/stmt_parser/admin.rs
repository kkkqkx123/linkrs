//! Administrative / utility statements: MIGRATE, COMMENT ON, CHECKPOINT,
//! LOAD FROM, and extension management.

use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::Expression as CoreExpression;

pub(super) fn parse_migrate_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    ctx.consume_keyword("MIGRATE")?;
    if ctx.check_keyword("PLAN") {
        ctx.consume_keyword("PLAN")?;
        ctx.consume_keyword("FOR")?;
        let is_edge = if ctx.check_keyword("TAG") {
            ctx.consume_keyword("TAG")?;
            false
        } else if ctx.check_keyword("EDGE") {
            ctx.consume_keyword("EDGE")?;
            true
        } else {
            return Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                "MIGRATE PLAN expects TAG or EDGE after FOR".to_string(),
                ctx.current_position(),
            ));
        };
        let label = ctx.expect_identifier()?;
        ctx.consume_keyword("FROM")?;
        ctx.consume_keyword("VERSION")?;
        let from_version = ctx.expect_integer_literal()? as u64;
        ctx.consume_keyword("TO")?;
        let to_version = ctx.expect_integer_literal()? as u64;
        ctx.consume_keyword("IN")?;
        let space = ctx.expect_identifier()?;
        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);
        Ok(Stmt::Migrate(MigrateStmt::Plan(MigratePlanStmt {
            span,
            space,
            label,
            is_edge,
            from_version,
            to_version,
        })))
    } else if ctx.check_keyword("EXECUTE") {
        ctx.consume_keyword("EXECUTE")?;
        let plan_json = ctx.expect_string_literal()?;
        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);
        Ok(Stmt::Migrate(MigrateStmt::Execute(MigrateExecuteStmt {
            span,
            plan_json,
        })))
    } else if ctx.check_keyword("ROLLBACK") {
        ctx.consume_keyword("ROLLBACK")?;
        let plan_json = ctx.expect_string_literal()?;
        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);
        Ok(Stmt::Migrate(MigrateStmt::Rollback(MigrateRollbackStmt {
            span,
            plan_json,
        })))
    } else {
        Err(ParseError::new(
            ParseErrorKind::SyntaxError,
            "MIGRATE expects PLAN, EXECUTE or ROLLBACK".to_string(),
            ctx.current_position(),
        ))
    }
}

/// Parse `COMMENT ON TAG|EDGE <name> IS '<string>'`.
pub(super) fn parse_comment_on_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    ctx.consume_keyword("COMMENT")?;
    ctx.consume_keyword("ON")?;

    let target = if ctx.check_keyword("TAG") {
        ctx.consume_keyword("TAG")?;
        let name = ctx.expect_identifier()?;
        CommentTarget::Tag(name)
    } else if ctx.check_keyword("EDGE") {
        ctx.consume_keyword("EDGE")?;
        let name = ctx.expect_identifier()?;
        CommentTarget::Edge(name)
    } else if ctx.check_keyword("TABLE") {
        ctx.consume_keyword("TABLE")?;
        let name = ctx.expect_identifier()?;
        CommentTarget::Table(name)
    } else {
        return Err(ParseError::new(
            ParseErrorKind::SyntaxError,
            "COMMENT ON expects TAG, EDGE, or TABLE".to_string(),
            ctx.current_position(),
        ));
    };

    ctx.consume_keyword("IS")?;
    let comment = ctx.expect_string_literal()?;

    let end_span = ctx.current_span();
    let span = ctx.merge_span(start_span.start, end_span.end);
    Ok(Stmt::CommentOn(CommentOnStmt {
        span,
        target,
        comment,
    }))
}

/// Parse `CHECKPOINT`.
pub(super) fn parse_checkpoint_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    ctx.consume_keyword("CHECKPOINT")?;
    let end_span = ctx.current_span();
    let span = ctx.merge_span(start_span.start, end_span.end);
    Ok(Stmt::Checkpoint(CheckpointStmt { span }))
}

/// Parse `LOAD FROM '<path>' [OPTIONS (key=value, ...)] [RETURN ...]`.
pub(super) fn parse_load_from_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    ctx.consume_keyword("LOAD")?;
    ctx.consume_keyword("FROM")?;

    let source = if ctx.check_keyword("GLOB") {
        ctx.consume_keyword("GLOB")?;
        ctx.expect_token(TokenKind::LParen)?;
        let pattern = ctx.expect_string_literal()?;
        ctx.expect_token(TokenKind::RParen)?;
        ScanSource::Glob(pattern)
    } else {
        let path = ctx.expect_string_literal()?;
        ScanSource::File(path)
    };

    let options = if ctx.check_keyword("OPTIONS") || ctx.check_token(TokenKind::LParen) {
        ctx.expect_token(TokenKind::LParen)?;
        let mut opts = Vec::new();
        loop {
            let key = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::Assign)?;
            let value = ctx.expect_string_literal()?;
            opts.push(LoadOption { key, value });
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }
        ctx.expect_token(TokenKind::RParen)?;
        opts
    } else {
        vec![]
    };

    let return_clause = if ctx.check_keyword("RETURN") || ctx.check_token(TokenKind::Return) {
        ctx.expect_token(TokenKind::Return)?;
        let distinct = ctx.match_token(TokenKind::Distinct);
        let mut items = Vec::new();
        if ctx.match_token(TokenKind::Star) {
            let expr = CoreExpression::variable("*");
            let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
            let id = ctx.expression_context().register_expression(expr_meta);
            let ctx_expr = ContextualExpression::new(id, ctx.expression_context_clone());
            items.push(ReturnItem::Expression {
                expression: ctx_expr,
                alias: None,
            });
        } else {
            loop {
                let expression = super::misc::parse_expression(ctx)?;
                let alias = if ctx.match_token(TokenKind::As) {
                    Some(ctx.expect_identifier()?)
                } else {
                    None
                };
                items.push(ReturnItem::Expression { expression, alias });
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }
        Some(ReturnClause {
            span: ctx.current_span(),
            items,
            distinct,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
            having_clause: None,
        })
    } else {
        None
    };

    let end_span = ctx.current_span();
    let span = ctx.merge_span(start_span.start, end_span.end);
    Ok(Stmt::LoadFrom(LoadFromStmt {
        span,
        source,
        options,
        return_clause,
    }))
}

/// Parse extension management statements:
/// `LOAD EXTENSION '<path>'`,
/// `INSTALL EXTENSION <name> FROM '<source>'`,
/// `UNINSTALL EXTENSION <name>`,
/// `UPDATE EXTENSION <name>`.
pub(super) fn parse_extension_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    let action = if ctx.check_keyword("LOAD") {
        ctx.consume_keyword("LOAD")?;
        ExtensionAction::Load
    } else if ctx.check_keyword("INSTALL") {
        ctx.consume_keyword("INSTALL")?;
        ExtensionAction::Install
    } else if ctx.check_keyword("UPDATE") {
        ctx.consume_keyword("UPDATE")?;
        ExtensionAction::Update
    } else {
        ctx.consume_keyword("UNINSTALL")?;
        ExtensionAction::Uninstall
    };
    ctx.consume_keyword("EXTENSION")?;

    let (name, source) = match action {
        ExtensionAction::Load => {
            let path = ctx.expect_string_literal()?;
            (path, None)
        }
        ExtensionAction::Install => {
            let name = ctx.expect_identifier()?;
            ctx.consume_keyword("FROM")?;
            let source = ctx.expect_string_literal()?;
            (name, Some(source))
        }
        ExtensionAction::Uninstall | ExtensionAction::Update => {
            let name = ctx.expect_identifier()?;
            (name, None)
        }
    };

    let end_span = ctx.current_span();
    let span = ctx.merge_span(start_span.start, end_span.end);
    Ok(Stmt::Extension(ExtensionStmt {
        span,
        action,
        name,
        source,
    }))
}
