use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::parser::ast::stmt::{Ast, Stmt};
use crate::query::parser::parsing::expr_parser::ExprParser;
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::parsing::stmt_parser::StmtParser;
use crate::query::validator::context::ExpressionAnalysisContext;

/// Parser analysis results, including the AST (Statement + Expression Context).
///
/// # Refactoring changes
/// Replace the separate `stmt` and `expr_context` with `Arc<Ast>`.
/// The `Ast` class contains both `Stmt` and `ExpressionAnalysisContext` objects.
#[derive(Debug, Clone)]
pub struct ParserResult {
    /// Parsed AST (using Arc for shared ownership)
    pub ast: Arc<Ast>,
}

pub struct Parser<'a> {
    ctx: ParseContext<'a>,
    expr_context: Arc<ExpressionAnalysisContext>,
    _expr_parser: std::marker::PhantomData<ExprParser<'a>>,
    _stmt_parser: std::marker::PhantomData<StmtParser>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut ctx = ParseContext::new(input);
        ctx.set_expression_context(expr_context.clone());

        Self {
            ctx,
            expr_context,
            _expr_parser: std::marker::PhantomData,
            _stmt_parser: std::marker::PhantomData,
        }
    }

    pub fn from_string(input: String) -> Self {
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut ctx = ParseContext::from_string(input);
        ctx.set_expression_context(expr_context.clone());

        Self {
            ctx,
            expr_context,
            _expr_parser: std::marker::PhantomData,
            _stmt_parser: std::marker::PhantomData,
        }
    }

    pub fn set_compat_mode(&mut self, enabled: bool) {
        self.ctx.set_compat_mode(enabled);
    }

    /// Parse a complete statement and return the AST with expression context.
    ///
    /// Each call to `parse()` resets the expression context to ensure isolated parsing sessions.
    /// This prevents semantic information from previous parses from polluting subsequent ones.
    /// For parsing multiple independent queries, either create a new Parser instance for each,
    /// or call parse() multiple times on the same instance (each call gets a fresh context).
    pub fn parse(&mut self) -> Result<ParserResult, crate::query::parser::core::error::ParseError> {
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        self.ctx.set_expression_context(expr_context.clone());
        self.expr_context = expr_context;

        let stmt = self.parse_statement()?;
        let ast = Ast::new(stmt, self.expr_context.clone());
        Ok(ParserResult { ast: Arc::new(ast) })
    }

    pub fn parse_statement(
        &mut self,
    ) -> Result<Stmt, crate::query::parser::core::error::ParseError> {
        let mut stmt_parser = StmtParser::new();
        stmt_parser.parse_statement(&mut self.ctx)
    }

    /// Parse the expression and return the ContextualExpression.
    pub fn parse_expression_contextual(
        &mut self,
    ) -> Result<ContextualExpression, crate::query::parser::core::error::ParseError> {
        let mut expr_parser = ExprParser::new(&self.ctx);
        expr_parser.parse_expression_with_context(&mut self.ctx, self.expr_context.clone())
    }

    /// Obtain the context of the expression.
    pub fn expression_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expr_context
    }

    /// Obtain a clone of the context in which the expression is used.
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
