//! Shared helpers for pattern parsing (traversal MATCH and CREATE DML).
//!
//! Both parsers spell the same edge-hop grammar (`<-[...]->`, `-[...]-`),
//! so the direction derivation and the qualified-reference rejection live
//! here instead of being duplicated with per-parser drift.

use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::core::token::TokenKind;
use crate::parser::parsing::parse_context::ParseContext;
use linkrs_core::EdgeDirection;

/// Direction of an edge hop from its boundary tokens.
///
/// `incoming` records whether the hop opened with `<-`; `closing` is the
/// token that ended the hop (`->` or `-`, the only valid closings).
/// `None` means the closing token is not a valid hop terminator and the
/// caller must report a syntax error.
pub fn resolve_edge_direction(incoming: bool, closing: &TokenKind) -> Option<EdgeDirection> {
    let outgoing_close = matches!(closing, TokenKind::Arrow);
    let undirected_close = matches!(closing, TokenKind::Minus);
    match (incoming, outgoing_close, undirected_close) {
        (false, true, _) => Some(EdgeDirection::Out),
        (false, false, true) => Some(EdgeDirection::Both),
        (true, true, _) => Some(EdgeDirection::Both),
        (true, false, true) => Some(EdgeDirection::In),
        _ => None,
    }
}

/// Reject `alias.table` in label / edge-type position.
///
/// Call after consuming an identifier that names a label or edge type: if a
/// `Dot` follows, the qualified form is consumed and reported with the
/// attached-database hint message.
pub fn reject_qualified_reference(ctx: &mut ParseContext, name: &str) -> Result<(), ParseError> {
    if ctx.match_token(TokenKind::Dot) {
        let table = ctx.expect_identifier()?;
        let pos = ctx.current_position();
        return Err(ParseError::new(
            ParseErrorKind::UnexpectedToken,
            crate::attached::qualified_reference_message(name, &table),
            pos,
        ));
    }
    Ok(())
}
