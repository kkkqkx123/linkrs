//! Lexer implementation for the query parser
//!
//! This module implements a lexical analyzer that converts input query strings into tokens.
//!
//! ## Module Structure
//!
//! - `scanner` - character-level scanning (identifiers, numbers, strings, comments)
//! - `token` - token generation and keyword lookup

use crate::parser::lexing::LexError;
use crate::parser::{Token, TokenKind as Tk};
use linkrs_core::types::Position;
use std::iter::Peekable;

mod scanner;
mod token;

#[derive(Clone)]
pub struct Lexer<'a> {
    input: std::borrow::Cow<'a, str>,
    chars: Peekable<std::vec::IntoIter<char>>,
    position: usize,
    line: usize,
    column: usize,
    current_token: Token,
    errors: Vec<LexError>,
}

pub struct LexerCheckpoint {
    position: usize,
    line: usize,
    column: usize,
    current_token: Token,
    chars: Vec<char>,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        let chars: Vec<char> = input.chars().collect();
        let mut lexer = Lexer {
            input: std::borrow::Cow::Borrowed(input),
            chars: chars.into_iter().peekable(),
            position: 0,
            line: 1,
            column: 0,
            current_token: Token::new(Tk::Eof, String::new(), 0, 0),
            errors: Vec::new(),
        };
        lexer.current_token = lexer.next_token();
        lexer
    }

    pub fn from_string(input: String) -> Self {
        let chars: Vec<char> = input.chars().collect();
        let mut lexer = Lexer {
            input: std::borrow::Cow::Owned(input),
            chars: chars.into_iter().peekable(),
            position: 0,
            line: 1,
            column: 0,
            current_token: Token::new(Tk::Eof, String::new(), 0, 0),
            errors: Vec::new(),
        };
        lexer.read_char();
        lexer.current_token = lexer.next_token();
        lexer
    }

    fn read_char(&mut self) -> Option<char> {
        let ch = self.chars.next();
        if let Some(c) = ch {
            self.position += c.len_utf8();
            if c == '\n' {
                self.line += 1;
                self.column = 0;
            } else {
                self.column += 1;
            }
        }
        ch
    }

    fn peek_char(&mut self) -> Option<&char> {
        self.chars.peek()
    }

    fn add_error(&mut self, error: LexError) {
        self.errors.push(error);
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn take_errors(&mut self) -> Vec<LexError> {
        std::mem::take(&mut self.errors)
    }

    pub fn errors(&self) -> &[LexError] {
        &self.errors
    }

    pub fn checkpoint(&self) -> LexerCheckpoint {
        LexerCheckpoint {
            position: self.position,
            line: self.line,
            column: self.column,
            current_token: self.current_token.clone(),
            chars: self.chars.clone().collect(),
        }
    }

    pub fn restore(&mut self, ckpt: LexerCheckpoint) {
        self.position = ckpt.position;
        self.line = ckpt.line;
        self.column = ckpt.column;
        self.current_token = ckpt.current_token;
        self.chars = ckpt.chars.into_iter().peekable();
    }

    pub fn current_token(&self) -> &Token {
        &self.current_token
    }

    pub fn current_position(&self) -> Position {
        Position::new(self.line, self.column)
    }

    pub fn is_at_end(&mut self) -> bool {
        self.chars.peek().is_none()
    }

    pub fn peek(&mut self) -> Result<Token, String> {
        Ok(self.current_token.clone())
    }

    pub fn advance(&mut self) {
        self.current_token = self.next_token();
    }

    pub fn check(&mut self, kind: Tk) -> bool {
        self.current_token.kind == kind
    }
}

#[cfg(test)]
mod tests;
