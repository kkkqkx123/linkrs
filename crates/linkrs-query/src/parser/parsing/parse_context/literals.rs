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
            TokenKind::Detach => keyword.eq_ignore_ascii_case("DETACH"),
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
            TokenKind::Comment => keyword.eq_ignore_ascii_case("COMMENT"),
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
            TokenKind::In => keyword.eq_ignore_ascii_case("IN"),
            TokenKind::Rollback => keyword.eq_ignore_ascii_case("ROLLBACK"),
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
}
