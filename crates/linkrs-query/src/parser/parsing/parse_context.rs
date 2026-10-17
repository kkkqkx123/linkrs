use std::sync::Arc;

use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::lexing::LexError;
use crate::parser::lexing::Lexer;
use crate::parser::ParseErrors;
use crate::parser::Token;
use crate::parser::TokenKind;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::{Position, Span};

const RECOVERY_LIMIT: usize = 5;

const STATEMENT_SYNC_TOKENS: &[TokenKind] = &[
    TokenKind::Semicolon,
    TokenKind::Match,
    TokenKind::Go,
    TokenKind::Fetch,
    TokenKind::Insert,
    TokenKind::Copy,
    TokenKind::Delete,
    TokenKind::Update,
    TokenKind::Upsert,
    TokenKind::Create,
    TokenKind::Drop,
    TokenKind::Show,
    TokenKind::Explain,
    TokenKind::Profile,
    TokenKind::Group,
    TokenKind::Lookup,
    TokenKind::Find,
    TokenKind::Return,
    TokenKind::With,
    TokenKind::Unwind,
    TokenKind::Set,
    TokenKind::Use,
    TokenKind::Begin,
    TokenKind::Commit,
    TokenKind::Rollback,
    TokenKind::Savepoint,
    TokenKind::Release,
    TokenKind::Search,
    TokenKind::Dollar,
    TokenKind::Eof,
];

const CLAUSE_SYNC_TOKENS: &[TokenKind] = &[
    TokenKind::Semicolon,
    TokenKind::Where,
    TokenKind::Return,
    TokenKind::With,
    TokenKind::Yield,
    TokenKind::Order,
    TokenKind::Limit,
    TokenKind::Skip,
    TokenKind::Having,
    TokenKind::Group,
    TokenKind::Union,
    TokenKind::Intersect,
    TokenKind::SetMinus,
    TokenKind::Pipe,
    TokenKind::Match,
    TokenKind::Go,
    TokenKind::Find,
    TokenKind::Lookup,
    TokenKind::Insert,
    TokenKind::Update,
    TokenKind::Delete,
    TokenKind::And,
    TokenKind::Or,
    TokenKind::Eof,
];

const EXPR_SYNC_TOKENS: &[TokenKind] = &[
    TokenKind::Comma,
    TokenKind::RParen,
    TokenKind::RBracket,
    TokenKind::RBrace,
    TokenKind::Then,
    TokenKind::Else,
    TokenKind::End,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryScope {
    Statement,
    Clause,
    Expression,
}

pub struct ParseContext<'a> {
    lexer: Lexer<'a>,
    current_token: Token,
    next_token: Option<Token>,
    errors: ParseErrors,
    compat_mode: bool,
    upsert_mode: bool,
    edge_syntax_mode: bool,
    /// When true, a top-level `|` is NOT consumed as the bitwise-OR operator.
    /// Set while parsing a list-comprehension source/filter operand so the
    /// comprehension body separator `|` is not swallowed; cleared again inside
    /// any bracketed group where `|` is unambiguously bitwise-OR.
    suppress_pipe_or: bool,
    recursion_depth: usize,
    max_recursion_depth: usize,
    expr_context: Arc<ExpressionAnalysisContext>,
    recovery_count: usize,
    /// Shared user-defined type-alias catalog. When set, unknown type names
    /// resolve through aliases (`CAST(x AS alias)`, DDL column types).
    type_alias_manager: Option<Arc<linkrs_core::metadata::TypeAliasManager>>,
    /// Recursive comprehension parsed during edge pattern parsing
    recursive_comprehension: Option<crate::parser::ast::pattern::RecursiveComprehension>,
}

pub struct ParseContextCheckpoint {
    lexer_ckpt: crate::parser::lexing::LexerCheckpoint,
    current_token: Token,
    next_token: Option<Token>,
}

mod literals;

impl<'a> ParseContext<'a> {
    pub fn new(input: &'a str) -> Self {
        let lexer = Lexer::new(input);
        let current_token = lexer.current_token().clone();
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        Self {
            lexer,
            current_token,
            next_token: None,
            errors: ParseErrors::new(),
            compat_mode: false,
            upsert_mode: false,
            edge_syntax_mode: false,
            suppress_pipe_or: false,
            recursion_depth: 0,
            max_recursion_depth: 100,
            expr_context,
            recovery_count: 0,
            type_alias_manager: None,
            recursive_comprehension: None,
        }
    }

    pub fn from_string(input: String) -> Self {
        let lexer = Lexer::from_string(input);
        let current_token = lexer.current_token().clone();
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        Self {
            lexer,
            current_token,
            next_token: None,
            errors: ParseErrors::new(),
            compat_mode: false,
            upsert_mode: false,
            edge_syntax_mode: false,
            suppress_pipe_or: false,
            recursion_depth: 0,
            max_recursion_depth: 100,
            expr_context,
            recovery_count: 0,
            type_alias_manager: None,
            recursive_comprehension: None,
        }
    }

