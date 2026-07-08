//! Core types for the query parser
//!
//! This module provides the fundamental types used throughout
//! the parser including tokens, errors, positions, and parsing context.

pub mod error;
pub mod token;

pub use error::{ParseError, ParseErrorKind, ParseErrors};
pub use token::{Token, TokenKind, TokenKindExt};
