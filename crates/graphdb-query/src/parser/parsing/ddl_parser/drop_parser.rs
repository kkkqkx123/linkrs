use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

use super::DdlParser;

impl DdlParser {
    pub fn parse_drop_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Drop)?;

        if ctx.check_keyword("FULLTEXT") {
            return crate::parser::parsing::fulltext_parser::parse_drop_fulltext_index_after_drop(
                ctx,
            );
        }

        if ctx.check_keyword("VECTOR") {
            return crate::parser::parsing::vector_parser::parse_drop_vector_index_after_drop(ctx);
        }

        let target = if ctx.match_token(TokenKind::Space) {
            let mut if_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Exists)?;
                if_exists = true;
            }
            let space_name = ctx.expect_dcl_name()?;
            return Ok(Stmt::Drop(DropStmt {
                span: start_span,
                target: DropTarget::Space(space_name),
                if_exists,
            }));
        } else if ctx.match_token(TokenKind::Tag) {
            if ctx.check_token(TokenKind::Index) {
                ctx.next_token();
                let index_name = ctx.expect_identifier()?;
                let space_name = if ctx.match_token(TokenKind::On) {
                    Some(ctx.expect_identifier()?)
                } else {
                    None
                };
                DropTarget::TagIndex {
                    space_name: space_name.unwrap_or_default(),
                    index_name,
                }
            } else {
                let mut if_exists = false;
                if ctx.match_token(TokenKind::If) {
                    ctx.expect_token(TokenKind::Exists)?;
                    if_exists = true;
                }
                let mut tag_names = vec![ctx.expect_identifier()?];
                while ctx.match_token(TokenKind::Comma) {
                    tag_names.push(ctx.expect_identifier()?);
                }
                return Ok(Stmt::Drop(DropStmt {
                    span: start_span,
                    target: DropTarget::Tags(tag_names),
                    if_exists,
                }));
            }
        } else if ctx.check_token(TokenKind::Edge) {
            ctx.next_token();
            if ctx.check_token(TokenKind::Index) {
                ctx.next_token();
                let index_name = ctx.expect_identifier()?;
                let space_name = if ctx.match_token(TokenKind::On) {
                    Some(ctx.expect_identifier()?)
                } else {
                    None
                };
                DropTarget::EdgeIndex {
                    space_name: space_name.unwrap_or_default(),
                    index_name,
                }
            } else {
                let mut if_exists = false;
                if ctx.match_token(TokenKind::If) {
                    ctx.expect_token(TokenKind::Exists)?;
                    if_exists = true;
                }
                let mut edge_names = vec![ctx.expect_identifier()?];
                while ctx.match_token(TokenKind::Comma) {
                    edge_names.push(ctx.expect_identifier()?);
                }
                return Ok(Stmt::Drop(DropStmt {
                    span: start_span,
                    target: DropTarget::Edges(edge_names),
                    if_exists,
                }));
            }
        } else if ctx.match_token(TokenKind::Index) {
            let index_name = ctx.expect_identifier()?;
            let space_name = if ctx.match_token(TokenKind::On) {
                Some(ctx.expect_identifier()?)
            } else {
                None
            };
            DropTarget::TagIndex {
                space_name: space_name.unwrap_or_default(),
                index_name,
            }
        } else if ctx.match_token(TokenKind::User) {
            let mut if_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Exists)?;
                if_exists = true;
            }
            let username = ctx.expect_dcl_name()?;

            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            return Ok(Stmt::DropUser(DropUserStmt {
                span,
                username,
                if_exists,
            }));
        } else {
            return Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                "Expected SPACE, TAG, EDGE, INDEX, or USER".to_string(),
                ctx.current_position(),
            ));
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Drop(DropStmt {
            span,
            target,
            if_exists: false,
        }))
    }
}
