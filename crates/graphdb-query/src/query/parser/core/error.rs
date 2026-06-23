//! Error handling for the query parser
//!
//! This module defines error types for the parsing process,
//! providing unified error reporting with position information,
//! hints, and context support.

use crate::query::QueryError;
use std::error::Error;
use std::fmt;

use crate::core::types::Position;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    LexicalError,
    SyntaxError,
    UnexpectedToken,
    UnterminatedString,
    UnterminatedComment,
    InvalidNumber,
    InvalidEscapeSequence,
    UnicodeEscapeError,
    UnexpectedEndOfInput,
    InvalidCharacter,
    UnknownKeyword,
    RecursionLimitExceeded,
    UnsupportedFeature,
    SemanticError,
}

#[derive(Debug)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub message: Box<str>,
    pub position: Position,
    pub offset: Option<usize>,
    pub unexpected_token: Option<Box<str>>,
    pub expected_tokens: Box<[String]>,
    pub context: Option<Box<dyn Error + Send + Sync>>,
    pub hints: Box<[String]>,
}

impl ParseError {
    pub fn new(kind: ParseErrorKind, message: impl Into<Box<str>>, position: Position) -> Self {
        ParseError {
            kind,
            message: message.into(),
            position,
            offset: None,
            unexpected_token: None,
            expected_tokens: Box::new([]),
            context: None,
            hints: Box::new([]),
        }
    }

    pub fn new_simple(message: impl Into<Box<str>>, position: Position) -> Self {
        ParseError::new(ParseErrorKind::SyntaxError, message, position)
    }

    pub fn syntax_error<T: fmt::Display>(msg: T, position: Position) -> ParseError {
        ParseError::new(
            ParseErrorKind::SyntaxError,
            format!("Syntax error: {}", msg),
            position,
        )
    }

    pub fn unexpected_token<T: fmt::Display>(token: T, position: Position) -> ParseError {
        ParseError::new(
            ParseErrorKind::UnexpectedToken,
            format!("Unexpected token: {}", token),
            position,
        )
    }

    pub fn unterminated_string(position: Position) -> ParseError {
        ParseError::new(
            ParseErrorKind::UnterminatedString,
            "Unterminated string literal",
            position,
        )
    }

    pub fn unterminated_comment(position: Position) -> ParseError {
        ParseError::new(
            ParseErrorKind::UnterminatedComment,
            "Unterminated multi-line comment",
            position,
        )
    }

    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn with_unexpected_token<T: fmt::Display>(mut self, token: T) -> Self {
        self.unexpected_token = Some(token.to_string().into_boxed_str());
        self
    }

    pub fn with_expected_tokens(mut self, tokens: Vec<String>) -> Self {
        self.expected_tokens = tokens.into_boxed_slice();
        self
    }

    pub fn with_context<E: Error + Send + Sync + 'static>(mut self, context: E) -> Self {
        self.context = Some(Box::new(context));
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hints = std::iter::once(hint.into())
            .collect::<Vec<String>>()
            .into_boxed_slice();
        self
    }

    pub fn with_hints(mut self, hints: Vec<impl Into<String>>) -> Self {
        self.hints = hints
            .into_iter()
            .map(|h| h.into())
            .collect::<Vec<String>>()
            .into_boxed_slice();
        self
    }

    pub fn add_hint(&mut self, hint: impl Into<String>) {
        let mut new_hints = self
            .hints
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>();
        new_hints.push(hint.into());
        self.hints = new_hints.into_boxed_slice();
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Parse error at line {}, column {}: {}",
            self.position.line, self.position.column, self.message
        )?;

        if let Some(ref token) = self.unexpected_token {
            writeln!(f, "\n  Unexpected token: {}", token)?;
        }

        if !self.expected_tokens.is_empty() {
            writeln!(
                f,
                "\n  Expected one of: {}",
                self.expected_tokens.join(", ")
            )?;
        }

        if let Some(ref context) = self.context {
            writeln!(f, "\n  Context: {}", context)?;
        }

        if !self.hints.is_empty() {
            writeln!(f, "\n  Hint(s):")?;
            for hint in &self.hints {
                writeln!(f, "    - {}", hint)?;
            }
        }

        Ok(())
    }
}

impl Error for ParseError {}

impl From<String> for ParseError {
    fn from(message: String) -> Self {
        ParseError::new(ParseErrorKind::SyntaxError, message, Position::new(0, 0))
    }
}

impl From<super::super::lexing::LexError> for ParseError {
    fn from(lex_error: super::super::lexing::LexError) -> Self {
        let mut parse_error = ParseError::new(
            ParseErrorKind::LexicalError,
            lex_error.message,
            lex_error.position,
        );
        // Retain the offset information.
        if let Some(offset) = lex_error.offset {
            parse_error = parse_error.with_offset(offset);
        }
        parse_error
    }
}

