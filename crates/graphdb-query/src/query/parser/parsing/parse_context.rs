use std::sync::Arc;

use crate::core::types::{Position, Span};
use crate::query::parser::core::error::{ParseError, ParseErrorKind};
use crate::query::parser::lexing::LexError;
use crate::query::parser::lexing::Lexer;
use crate::query::parser::ParseErrors;
use crate::query::parser::Token;
use crate::query::parser::TokenKind;
use crate::query::validator::context::ExpressionAnalysisContext;

pub struct ParseContext<'a> {
    lexer: Lexer<'a>,
    current_token: Token,
    errors: ParseErrors,
    compat_mode: bool,
    upsert_mode: bool,
    recursion_depth: usize,
    max_recursion_depth: usize,
    expr_context: Arc<ExpressionAnalysisContext>,
}

impl<'a> ParseContext<'a> {
    pub fn new(input: &'a str) -> Self {
        let lexer = Lexer::new(input);
        let current_token = lexer.current_token().clone();
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        Self {
            lexer,
            current_token,
            errors: ParseErrors::new(),
            compat_mode: false,
            upsert_mode: false,
            recursion_depth: 0,
            max_recursion_depth: 100,
            expr_context,
        }
    }

    pub fn from_string(input: String) -> Self {
        let lexer = Lexer::from_string(input);
        let current_token = lexer.current_token().clone();
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        Self {
            lexer,
            current_token,
            errors: ParseErrors::new(),
            compat_mode: false,
            upsert_mode: false,
            recursion_depth: 0,
            max_recursion_depth: 100,
            expr_context,
        }
    }

    pub fn set_expression_context(&mut self, expr_context: Arc<ExpressionAnalysisContext>) {
        self.expr_context = expr_context;
    }

