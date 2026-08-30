//! Explain / Profile / Analyze Statement Parsing Module
//!
//! Responsible for parsing EXPLAIN, PROFILE, and ANALYZE statements.

use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::parsing::stmt_parser::StmtParser;
use crate::parser::TokenKind;

/// Explain / Profile / Analyze Parser
pub struct ExplainParser;

impl ExplainParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse EXPLAIN statement (contains a sub-statement)
    pub fn parse_explain_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Explain)?;

        let analyze = ctx.match_token(TokenKind::Analyze);

        let format = if ctx.match_token(TokenKind::Format) {
            ctx.expect_token(TokenKind::Assign)?;
            let format_name = ctx.expect_identifier()?;
            match format_name.to_uppercase().as_str() {
                "DOT" => ExplainFormat::Dot,
                "TABLE" => ExplainFormat::Table,
                _ => {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        format!(
                            "Unknown EXPLAIN format: {}, expects DOT or TABLE",
                            format_name
                        ),
                        ctx.current_position(),
                    ));
                }
            }
        } else {
            ExplainFormat::default()
        };

        let statement = Box::new(StmtParser::parse_statement(ctx)?);

        Ok(Stmt::Explain(ExplainStmt {
            span: start_span,
            statement,
            format,
            analyze,
        }))
    }

    /// Parse PROFILE statement
    pub fn parse_profile_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Profile)?;

        let format = if ctx.match_token(TokenKind::Format) {
            ctx.expect_token(TokenKind::Assign)?;
            let format_name = ctx.expect_identifier()?;
            match format_name.to_uppercase().as_str() {
                "DOT" => ExplainFormat::Dot,
                "TABLE" => ExplainFormat::Table,
                _ => {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        format!(
                            "Unknown PROFILE format: {}, expects DOT or TABLE",
                            format_name
                        ),
                        ctx.current_position(),
                    ));
                }
            }
        } else {
            ExplainFormat::default()
        };

        let statement = Box::new(StmtParser::parse_statement(ctx)?);

        Ok(Stmt::Profile(ProfileStmt {
            span: start_span,
            statement,
            format,
        }))
    }

    /// Parse ANALYZE statement: `ANALYZE` or `ANALYZE SPACE <name>`.
    pub fn parse_analyze_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Analyze)?;

        let space = if ctx.match_token(TokenKind::Space) {
            Some(ctx.expect_identifier()?)
        } else {
            None
        };

        Ok(Stmt::Analyze(AnalyzeStmt {
            span: start_span,
            space,
        }))
    }
}
