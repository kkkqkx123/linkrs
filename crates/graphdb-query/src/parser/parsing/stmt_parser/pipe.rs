//! Pipeline (`|`) suffix and set-operation suffix parsing.

use super::group_by;
use super::misc;
use crate::parser::ast::stmt::*;
use crate::parser::core::error::ParseError;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::parsing::util_stmt_parser::UtilStmtParser;
use crate::parser::TokenKind;

/// Analyzing the pipe suffix (the | operator)
pub(super) fn parse_pipe_suffix(ctx: &mut ParseContext, left: Stmt) -> Result<Stmt, ParseError> {
    if ctx.match_token(TokenKind::Pipe) {
        let start_span = left.span();
        let right = parse_pipe_stage(ctx)?;
        let end_span = right.span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        let pipe_stmt = Stmt::Pipe(PipeStmt {
            span,
            left: Box::new(left),
            right: Box::new(right),
        });

        parse_pipe_suffix(ctx, pipe_stmt)
    } else if ctx.current_token().kind == TokenKind::With {
        let start_span = left.span();
        let right = UtilStmtParser::new().parse_with_statement(ctx)?;
        let end_span = right.span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        let pipe_stmt = Stmt::Pipe(PipeStmt {
            span,
            left: Box::new(left),
            right: Box::new(right),
        });

        parse_pipe_suffix(ctx, pipe_stmt)
    } else if ctx.current_token().kind == TokenKind::Return {
        let start_span = left.span();
        let right = UtilStmtParser::new().parse_return_statement(ctx)?;
        let end_span = right.span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        let pipe_stmt = Stmt::Pipe(PipeStmt {
            span,
            left: Box::new(left),
            right: Box::new(right),
        });

        parse_pipe_suffix(ctx, pipe_stmt)
    } else if ctx.current_token().kind == TokenKind::Unwind {
        let start_span = left.span();
        let right = UtilStmtParser::new().parse_unwind_statement(ctx)?;
        let end_span = right.span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        let pipe_stmt = Stmt::Pipe(PipeStmt {
            span,
            left: Box::new(left),
            right: Box::new(right),
        });

        parse_pipe_suffix(ctx, pipe_stmt)
    } else if ctx.current_token().kind == TokenKind::Group {
        let start_span = left.span();
        let right = group_by::parse_group_by_statement(ctx)?;
        let end_span = right.span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        let pipe_stmt = Stmt::Pipe(PipeStmt {
            span,
            left: Box::new(left),
            right: Box::new(right),
        });

        parse_pipe_suffix(ctx, pipe_stmt)
    } else {
        parse_set_operation_suffix(ctx, left)
    }
}

/// Parse the right-hand side of a `|` pipe operator.
fn parse_pipe_stage(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    if ctx.current_token().kind == TokenKind::Where {
        let start_span = ctx.current_span();
        ctx.next_token();
        let expression = misc::parse_expression(ctx)?;
        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);
        return Ok(Stmt::Filter(FilterStmt { span, expression }));
    }
    if ctx.current_token().kind == TokenKind::Group {
        return group_by::parse_group_by_statement(ctx);
    }
    if matches!(
        ctx.current_token().kind,
        TokenKind::Identifier(ref word) if word.eq_ignore_ascii_case("COLLECT")
    ) {
        let start_span = ctx.current_span();
        ctx.next_token();
        let mut items = Vec::new();
        loop {
            let expression = misc::parse_expression(ctx)?;
            let alias = if ctx.match_token(TokenKind::As) {
                Some(ctx.expect_identifier()?)
            } else {
                None
            };
            items.push(YieldItem { expression, alias });
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }
        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);
        return Ok(Stmt::Collect(CollectStmt { span, items }));
    }
    super::StmtParser::parse_single_statement(ctx)
}

/// Pipeline after parsing set operation statements, or end of the process.
fn parse_set_operation_suffix(ctx: &mut ParseContext, left: Stmt) -> Result<Stmt, ParseError> {
    let op_type = if ctx.match_token(TokenKind::Union) {
        if ctx.match_token(TokenKind::All) {
            SetOperationType::UnionAll
        } else {
            SetOperationType::Union
        }
    } else if ctx.match_token(TokenKind::Intersect) {
        SetOperationType::Intersect
    } else if ctx.match_token(TokenKind::SetMinus) {
        SetOperationType::Minus
    } else {
        return Ok(left);
    };

    let start_span = left.span();
    let right = super::StmtParser::parse_single_statement(ctx)?;
    let end_span = right.span();
    let span = ctx.merge_span(start_span.start, end_span.end);

    let set_op_stmt = Stmt::SetOperation(SetOperationStmt {
        span,
        op_type,
        left: Box::new(left),
        right: Box::new(right),
    });

    parse_set_operation_suffix(ctx, set_op_stmt)
}