    pub fn expression_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expr_context
    }

    pub fn expression_context_clone(&self) -> Arc<ExpressionAnalysisContext> {
        self.expr_context.clone()
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

    pub fn current_token(&self) -> &Token {
        &self.current_token
    }

    pub fn next_token(&mut self) {
        self.current_token = self.lexer.next_token();
    }

    pub fn peek_token(&self) -> &Token {
        &self.current_token
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
            ))
        }
    }

    pub fn expect_identifier(&mut self) -> Result<String, ParseError> {
        match &self.current_token.kind {
            TokenKind::Identifier(s) => {
                let id = s.clone();
                self.next_token();
                Ok(id)
            }
            // Allow certain keywords to be used as identifiers.
            TokenKind::Count => {
                self.next_token();
                Ok("count".to_string())
            }
            TokenKind::Sum => {
                self.next_token();
                Ok("sum".to_string())
            }
            TokenKind::Avg => {
                self.next_token();
                Ok("avg".to_string())
            }
            TokenKind::Min => {
                self.next_token();
                Ok("min".to_string())
            }
            TokenKind::Max => {
                self.next_token();
                Ok("max".to_string())
            }
            TokenKind::Weight => {
                self.next_token();
                Ok("weight".to_string())
            }
            TokenKind::User => {
                self.next_token();
                Ok("User".to_string())
            }
            TokenKind::Order => {
                self.next_token();
                Ok("Order".to_string())
            }
            TokenKind::Status => {
                self.next_token();
                Ok("status".to_string())
            }
            TokenKind::Contains => {
                self.next_token();
                Ok("CONTAINS".to_string())
            }
            TokenKind::Tags => {
                self.next_token();
                Ok("tags".to_string())
            }
            TokenKind::Tag => {
                self.next_token();
                Ok("tag".to_string())
            }
            TokenKind::Path => {
                self.next_token();
                Ok("path".to_string())
            }
            TokenKind::Vertex => {
                self.next_token();
                Ok("vertex".to_string())
            }
            TokenKind::KeywordVector => {
                self.next_token();
                Ok("VECTOR".to_string())
            }
            TokenKind::Vertices => {
                self.next_token();
                Ok("vertices".to_string())
            }
            TokenKind::Edges => {
                self.next_token();
                Ok("edges".to_string())
            }
            TokenKind::Admin
            | TokenKind::AdminRole
            | TokenKind::God
            | TokenKind::Dba
            | TokenKind::Guest => {
                let s = self.current_token.lexeme.clone();
                self.next_token();
                Ok(s)
            }
            _ => {
                let pos = self.current_position();
                Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken,
                    format!("Expected identifier, found {:?}", self.current_token.kind),
                    pos,
                ))
            }
        }
    }

    pub fn expect_string_literal(&mut self) -> Result<String, ParseError> {
        match &self.current_token.kind {
            TokenKind::StringLiteral(s) => {
                let s = s.clone();
                self.next_token();
                Ok(s)
            }
            _ => {
                let pos = self.current_position();
                Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken,
                    format!(
                        "Expected string literal, found {:?}",
                        self.current_token.kind
                    ),
                    pos,
                ))
            }
        }
    }

    pub fn expect_integer_literal(&mut self) -> Result<i64, ParseError> {
        match &self.current_token.kind {
            TokenKind::IntegerLiteral(n) => {
                let n = *n;
                self.next_token();
                Ok(n)
            }
            _ => {
                let pos = self.current_position();
                Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken,
                    format!(
                        "Expected integer literal, found {:?}",
                        self.current_token.kind
                    ),
                    pos,
                ))
            }
        }
    }

    pub fn expect_float_literal(&mut self) -> Result<f64, ParseError> {
        match &self.current_token.kind {
            TokenKind::FloatLiteral(f) => {
                let f = *f;
                self.next_token();
                Ok(f)
            }
            _ => {
                let pos = self.current_position();
                Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken,
                    format!(
                        "Expected float literal, found {:?}",
                        self.current_token.kind
                    ),
                    pos,
                ))
            }
        }
    }

    pub fn is_identifier_or_in_token(&self) -> bool {
        matches!(
            self.current_token.kind,
            TokenKind::Identifier(_) | TokenKind::In
        )
    }

    pub fn is_identifier_token(&self) -> bool {
        matches!(
            self.current_token.kind,
            TokenKind::Identifier(_)
                | TokenKind::Count
                | TokenKind::Sum
                | TokenKind::Avg
                | TokenKind::Min
                | TokenKind::Max
                | TokenKind::Weight
                | TokenKind::User
                | TokenKind::Order
                | TokenKind::Status
                | TokenKind::Contains
                | TokenKind::Tag
                | TokenKind::Tags
                | TokenKind::Path
                | TokenKind::Vertex
                | TokenKind::Vertices
                | TokenKind::Edges
        )
    }

    pub fn check_keyword(&mut self, keyword: &str) -> bool {
        match &self.current_token.kind {
            TokenKind::Identifier(kw) => kw.eq_ignore_ascii_case(keyword),
            TokenKind::Index => keyword.eq_ignore_ascii_case("INDEX"),
            TokenKind::On => keyword.eq_ignore_ascii_case("ON"),
            TokenKind::Drop => keyword.eq_ignore_ascii_case("DROP"),
            TokenKind::Create => keyword.eq_ignore_ascii_case("CREATE"),
            TokenKind::Alter => keyword.eq_ignore_ascii_case("ALTER"),
            TokenKind::Show => keyword.eq_ignore_ascii_case("SHOW"),
            TokenKind::Desc => {
                keyword.eq_ignore_ascii_case("DESC") || keyword.eq_ignore_ascii_case("DESCRIBE")
            }
            TokenKind::Search => keyword.eq_ignore_ascii_case("SEARCH"),
            TokenKind::Lookup => keyword.eq_ignore_ascii_case("LOOKUP"),
            TokenKind::Match => keyword.eq_ignore_ascii_case("MATCH"),
            TokenKind::KeywordVector => keyword.eq_ignore_ascii_case("VECTOR"),
            TokenKind::If => keyword.eq_ignore_ascii_case("IF"),
            TokenKind::Not => keyword.eq_ignore_ascii_case("NOT"),
            TokenKind::Exists => keyword.eq_ignore_ascii_case("EXISTS"),
            TokenKind::User => keyword.eq_ignore_ascii_case("USER"),
            TokenKind::Tag => keyword.eq_ignore_ascii_case("TAG"),
            TokenKind::Edge => keyword.eq_ignore_ascii_case("EDGE"),
            TokenKind::Space => keyword.eq_ignore_ascii_case("SPACE"),
            TokenKind::Insert => keyword.eq_ignore_ascii_case("INSERT"),
            TokenKind::Delete => keyword.eq_ignore_ascii_case("DELETE"),
            TokenKind::Update => keyword.eq_ignore_ascii_case("UPDATE"),
            TokenKind::Return => keyword.eq_ignore_ascii_case("RETURN"),
            TokenKind::Where => keyword.eq_ignore_ascii_case("WHERE"),
            TokenKind::Set => keyword.eq_ignore_ascii_case("SET"),
            TokenKind::Remove => keyword.eq_ignore_ascii_case("REMOVE"),
            TokenKind::Add => keyword.eq_ignore_ascii_case("ADD"),
            TokenKind::With => keyword.eq_ignore_ascii_case("WITH"),
            TokenKind::Yield => keyword.eq_ignore_ascii_case("YIELD"),
            TokenKind::Go => keyword.eq_ignore_ascii_case("GO"),
            TokenKind::Over => keyword.eq_ignore_ascii_case("OVER"),
            TokenKind::Step => keyword.eq_ignore_ascii_case("STEP"),
            TokenKind::Upto => keyword.eq_ignore_ascii_case("UPTO"),
            TokenKind::Limit => keyword.eq_ignore_ascii_case("LIMIT"),
            TokenKind::Asc => keyword.eq_ignore_ascii_case("ASC"),
            TokenKind::Order => keyword.eq_ignore_ascii_case("ORDER"),
            TokenKind::By => keyword.eq_ignore_ascii_case("BY"),
            TokenKind::Skip => keyword.eq_ignore_ascii_case("SKIP"),
            TokenKind::Unwind => keyword.eq_ignore_ascii_case("UNWIND"),
            TokenKind::Optional => keyword.eq_ignore_ascii_case("OPTIONAL"),
            TokenKind::Distinct => keyword.eq_ignore_ascii_case("DISTINCT"),
            TokenKind::All => keyword.eq_ignore_ascii_case("ALL"),
            TokenKind::Null => keyword.eq_ignore_ascii_case("NULL"),
            TokenKind::Is => keyword.eq_ignore_ascii_case("IS"),
            TokenKind::And => keyword.eq_ignore_ascii_case("AND"),
            TokenKind::Or => keyword.eq_ignore_ascii_case("OR"),
            TokenKind::Xor => keyword.eq_ignore_ascii_case("XOR"),
            TokenKind::Contains => keyword.eq_ignore_ascii_case("CONTAINS"),
            TokenKind::StartsWith => keyword.eq_ignore_ascii_case("STARTS WITH"),
            TokenKind::EndsWith => keyword.eq_ignore_ascii_case("ENDS WITH"),
            TokenKind::Case => keyword.eq_ignore_ascii_case("CASE"),
            TokenKind::When => keyword.eq_ignore_ascii_case("WHEN"),
            TokenKind::Then => keyword.eq_ignore_ascii_case("THEN"),
            TokenKind::Else => keyword.eq_ignore_ascii_case("ELSE"),
            TokenKind::End => keyword.eq_ignore_ascii_case("END"),
            TokenKind::Union => keyword.eq_ignore_ascii_case("UNION"),
            TokenKind::Intersect => keyword.eq_ignore_ascii_case("INTERSECT"),
            TokenKind::SetMinus => keyword.eq_ignore_ascii_case("SET MINUS"),
            TokenKind::Group => keyword.eq_ignore_ascii_case("GROUP"),
            TokenKind::Having => keyword.eq_ignore_ascii_case("HAVING"),
            TokenKind::Between => keyword.eq_ignore_ascii_case("BETWEEN"),
            TokenKind::Admin => keyword.eq_ignore_ascii_case("ADMIN"),
            TokenKind::Edges => keyword.eq_ignore_ascii_case("EDGES"),
            TokenKind::Vertex => keyword.eq_ignore_ascii_case("VERTEX"),
            TokenKind::Vertices => keyword.eq_ignore_ascii_case("VERTICES"),
            TokenKind::Tags => keyword.eq_ignore_ascii_case("TAGS"),
            TokenKind::Indexes => keyword.eq_ignore_ascii_case("INDEXES"),
            TokenKind::Find => keyword.eq_ignore_ascii_case("FIND"),
            TokenKind::Path => keyword.eq_ignore_ascii_case("PATH"),
            TokenKind::From => keyword.eq_ignore_ascii_case("FROM"),
            TokenKind::To => keyword.eq_ignore_ascii_case("TO"),
            TokenKind::As => keyword.eq_ignore_ascii_case("AS"),
            TokenKind::Upsert => keyword.eq_ignore_ascii_case("UPSERT"),
            _ => false,
        }
    }

    /// Check if the next tokens match the given sequence (non-consuming)
    pub fn check_keyword_sequence(&mut self, keywords: &[&str]) -> bool {
        if keywords.is_empty() {
            return false;
        }

        // Save lexer state
        let saved_lexer = self.lexer.clone();
        let saved_token = self.current_token.clone();

        // Check each keyword in sequence
        for (i, &keyword) in keywords.iter().enumerate() {
            if i > 0 {
                self.next_token();
            }
            if !self.check_keyword(keyword) {
                // Restore lexer state
                self.lexer = saved_lexer;
                self.current_token = saved_token;
                return false;
            }
        }

        // Restore lexer state
        self.lexer = saved_lexer;
        self.current_token = saved_token;
        true
    }

    pub fn consume_keyword(&mut self, keyword: &str) -> Result<(), ParseError> {
        if self.check_keyword(keyword) {
            self.next_token();
            Ok(())
        } else {
            let pos = self.current_position();
            Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!(
                    "Expected keyword '{}', found {:?}",
                    keyword, self.current_token.kind
                ),
                pos,
            ))
        }
    }

    pub fn consume_identifier(&mut self) -> Result<String, ParseError> {
        self.expect_identifier()
    }

    pub fn consume_string(&mut self) -> Result<String, ParseError> {
        self.expect_string_literal()
    }

    pub fn consume_float(&mut self) -> Result<f64, ParseError> {
        self.expect_float_literal()
    }

    pub fn consume_int(&mut self) -> Result<i64, ParseError> {
        self.expect_integer_literal()
    }

    pub fn consume_token(&mut self, token: &str) -> Result<(), ParseError> {
        match token {
            "(" => self.expect_token(TokenKind::LParen),
            ")" => self.expect_token(TokenKind::RParen),
            "," => self.expect_token(TokenKind::Comma),
            ";" => self.expect_token(TokenKind::Semicolon),
            ":" => self.expect_token(TokenKind::Colon),
            "+" => self.expect_token(TokenKind::Plus),
            "-" => self.expect_token(TokenKind::Minus),
            "*" => self.expect_token(TokenKind::Star),
            "/" => self.expect_token(TokenKind::Div),
            "=" => self.expect_token(TokenKind::Eq),
            "!=" => self.expect_token(TokenKind::Ne),
            "<" => self.expect_token(TokenKind::Lt),
            "<=" => self.expect_token(TokenKind::Le),
            ">" => self.expect_token(TokenKind::Gt),
            ">=" => self.expect_token(TokenKind::Ge),
            _ => {
                let pos = self.current_position();
                Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken,
                    format!("Unsupported token: {}", token),
                    pos,
                ))
            }
        }
    }

    pub fn consume_optional_token(&mut self, token: &str) -> bool {
        match token {
            "(" => self.match_token(TokenKind::LParen),
            ")" => self.match_token(TokenKind::RParen),
            "," => self.match_token(TokenKind::Comma),
            ";" => self.match_token(TokenKind::Semicolon),
            ":" => self.match_token(TokenKind::Colon),
            "+" => self.match_token(TokenKind::Plus),
            "-" => self.match_token(TokenKind::Minus),
            "*" => self.match_token(TokenKind::Star),
            "/" => self.match_token(TokenKind::Div),
            "=" => self.match_token(TokenKind::Eq),
            "!=" => self.match_token(TokenKind::Ne),
            "<" => self.match_token(TokenKind::Lt),
            "<=" => self.match_token(TokenKind::Le),
            ">" => self.match_token(TokenKind::Gt),
            ">=" => self.match_token(TokenKind::Ge),
            _ => false,
        }
    }

    pub fn try_consume_string(&mut self) -> Option<String> {
        if let TokenKind::StringLiteral(s) = &self.current_token.kind {
            let s = s.clone();
            self.next_token();
            Some(s)
        } else {
            None
        }
    }

    pub fn try_consume_quoted_string(&mut self) -> Option<String> {
        self.try_consume_string()
    }

    pub fn consume_value(&mut self) -> Result<crate::core::Value, ParseError> {
        match &self.current_token.kind {
            TokenKind::StringLiteral(s) => {
                let s = s.clone();
                self.next_token();
                Ok(crate::core::Value::String(s))
            }
            TokenKind::IntegerLiteral(n) => {
                let n = *n;
                self.next_token();
                Ok(crate::core::Value::BigInt(n))
            }
            TokenKind::FloatLiteral(f) => {
                let f = *f;
                self.next_token();
                Ok(crate::core::Value::Double(f))
            }
            TokenKind::BooleanLiteral(b) => {
                let b = *b;
                self.next_token();
                Ok(crate::core::Value::Bool(b))
            }
            TokenKind::Null => {
                self.next_token();
                Ok(crate::core::Value::Null(crate::core::null::NullType::Null))
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
