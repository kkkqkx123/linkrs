//! Cross-cutting helpers: shared expression parsing plus the extended
//! UPDATE / CREATE dispatch and `$var =` assignment statements.

use crate::parser::TokenKind;
use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::expr_parser::parse_expression_with_context;
use crate::parser::parsing::parse_context::ParseContext;
use graphdb_core::types::expr::contextual::ContextualExpression;

/// Analyzing expressions (auxiliary method)
pub(super) fn parse_expression(ctx: &mut ParseContext) -> Result<ContextualExpression, ParseError> {
    parse_expression_with_context(ctx, ctx.expression_context_clone())
}

/// Analysis of the extended UPDATE statement (including UPDATE CONFIGS)
pub(super) fn parse_update_statement_extended(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    use crate::parser::parsing::dml_parser::DmlParser;

    let start_span = ctx.current_span();
    ctx.expect_token(TokenKind::Update)?;

    if ctx.check_token(TokenKind::Configs) {
        ctx.expect_token(TokenKind::Configs)?;

        let first_ident = ctx.expect_identifier()?;

        let (module, config_name) = if ctx.check_token(TokenKind::Assign) {
            (None, first_ident)
        } else {
            (Some(first_ident), ctx.expect_identifier()?)
        };

        ctx.expect_token(TokenKind::Assign)?;
        let config_value = parse_expression(ctx)?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::UpdateConfigs(UpdateConfigsStmt {
            span,
            module,
            config_name,
            config_value,
        }))
    } else {
        DmlParser::new().parse_update_after_token(ctx, start_span)
    }
}

/// Analysis of the variable assignment statement ($var = statement)
pub(super) fn parse_assignment_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let start_span = ctx.current_span();
    ctx.expect_token(TokenKind::Dollar)?;

    let var_name = ctx.expect_identifier()?;

    ctx.expect_token(TokenKind::Assign)?;

    let statement = Box::new(super::StmtParser::parse_statement(ctx)?);

    let end_span = ctx.current_span();
    let span = ctx.merge_span(start_span.start, end_span.end);

    Ok(Stmt::Assignment(AssignmentStmt {
        span,
        variable: var_name,
        statement,
    }))
}

/// Analysis of the extended CREATE statement
pub(super) fn parse_create_statement_extended(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    use crate::parser::parsing::ddl_parser::DdlParser;
    use crate::parser::parsing::dml_parser::DmlParser;
    use crate::parser::parsing::user_parser::UserParser;

    let start_span = ctx.current_span();
    ctx.expect_token(TokenKind::Create)?;

    if ctx.check_token(TokenKind::LParen) {
        return DmlParser::new().parse_create_data_after_token(ctx, start_span);
    }

    if ctx.check_token(TokenKind::User) {
        return UserParser::new().parse_create_user_statement_after_create(ctx, start_span);
    }

    if ctx.check_keyword("FULLTEXT") {
        return crate::parser::parsing::fulltext_parser::parse_create_fulltext_index_after_create(
            ctx,
        );
    }

    if ctx.check_keyword("VECTOR") {
        return crate::parser::parsing::vector_parser::parse_create_vector_index_after_create(ctx);
    }

    if ctx.check_token(TokenKind::Tag)
        || ctx.check_token(TokenKind::Edge)
        || ctx.check_token(TokenKind::Space)
        || ctx.check_keyword("GRAPH")
        || ctx.check_token(TokenKind::Index)
        || ctx.check_token(TokenKind::Sequence)
        || ctx.check_keyword("MACRO")
        || ctx.check_keyword("TYPE")
    {
        return DdlParser::new().parse_create_after_token(ctx, start_span);
    }

    Err(ParseError::new(
        ParseErrorKind::SyntaxError,
        "CREATE statement expects '(' (Cypher data creation) or TAG/EDGE/SPACE/INDEX/MACRO/TYPE (Schema definition) or USER (user management)".to_string(),
        ctx.current_position(),
    ))
}

/// Parse full-text search statements
pub(super) fn parse_fulltext_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    crate::parser::parsing::fulltext_parser::parse_fulltext(ctx)
}