impl From<ParseError> for QueryError {
    fn from(parse_error: ParseError) -> Self {
        use crate::core::error::query::{
            ParseErrorKind as QueryParseErrorKind, StructuredParseError,
        };
        use crate::core::types::Position;

        let kind = match parse_error.kind {
            ParseErrorKind::LexicalError => QueryParseErrorKind::LexicalError,
            ParseErrorKind::SyntaxError => QueryParseErrorKind::SyntaxError,
            ParseErrorKind::UnexpectedToken => QueryParseErrorKind::UnexpectedToken,
            ParseErrorKind::UnterminatedString => QueryParseErrorKind::UnterminatedString,
            ParseErrorKind::UnterminatedComment => QueryParseErrorKind::UnterminatedComment,
            ParseErrorKind::InvalidNumber => QueryParseErrorKind::InvalidNumber,
            ParseErrorKind::InvalidEscapeSequence => QueryParseErrorKind::InvalidEscapeSequence,
            ParseErrorKind::UnicodeEscapeError => QueryParseErrorKind::UnicodeEscapeError,
            ParseErrorKind::UnexpectedEndOfInput => QueryParseErrorKind::UnexpectedEndOfInput,
            ParseErrorKind::InvalidCharacter => QueryParseErrorKind::InvalidCharacter,
            ParseErrorKind::UnknownKeyword => QueryParseErrorKind::UnknownKeyword,
            ParseErrorKind::RecursionLimitExceeded => QueryParseErrorKind::RecursionLimitExceeded,
            ParseErrorKind::UnsupportedFeature => QueryParseErrorKind::UnsupportedFeature,
            ParseErrorKind::SemanticError => QueryParseErrorKind::SemanticError,
        };

        let structured = StructuredParseError {
            kind,
            message: parse_error.message.into(),
            position: Position::new(parse_error.position.line, parse_error.position.column),
            offset: parse_error.offset,
            unexpected_token: parse_error.unexpected_token.map(|t| t.into()),
            expected_tokens: parse_error.expected_tokens.into_vec(),
            hints: parse_error.hints.into_vec(),
            context: parse_error.context.as_ref().map(|c| c.to_string()),
        };

        QueryError::structured_parse_error(structured)
    }
}

#[derive(Debug)]
pub struct ParseErrors {
    pub errors: Vec<ParseError>,
}

impl ParseErrors {
    pub fn new() -> Self {
        ParseErrors { errors: Vec::new() }
    }

    pub fn add(&mut self, error: ParseError) {
        self.errors.push(error);
    }

    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn len(&self) -> usize {
        self.errors.len()
    }

    pub fn push(&mut self, error: ParseError) {
        self.errors.push(error);
    }

    pub fn extend(&mut self, errors: &mut ParseErrors) {
        self.errors.append(&mut errors.errors);
    }

    pub fn take(&mut self) -> Vec<ParseError> {
        std::mem::take(&mut self.errors)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ParseError> {
        self.errors.iter()
    }
}

impl IntoIterator for ParseErrors {
    type Item = ParseError;
    type IntoIter = std::vec::IntoIter<ParseError>;

    fn into_iter(self) -> Self::IntoIter {
        self.errors.into_iter()
    }
}

impl Default for ParseErrors {
    fn default() -> Self {
        ParseErrors::new()
    }
}

impl fmt::Display for ParseErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, error) in self.errors.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{}", error)?;
        }
        Ok(())
    }
}

impl Error for ParseErrors {}

impl From<Vec<ParseError>> for ParseErrors {
    fn from(errors: Vec<ParseError>) -> Self {
        ParseErrors { errors }
    }
}

impl From<ParseErrors> for QueryError {
    fn from(parse_errors: ParseErrors) -> Self {
        QueryError::parse_error(parse_errors.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_error_display() {
        let error = ParseError::unexpected_token("IDENTIFIER", Position::new(10, 5));
        let display = error.to_string();
        assert!(display.contains("line 10, column 5"));
        assert!(display.contains("Unexpected token: IDENTIFIER"));
    }

    #[test]
    fn test_parse_error_with_hint() {
        let error = ParseError::syntax_error("invalid syntax", Position::new(5, 10))
            .with_hint("Try adding a semicolon at the end".to_string());

        let display = error.to_string();
        assert!(display.contains("Hint"));
        assert!(display.contains("semicolon"));
    }

    #[test]
    fn test_parse_error_with_context() {
        let context_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let error =
            ParseError::syntax_error("error", Position::new(1, 1)).with_context(context_error);

        let display = error.to_string();
        assert!(display.contains("Context"));
    }
}
