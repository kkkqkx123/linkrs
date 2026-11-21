//! Shared prefix of `CREATE <family> INDEX` statements.
//!
//! Both index DDL families start with the same clause sequence
//! `INDEX [IF NOT EXISTS] <name> ON <schema>`; only the family keyword
//! before it and the payload after it differ. Keeping the shared part in
//! one place is what makes the two grammars agree on `IF NOT EXISTS`
//! placement and on identifier handling.

use crate::parser::core::error::ParseError;
use crate::parser::parsing::parse_context::ParseContext;
use linkrs_core::types::span::Span;

/// The clause sequence common to every `CREATE <family> INDEX` statement.
///
/// Parsed after the family keyword (`FULLTEXT` / `VECTOR`) has been
/// consumed and before the payload (field list, engine options, ...).
#[derive(Debug, Clone)]
pub(crate) struct CreateIndexHead {
    pub span: Span,
    pub if_not_exists: bool,
    pub index_name: String,
    pub schema_name: String,
}

/// Consume `INDEX [IF NOT EXISTS] <name> ON <schema>`.
pub(crate) fn parse_create_index_head(
    ctx: &mut ParseContext,
) -> Result<CreateIndexHead, ParseError> {
    let span = ctx.current_span();
    ctx.consume_keyword("INDEX")?;

    let if_not_exists = if ctx.check_keyword("IF") {
        ctx.consume_keyword("IF")?;
        ctx.consume_keyword("NOT")?;
        ctx.consume_keyword("EXISTS")?;
        true
    } else {
        false
    };

    let index_name = ctx.consume_identifier()?;
    ctx.consume_keyword("ON")?;
    let schema_name = ctx.consume_identifier()?;

    Ok(CreateIndexHead {
        span,
        if_not_exists,
        index_name,
        schema_name,
    })
}
