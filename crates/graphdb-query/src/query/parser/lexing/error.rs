pub use super::lexer::Lexer;
pub use crate::query::parser::{Token, TokenKind};

use std::fmt;

use crate::core::types::Position;

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub message: Box<str>,
    pub position: Position,
    pub offset: Option<usize>,
}

impl LexError {
    pub fn new(message: impl Into<Box<str>>, position: Position) -> Self {
        LexError {
            message: message.into(),
            position,
            offset: None,
        }
    }

    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn unterminated_string(position: Position) -> Self {
        LexError::new("Unterminated string literal", position)
    }

    pub fn unterminated_comment(position: Position) -> Self {
        LexError::new("Unterminated multi-line comment", position)
    }

    pub fn invalid_number<T: fmt::Display>(message: T, position: Position) -> Self {
        LexError::new(format!("Invalid number: {}", message), position)
    }

    pub fn invalid_escape_sequence<T: fmt::Display>(sequence: T, position: Position) -> Self {
        LexError::new(format!("Invalid escape sequence: \\{}", sequence), position)
    }

    pub fn unexpected_character(ch: char, position: Position) -> Self {
        LexError::new(format!("Unexpected character: '{}'", ch), position)
    }

    pub fn unexpected_end_of_input(position: Position) -> Self {
        LexError::new("Unexpected end of input", position)
    }
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Lex error at line {}, column {}: {}",
            self.position.line, self.position.column, self.message
        )
    }
}

impl std::error::Error for LexError {}
