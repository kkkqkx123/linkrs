use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::query::parser::ast::stmt::{Ast, Stmt};
use crate::query::parser::core::error::{ParseError, ParseErrorKind};
use crate::query::parser::parsing::expr_parser::parse_expression_with_context;
use crate::query::parser::parsing::parse_context::{ParseContext, RecoveryScope};
use crate::query::parser::parsing::stmt_parser::StmtParser;
use crate::query::parser::TokenKind;

/// Parser analysis results, including the AST (Statement + Expression Context).
#[derive(Debug, Clone)]
pub struct ParserResult {
    /// Parsed AST (using Arc for shared ownership)
    pub ast: Arc<Ast>,
}

pub struct Parser<'a> {
    ctx: ParseContext<'a>,
    expr_context: Arc<ExpressionAnalysisContext>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut ctx = ParseContext::new(input);
        ctx.set_expression_context(expr_context.clone());

        Self {
            ctx,
            expr_context,
        }
    }

    pub fn from_string(input: String) -> Self {
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut ctx = ParseContext::from_string(input);
        ctx.set_expression_context(expr_context.clone());

        Self {
            ctx,
            expr_context,
        }
    }

    pub fn set_compat_mode(&mut self, enabled: bool) {
        self.ctx.set_compat_mode(enabled);
    }

    pub fn parse(&mut self) -> Result<ParserResult, ParseError> {
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        self.ctx.set_expression_context(expr_context.clone());
        self.expr_context = expr_context;
        self.ctx.reset_recovery_count();

        let stmt = self.parse_statement()?;

        // A statement must consume the whole input. An optional trailing
        // semicolon terminator is allowed; any further token is a syntax
        // error, otherwise trailing garbage such as `COMMIT junk` would be
        // silently truncated to `COMMIT` and executed.
        if self.ctx.match_token(TokenKind::Semicolon) {
            // Optional statement terminator consumed.
        }
        if self.ctx.current_token().kind != TokenKind::Eof {
            self.ctx.add_error(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!(
                    "Unexpected token after end of statement: {:?}",
                    self.ctx.current_token().kind
                ),
                self.ctx.current_position(),
            ));
        }

        let ast = Ast::new(stmt, self.expr_context.clone());
        Ok(ParserResult { ast: Arc::new(ast) })
    }

    pub fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        if self.ctx.is_recovery_exhausted() {
            return Err(ParseError::new(
                crate::query::parser::core::error::ParseErrorKind::SyntaxError,
                "Too many parse errors, aborting".to_string(),
                self.ctx.current_position(),
            ));
        }

        match StmtParser::parse_statement(&mut self.ctx) {
            Ok(stmt) => Ok(stmt),
            Err(e) => {
                if self.ctx.is_recovery_exhausted() {
                    Err(e)
                } else {
                    self.ctx.try_recover(e, RecoveryScope::Statement)?;
                    self.parse_statement()
                }
            }
        }
    }

    pub fn parse_expression_contextual(&mut self) -> Result<ContextualExpression, ParseError> {
        match parse_expression_with_context(&mut self.ctx, self.expr_context.clone()) {
            Ok(expr) => Ok(expr),
            Err(e) => {
                if self.ctx.is_recovery_exhausted() {
                    Err(e)
                } else {
                    self.ctx.try_recover(e, RecoveryScope::Expression)?;
                    self.parse_expression_contextual()
                }
            }
        }
    }

    pub fn expression_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expr_context
    }

    pub fn expression_context_clone(&self) -> Arc<ExpressionAnalysisContext> {
        self.expr_context.clone()
    }

    pub fn has_errors(&self) -> bool {
        self.ctx.has_errors()
    }

    pub fn errors(&self) -> &crate::query::parser::ParseErrors {
        self.ctx.errors()
    }

    pub fn take_errors(&mut self) -> crate::query::parser::ParseErrors {
        self.ctx.take_errors()
    }
}
