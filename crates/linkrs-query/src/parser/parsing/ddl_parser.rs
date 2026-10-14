mod alter_parser;
mod create_parser;
mod describe_parser;
mod drop_parser;
mod helpers;

use crate::parser::core::error::ParseError;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

pub struct DdlParser;

impl DdlParser {
    pub fn new() -> Self {
        Self
    }
}

/// Consume either the `SPACE` token or the `GRAPH` keyword (namespace
/// alias shared by CREATE, USE and DROP). Returns `Ok(true)` when either
/// was matched and consumed.
pub(crate) fn consume_space_or_graph(ctx: &mut ParseContext) -> Result<bool, ParseError> {
    if ctx.match_token(TokenKind::Space) {
        return Ok(true);
    }
    if ctx.check_keyword("GRAPH") {
        ctx.consume_keyword("GRAPH")?;
        return Ok(true);
    }
    Ok(false)
}

impl Default for DdlParser {
    fn default() -> Self {
        Self::new()
    }
}
