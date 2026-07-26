use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::query::parser::ast::stmt::{Ast, Stmt};
use crate::query::parser::core::error::ParseError;
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

/// Result of attempting to parse via an extension.
#[derive(Debug)]
pub enum ExtensionParseResult {
    Matched(Box<Stmt>),
    NotMatched,
    Error(ParseError),
}

/// Trait for parser extensions that add custom syntax.
pub trait ParserExtension: Send + Sync {
    fn name(&self) -> &str;

    fn handled_statement_tokens(&self) -> &[TokenKind];

    fn handled_expression_tokens(&self) -> &[TokenKind];

    fn try_parse_statement(&self, ctx: &mut ParseContext) -> ExtensionParseResult;

    fn try_parse_expression(
        &self,
        ctx: &mut ParseContext,
    ) -> Result<ContextualExpression, ParseError>;
}

/// Registry for parser extensions.
pub struct ExtensionRegistry {
    extensions: Vec<Box<dyn ParserExtension>>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            extensions: Vec::new(),
        }
    }

    pub fn register(&mut self, extension: Box<dyn ParserExtension>) {
        self.extensions.push(extension);
    }

    pub fn find_statement_extension(&self, token: &TokenKind) -> Option<&dyn ParserExtension> {
        self.extensions
            .iter()
            .find(|ext| ext.handled_statement_tokens().contains(token))
            .map(|ext| ext.as_ref())
    }

    pub fn find_expression_extension(&self, token: &TokenKind) -> Option<&dyn ParserExtension> {
        self.extensions
            .iter()
            .find(|ext| ext.handled_expression_tokens().contains(token))
            .map(|ext| ext.as_ref())
    }

    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.extensions.len()
    }
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Parser<'a> {
    ctx: ParseContext<'a>,
    expr_context: Arc<ExpressionAnalysisContext>,
    extensions: Option<Arc<ExtensionRegistry>>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut ctx = ParseContext::new(input);
        ctx.set_expression_context(expr_context.clone());

        Self {
            ctx,
            expr_context,
            extensions: None,
        }
    }

    pub fn from_string(input: String) -> Self {
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut ctx = ParseContext::from_string(input);
        ctx.set_expression_context(expr_context.clone());

        Self {
            ctx,
            expr_context,
            extensions: None,
        }
    }

    pub fn set_extension_registry(&mut self, registry: Arc<ExtensionRegistry>) {
        self.extensions = Some(registry);
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

        let token = self.ctx.current_token().kind.clone();

        if let Some(registry) = &self.extensions {
            if let Some(ext) = registry.find_statement_extension(&token) {
                match ext.try_parse_statement(&mut self.ctx) {
                    ExtensionParseResult::Matched(stmt) => return Ok(*stmt),
                    ExtensionParseResult::NotMatched => {}
                    ExtensionParseResult::Error(e) => {
                        if self.ctx.try_recover(e, RecoveryScope::Statement).is_err() {
                            return Err(self.ctx.take_errors().into_iter().next().unwrap_or(
                                ParseError::new(
                                    crate::query::parser::core::error::ParseErrorKind::SyntaxError,
                                    "Too many parse errors".to_string(),
                                    self.ctx.current_position(),
                                ),
                            ));
                        }
                        return self.parse_statement();
                    }
                }
            }
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
        let token = self.ctx.current_token().kind.clone();

        if let Some(registry) = &self.extensions {
            if let Some(ext) = registry.find_expression_extension(&token) {
                match ext.try_parse_expression(&mut self.ctx) {
                    Ok(expr) => return Ok(expr),
                    Err(e) => {
                        if self.ctx.is_recovery_exhausted() {
                            return Err(e);
                        }
                        self.ctx.try_recover(e, RecoveryScope::Expression)?;
                        return self.parse_expression_contextual();
                    }
                }
            }
        }

        
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
