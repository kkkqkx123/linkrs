//! Session Statement Parsing Module
//!
//! Responsible for parsing session-related statements: LET (variable assignment)
//! and KILL QUERY.

use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::expr_parser::parse_expression_with_context;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

/// Session Parser
pub struct SessionParser;

impl Default for SessionParser {
    fn default() -> Self {
        Self
    }
}

impl SessionParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse LET statement
    ///
    /// Grammar: `LET [$]name = expr`. The name must be a valid identifier
    /// (`[A-Za-z_][A-Za-z0-9_]*`); the right-hand side is parsed through the
    /// standard expression pipeline so it may reference `$name` session
    /// variables and `@name` parameters.
    pub fn parse_let_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Let)?;

        let _ = ctx.match_token(TokenKind::Dollar);

        let name = match ctx.expect_identifier() {
            Ok(name) => name,
            Err(_) => {
                let pos = ctx.current_position();
                let display_name = ctx.current_token().lexeme.clone();
                return Err(ParseError::new(
                    ParseErrorKind::SyntaxError,
                    format!("Invalid session variable name '{}'", display_name),
                    pos,
                ));
            }
        };
        let valid = !name.is_empty()
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            let pos = ctx.current_position();
            return Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                format!("Invalid session variable name '{}'", name),
                pos,
            ));
        }

        if !ctx.check_token(TokenKind::Assign) {
            let pos = ctx.current_position();
            return Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                "LET requires an assignment: LET $name = expr".to_string(),
                pos,
            ));
        }
        ctx.expect_token(TokenKind::Assign)?;

        let expression = parse_expression_with_context(ctx, ctx.expression_context_clone())?;
        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::AssignVariable(AssignVariableStmt {
            span,
            name,
            expression,
        }))
    }

    /// Parse KILL QUERY statement
    pub fn parse_kill_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Kill)?;
        ctx.expect_token(TokenKind::Query)?;

        let session_id = ctx.expect_integer_literal()?;
        ctx.expect_token(TokenKind::Comma)?;
        let plan_id = ctx.expect_integer_literal()?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::KillQuery(KillQueryStmt {
            span,
            session_id,
            plan_id,
        }))
    }
}
