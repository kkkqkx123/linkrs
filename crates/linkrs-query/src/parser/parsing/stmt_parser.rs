//! Sentence Parsing Module
//!
//! Responsible for parsing various statements, including MATCH, GO, CREATE, DELETE, UPDATE, etc.
//! This module serves as an entry point; it delegates the specific analysis logic to the various sub-modules.

mod admin;
mod database;
mod dispatch;
mod group_by;
mod misc;
mod pipe;

#[cfg(test)]
mod tests;

use crate::parser::ast::stmt::*;
use crate::parser::core::error::ParseError;
use crate::parser::parsing::parse_context::ParseContext;

/// Statement parser - namespace for statement parsing functions.
pub struct StmtParser;

impl StmtParser {
    /// Parse statements (pipeline operators are supported).
    pub fn parse_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let stmt = Self::parse_single_statement(ctx)?;
        pipe::parse_pipe_suffix(ctx, stmt)
    }

    /// Analyzing a single statement (without distributing it through any pipelines)
    fn parse_single_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        if let Some(result) = dispatch::parse_keyword_statement(ctx) {
            return result;
        }
        dispatch::parse_token_statement(ctx)
    }
}