    pub fn checkpoint(&self) -> ParseContextCheckpoint {
        ParseContextCheckpoint {
            lexer_ckpt: self.lexer.checkpoint(),
            current_token: self.current_token.clone(),
            next_token: self.next_token.clone(),
        }
    }

    pub fn restore(&mut self, ckpt: ParseContextCheckpoint) {
        self.lexer.restore(ckpt.lexer_ckpt);
        self.current_token = ckpt.current_token;
        self.next_token = ckpt.next_token;
    }

    pub fn set_expression_context(&mut self, expr_context: Arc<ExpressionAnalysisContext>) {
        self.expr_context = expr_context;
    }

    pub fn expression_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expr_context
    }

    /// Attach the shared user-defined type-alias catalog.
    pub fn set_type_alias_manager(
        &mut self,
        manager: Arc<linkrs_core::metadata::TypeAliasManager>,
    ) {
        self.type_alias_manager = Some(manager);
    }

    /// Resolve a raw type name through builtin types, then user-defined
    /// aliases. Returns `None` when the name denotes neither.
    pub fn resolve_named_type(&self, name: &str) -> Option<linkrs_core::DataType> {
        if let Ok(builtin) = name.parse::<linkrs_core::DataType>() {
            return Some(builtin);
        }
        let manager = self.type_alias_manager.as_ref()?;
        manager
            .resolve(name, &|text| text.parse::<linkrs_core::DataType>().ok())
            .ok()
    }

    pub fn expression_context_clone(&self) -> Arc<ExpressionAnalysisContext> {
        self.expr_context.clone()
    }

    pub fn take_recursive_comprehension(
        &mut self,
    ) -> Option<crate::parser::ast::pattern::RecursiveComprehension> {
        self.recursive_comprehension.take()
    }

    pub fn set_recursive_comprehension(
        &mut self,
        rc: crate::parser::ast::pattern::RecursiveComprehension,
    ) {
        self.recursive_comprehension = Some(rc);
    }

    pub fn lexer(&self) -> &Lexer<'a> {
        &self.lexer
    }

    pub fn lexer_mut(&mut self) -> &mut Lexer<'a> {
        &mut self.lexer
    }

    pub fn set_compat_mode(&mut self, enabled: bool) {
        self.compat_mode = enabled;
    }

    pub fn set_upsert_mode(&mut self, enabled: bool) {
        self.upsert_mode = enabled;
    }

    pub fn is_upsert_mode(&self) -> bool {
        self.upsert_mode
    }

    /// Whether the parser is inside a DML edge statement (`src -> dst`)
    /// where `->` is an edge arrow rather than the JSON access operator.
    pub fn is_edge_syntax_mode(&self) -> bool {
        self.edge_syntax_mode
    }

    /// Run `f` with DML edge syntax enabled, then restore the previous
    /// mode even on error.
    pub fn with_edge_syntax_mode<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        let saved = self.edge_syntax_mode;
        self.edge_syntax_mode = true;
        let result = f(self);
        self.edge_syntax_mode = saved;
        result
    }

    /// Whether `|` should be treated as a list-comprehension separator instead
    /// of the bitwise-OR operator in the current parsing position.
    pub fn is_pipe_or_suppressed(&self) -> bool {
        self.suppress_pipe_or
    }

    /// Run `f` with `suppress_pipe_or` forced to `value`, restoring the
    /// previous setting afterwards (even on error).
    pub fn with_pipe_or_suppression<T>(
        &mut self,
        value: bool,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        let saved = self.suppress_pipe_or;
        self.suppress_pipe_or = value;
        let result = f(self);
        self.suppress_pipe_or = saved;
        result
    }

    pub fn enter_recursion(&mut self) -> Result<(), ParseError> {
        self.recursion_depth += 1;
        if self.recursion_depth > self.max_recursion_depth {
            let pos = self.current_position();
            Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                "Recursion limit exceeded".to_string(),
                pos,
            ))
        } else {
            Ok(())
        }
    }

    pub fn exit_recursion(&mut self) {
        if self.recursion_depth > 0 {
            self.recursion_depth -= 1;
        }
    }

    pub fn add_error(&mut self, error: ParseError) {
        self.errors.add(error);
    }

    pub fn add_lex_error(&mut self, error: LexError) {
        self.errors.add(error.into());
    }

    pub fn errors(&self) -> &ParseErrors {
        &self.errors
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty() || self.lexer.has_errors()
    }

    pub fn take_errors(&mut self) -> ParseErrors {
        for lex_error in self.lexer.take_errors() {
            self.errors.add(lex_error.into());
        }
        std::mem::take(&mut self.errors)
    }

    pub fn current_position(&self) -> Position {
        self.lexer.current_position()
    }

    pub fn current_span(&self) -> Span {
        let pos = self.current_position();
        Span::new(pos, pos)
    }

    pub fn merge_span(&self, start: Position, end: Position) -> Span {
        Span::new(start, end)
    }

    /// Span from `start` (usually another span's `.start`) up to the current
    /// cursor position. Replaces the `end_span = ctx.current_span()` /
    /// `merge_span(start, end_span.end)` idiom spread across the parsers.
    pub fn span_since(&self, start: Position) -> Span {
        let end = self.current_span();
        Span::new(start, end.end)
    }

    pub fn current_token(&self) -> &Token {
        &self.current_token
    }

    pub fn next_token(&mut self) {
        if let Some(tok) = self.next_token.take() {
            self.current_token = tok;
        } else {
            self.current_token = self.lexer.next_token();
        }
    }

    /// Return the next token WITHOUT consuming it.
    ///
    /// The lookahead is buffered in the context, so the token returned here
    /// is the one that the next [`ParseContext::next_token`] (or
    /// `match_token` / `expect_token`) call will consume.
    pub fn peek_token(&mut self) -> &Token {
        if self.next_token.is_none() {
            self.next_token = Some(self.lexer.next_token());
        }
        self.next_token
            .as_ref()
            .expect("lookahead buffer should be filled")
    }

    pub fn match_token(&mut self, expected: TokenKind) -> bool {
        if self.current_token.kind == expected {
            self.next_token();
            true
        } else {
            false
        }
    }

    pub fn check_token(&self, expected: TokenKind) -> bool {
        self.current_token.kind == expected
    }

    pub fn expect_token(&mut self, expected: TokenKind) -> Result<(), ParseError> {
        if self.current_token.kind == expected {
            self.next_token();
            Ok(())
        } else {
            let pos = self.current_position();
            Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!(
                    "Expected {:?}, found {:?}",
                    expected, self.current_token.kind
                ),
                pos,
            )
            .with_expected_tokens(vec![format!("{:?}", expected)]))
        }
    }

    pub fn is_recovery_exhausted(&self) -> bool {
        self.recovery_count >= RECOVERY_LIMIT
    }

    pub fn record_recovery(&mut self) {
        self.recovery_count += 1;
    }

    pub fn reset_recovery_count(&mut self) {
        self.recovery_count = 0;
    }

    pub fn synchronize(&mut self, scope: RecoveryScope) {
        if self.is_recovery_exhausted() {
            return;
        }

        let sync_tokens: &[TokenKind] = match scope {
            RecoveryScope::Statement => STATEMENT_SYNC_TOKENS,
            RecoveryScope::Clause => CLAUSE_SYNC_TOKENS,
            RecoveryScope::Expression => EXPR_SYNC_TOKENS,
        };

        while self.current_token.kind != TokenKind::Eof {
            if sync_tokens.contains(&self.current_token.kind) {
                return;
            }
            self.next_token();
        }
    }

    pub fn try_recover(
        &mut self,
        error: ParseError,
        scope: RecoveryScope,
    ) -> Result<(), ParseError> {
        if self.is_recovery_exhausted() {
            return Err(error);
        }

        self.add_error(error.mark_recovered());
        self.record_recovery();
        self.synchronize(scope);
        Ok(())
    }

    /// Parse a clause body with clause-level error recovery.
    ///
    /// On failure the error is recorded and the parser synchronizes to the
    /// next clause boundary keyword, so the remaining clauses of the
    /// enclosing statement are still parsed. `fallback` must produce a
    /// structurally valid placeholder for the failed clause; the query is
    /// still rejected once the collected errors are inspected by the caller.
    /// Returns the original error, unrecorded, when the recovery limit is
    /// reached.
    pub fn recover_clause<T>(
        &mut self,
        fallback: impl FnOnce(&mut ParseContext) -> Result<T, ParseError>,
        parse: impl FnOnce(&mut ParseContext) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        match parse(self) {
            Ok(value) => Ok(value),
            Err(error) => match self.try_recover(error, RecoveryScope::Clause) {
                Ok(()) => fallback(self),
                Err(error) => Err(error),
            },
        }
    }

    pub fn consume_value(&mut self) -> Result<linkrs_core::Value, ParseError> {
        match &self.current_token.kind {
            TokenKind::StringLiteral(s) => {
                let s = s.clone();
                self.next_token();
                Ok(linkrs_core::Value::string(s))
            }
            TokenKind::IntegerLiteral(n) => {
                let n = *n;
                self.next_token();
                Ok(linkrs_core::Value::BigInt(n))
            }
            TokenKind::FloatLiteral(f) => {
                let f = *f;
                self.next_token();
                Ok(linkrs_core::Value::Double(f))
            }
            TokenKind::BooleanLiteral(b) => {
                let b = *b;
                self.next_token();
                Ok(linkrs_core::Value::Bool(b))
            }
            TokenKind::Null => {
                self.next_token();
                Ok(linkrs_core::Value::Null(linkrs_core::null::NullType::Null))
            }
            _ => {
                let pos = self.current_position();
                Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken,
                    format!("Expected value, found {:?}", self.current_token.kind),
                    pos,
                ))
            }
        }
    }
}
