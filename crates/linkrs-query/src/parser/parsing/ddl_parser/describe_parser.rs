use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

use super::DdlParser;

impl DdlParser {
    pub fn parse_desc_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Desc)?;

        if ctx.check_token(TokenKind::User) {
            return self.parse_describe_user_internal(ctx, start_span);
        }

        let target = if ctx.match_token(TokenKind::Space) {
            DescTarget::Space(ctx.expect_dcl_name()?)
        } else if ctx.match_token(TokenKind::Tag) {
            let tag_name = ctx.expect_identifier()?;
            let space_name = if ctx.match_token(TokenKind::In) {
                Some(ctx.expect_identifier()?)
            } else {
                None
            };
            DescTarget::Tag {
                space_name: space_name.unwrap_or_default(),
                tag_name,
            }
        } else if ctx.match_token(TokenKind::Edge) {
            let edge_name = ctx.expect_identifier()?;
            let space_name = if ctx.match_token(TokenKind::In) {
                Some(ctx.expect_identifier()?)
            } else {
                None
            };
            DescTarget::Edge {
                space_name: space_name.unwrap_or_default(),
                edge_name,
            }
        } else {
            return Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                "Expected SPACE, TAG, EDGE, or USER".to_string(),
                ctx.current_position(),
            ));
        };

        let span = ctx.span_since(start_span.start);

        Ok(Stmt::Desc(DescStmt { span, target }))
    }

    fn parse_describe_user_internal(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        ctx.expect_token(TokenKind::User)?;

        let username = ctx.expect_dcl_name()?;

        let span = ctx.span_since(start_span.start);

        Ok(Stmt::DescribeUser(DescribeUserStmt { span, username }))
    }
}
