//! Query parser module for graph database
//!
//! This module provides functionality to parse query strings into abstract syntax trees (AST)
//! that can be processed by query execution pipeline.

pub mod ast;
pub mod core;
pub mod lexing;
pub mod parsing;

// Re-export the common types of the core module
pub use crate::core::types::{Position, Span};
pub use core::{ParseError, ParseErrors, Token, TokenKind};

// Re-export types
pub use crate::core::types::EdgeDirection;
pub use crate::core::types::OrderDirection;

// Re-export commonly used types from ast
pub use ast::stmt::OrderByItem;

// Re-export the parser
pub use parsing::ExprParser;
pub use parsing::ParseContext;
pub use parsing::Parser;
pub use parsing::ParserResult;
pub use parsing::StmtParser;
