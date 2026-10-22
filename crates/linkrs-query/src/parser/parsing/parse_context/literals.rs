//! Literal, identifier and keyword consumption helpers of `ParseContext`.
//!
//! The token-consumption surface of the parser context is kept apart from
//! the context core (construction, checkpointing, token access, error
//! recovery). Methods of the same type may be defined across files, so the
//! public API of `ParseContext` is unchanged.

use super::ParseContext;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::TokenKind;

impl<'a> ParseContext<'a> {
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
            TokenKind::User | TokenKind::Order => {
                let s = self.current_token.lexeme.clone();
                self.next_token();
                Ok(s)
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
            | TokenKind::God
            | TokenKind::Dba
            | TokenKind::Guest
            | TokenKind::Space => {
                let s = self.current_token.lexeme.clone();
                self.next_token();
                Ok(s)
            }
            // Reserved words usable as tag/property names (e.g.
            // `CREATE TAG ... (data STRING)`, `MATCH (:transaction)`)
            // and as configuration module/key names (e.g.
            // `SHOW CONFIGS transaction`, `UPDATE CONFIGS database host`).
            TokenKind::Data | TokenKind::Transaction | TokenKind::Host => {
                let s = self.current_token.lexeme.clone();
                self.next_token();
                Ok(s)
            }
            // Access-mode keywords are only reserved after BEGIN; allow them
            // as identifiers elsewhere (e.g. a tag/property named `read`).
            TokenKind::Read | TokenKind::Only | TokenKind::Write => {
                let s = self.current_token.lexeme.clone();
                self.next_token();
                Ok(s)
            }
            // Option-keywords used as keys in LOAD/EXPORT OPTIONS (...)
            TokenKind::Format | TokenKind::Header | TokenKind::Delimiter => {
                let s = self.current_token.lexeme.clone();
                self.next_token();
                Ok(s)
            }
            // Type-name keywords are accepted as identifiers so they can be
            // used as macro/tag/property names (e.g. `CREATE MACRO double(x)`
            // or a tag literally named `int`). This mirrors the expression
            // parser's handling of type keywords as function/variable names.
            TokenKind::Bool
            | TokenKind::Int
            | TokenKind::Int8
            | TokenKind::Int16
            | TokenKind::Int32
            | TokenKind::Int64
            | TokenKind::Float
            | TokenKind::Double
            | TokenKind::String
            | TokenKind::FixedString
            | TokenKind::Timestamp
            | TokenKind::Date
            | TokenKind::Time
            | TokenKind::Datetime
            | TokenKind::Serial
            | TokenKind::Geography
            | TokenKind::List
            | TokenKind::Map
            | TokenKind::Struct
            | TokenKind::Array
            | TokenKind::UUID
            | TokenKind::Duration => {
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
                )
                .with_expected_tokens(vec!["identifier".to_string()]))
            }
        }
    }

    /// Parse a boolean literal (`true` / `false`).
    ///
    /// Unlike a fuzzy identifier comparison, any non-boolean token is reported
    /// explicitly so that `SET LOCKED = yes` fails instead of silently
    /// behaving as `false`.
    pub fn expect_boolean_literal(&mut self) -> Result<bool, ParseError> {
        match self.current_token.kind {
            TokenKind::BooleanLiteral(value) => {
                self.next_token();
                Ok(value)
            }
            _ => {
                let pos = self.current_position();
                Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken,
                    format!(
                        "Expected boolean value, found {:?}",
                        self.current_token.kind
                    ),
                    pos,
                )
                .with_expected_tokens(vec!["true".to_string(), "false".to_string()]))
            }
        }
    }

    /// Parse a DCL object name (username / space name).
    ///
    /// Accepts identifiers, keywords, numbers, string literals and hyphenated
    /// sequences (e.g. `user-name-123`, `NULL`, `12345`, `"'; DROP USER --"`).
    pub fn expect_dcl_name(&mut self) -> Result<String, ParseError> {
        let mut name = self.expect_dcl_name_part()?;
        while self.match_token(TokenKind::Minus) {
            name.push('-');
            name.push_str(&self.expect_dcl_name_part()?);
        }
        Ok(name)
    }

    fn expect_dcl_name_part(&mut self) -> Result<String, ParseError> {
        let token = self.current_token().clone();
        match &token.kind {
            TokenKind::Identifier(s) | TokenKind::StringLiteral(s) => {
                let s = s.clone();
                self.next_token();
                Ok(s)
            }
            TokenKind::IntegerLiteral(_) => {
                let lexeme = token.lexeme.clone();
                self.next_token();
                Ok(lexeme)
            }
            _ => {
                // Structural DCL keywords cannot be used as object names, so
                // that e.g. `CREATE USER WITH PASSWORD 'x'` reports an error
                // instead of treating WITH as the username.
                if matches!(
                    token.kind,
                    TokenKind::With
                        | TokenKind::Password
                        | TokenKind::Role
                        | TokenKind::To
                        | TokenKind::From
                        | TokenKind::On
                        | TokenKind::In
                        | TokenKind::If
                        | TokenKind::Exists
                        | TokenKind::Set
                        | TokenKind::Locked
                ) {
                    let pos = self.current_position();
                    return Err(ParseError::new(
                        ParseErrorKind::UnexpectedToken,
                        format!("Expected name, found {:?}", self.current_token.kind),
                        pos,
                    ));
                }
                if token.lexeme.is_empty() {
                    let pos = self.current_position();
                    Err(ParseError::new(
                        ParseErrorKind::UnexpectedToken,
                        format!("Expected name, found {:?}", self.current_token.kind),
                        pos,
                    ))
                } else {
                    let lexeme = token.lexeme.clone();
                    self.next_token();
                    Ok(lexeme)
                }
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
                )
                .with_expected_tokens(vec!["string literal".to_string()]))
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
                )
                .with_expected_tokens(vec!["integer literal".to_string()]))
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
                )
                .with_expected_tokens(vec!["float literal".to_string()]))
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
        token_matches_keyword(&self.current_token.kind, keyword)
    }

    /// [`check_keyword`] against the lookahead token (without consuming it).
    /// One-token lookahead predicates route through here so their keyword
    /// comparison shares the same derivation as the current-token check.
    pub fn check_keyword_ahead(&mut self, keyword: &str) -> bool {
        let kind = self.peek_token().kind.clone();
        token_matches_keyword(&kind, keyword)
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
        match crate::parser::lexing::spec::symbol_kind(token) {
            Some(kind) => self.expect_token(kind),
            None => {
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
        match crate::parser::lexing::spec::symbol_kind(token) {
            Some(kind) => self.match_token(kind),
            None => false,
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
}

/// Whether a token spells the given keyword: identifiers compare their
/// payload, reserved keyword tokens compare their registered spellings.
fn token_matches_keyword(kind: &TokenKind, keyword: &str) -> bool {
    match kind {
        TokenKind::Identifier(current) => current.eq_ignore_ascii_case(keyword),
        TokenKind::BooleanLiteral(_) => false,
        other => crate::parser::lexing::spec::unit_spellings_for(other).is_some_and(|spellings| {
            spellings
                .iter()
                .any(|spelling| spelling.eq_ignore_ascii_case(keyword))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::ParseContext;

    fn check_first(input: &str, keyword: &str) -> bool {
        let mut ctx = ParseContext::new(input);
        ctx.check_keyword(keyword)
    }

    #[test]
    fn derived_check_keyword_matches_spec() {
        assert!(check_first("REBUILD", "REBUILD"));
        assert!(check_first("rebuild", "REBUILD"));
        assert!(check_first("HEADER", "HEADER"));
        assert!(check_first("DELIMITER", "DELIMITER"));
        assert!(check_first("CSV", "CSV"));
        assert!(check_first("NO", "NO"));
        assert!(check_first("STEP", "STEPS"));
        assert!(check_first("STEPS", "STEP"));
        assert!(check_first("DESCRIBE", "DESC"));
        assert!(check_first("DESC", "DESCRIBE"));
    }

    #[test]
    fn derived_check_keyword_keeps_payload_rules() {
        assert!(!check_first("TRUE", "TRUE"));
        assert!(!check_first("FALSE", "FALSE"));
        assert!(check_first("BALANCE", "BALANCE"));
        assert!(!check_first("BALANCE", "OTHER"));
        assert!(check_first("ANALYZER", "ANALYZER"));
        assert!(!check_first("ANALYZER", "ENGINE"));
        assert!(!check_first("MATCH", "RETURN"));
    }

    fn check_ahead(input: &str, keyword: &str) -> bool {
        let mut ctx = ParseContext::new(input);
        // The cursor already rests on the first token, so the probe inspects
        // the second one.
        ctx.check_keyword_ahead(keyword)
    }

    #[test]
    fn check_keyword_ahead_matches_identifier_payload() {
        assert!(check_ahead("( SELECT", "SELECT"));
        assert!(check_ahead("( select", "SELECT"));
        assert!(!check_ahead("( SELECT", "MATCH"));
        assert!(!check_ahead("( 42", "SELECT"));
    }
}
