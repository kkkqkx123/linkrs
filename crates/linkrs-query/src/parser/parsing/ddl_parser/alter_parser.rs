use crate::parser::ast::stmt::*;

use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

use super::DdlParser;

type AlterOpsResult = (
    Vec<linkrs_core::types::PropertyDef>,
    Vec<String>,
    Vec<PropertyChange>,
    Option<String>,
);

impl DdlParser {
    pub fn parse_alter_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Alter)?;

        if ctx.check_keyword("FULLTEXT") {
            return crate::parser::parsing::fulltext_parser::parse_alter_fulltext_index_after_alter(
                ctx,
            );
        }

        if ctx.check_token(TokenKind::User) {
            return crate::parser::parsing::user_parser::UserParser::new()
                .parse_alter_user_internal(ctx, start_span);
        }

        if ctx.match_token(TokenKind::Sequence) {
            let name = ctx.expect_identifier()?;
            let mut increment = None;
            let mut min_value = None;
            let mut max_value = None;
            let mut cycle = None;

            loop {
                if ctx.check_keyword("INCREMENT") && increment.is_none() {
                    ctx.next_token();
                    ctx.expect_token(TokenKind::Assign)?;
                    increment = Some(ctx.expect_integer_literal()?);
                } else if ctx.check_keyword("MINVALUE") && min_value.is_none() {
                    ctx.next_token();
                    ctx.expect_token(TokenKind::Assign)?;
                    min_value = Some(ctx.expect_integer_literal()?);
                } else if ctx.check_keyword("MAXVALUE") && max_value.is_none() {
                    ctx.next_token();
                    ctx.expect_token(TokenKind::Assign)?;
                    max_value = Some(ctx.expect_integer_literal()?);
                } else if ctx.match_token(TokenKind::Cycle) {
                    cycle = Some(true);
                } else if ctx.check_keyword("NOCYCLE") {
                    ctx.next_token();
                    cycle = Some(false);
                } else {
                    break;
                }
            }

            let span = ctx.span_since(start_span.start);
            return Ok(Stmt::Alter(AlterStmt {
                span,
                target: AlterTarget::Sequence {
                    name,
                    increment,
                    min_value,
                    max_value,
                    cycle,
                },
            }));
        }

        let (is_tag, name) = if ctx.match_token(TokenKind::Tag) {
            (true, ctx.expect_identifier()?)
        } else if ctx.match_token(TokenKind::Edge) {
            (false, ctx.expect_identifier()?)
        } else {
            return Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                "Expected TAG, EDGE, SEQUENCE, or USER".to_string(),
                ctx.current_position(),
            ));
        };

        if ctx.match_token(TokenKind::Rename) {
            ctx.expect_token(TokenKind::To)?;
            let new_name = ctx.expect_identifier()?;
            let span = ctx.span_since(start_span.start);
            return Ok(Stmt::Alter(AlterStmt {
                span,
                target: if is_tag {
                    AlterTarget::RenameTag {
                        old_name: name,
                        new_name,
                    }
                } else {
                    AlterTarget::RenameEdge {
                        old_name: name,
                        new_name,
                    }
                },
            }));
        }

        // Edge endpoint-constraint changes:
        // `ALTER EDGE <name> ADD FROM <src> TO <dst>` /
        // `ALTER EDGE <name> DROP FROM <src> TO <dst>`.
        if !is_tag
            && matches!(ctx.current_token().kind, TokenKind::Add | TokenKind::Drop)
            && matches!(ctx.peek_token().kind, TokenKind::From)
        {
            let is_add = matches!(ctx.current_token().kind, TokenKind::Add);
            ctx.next_token(); // ADD | DROP
            ctx.next_token(); // FROM
            let src_tag = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::To)?;
            let dst_tag = ctx.expect_identifier()?;
            let span = ctx.span_since(start_span.start);
            let target = if is_add {
                AlterTarget::AddFrom {
                    edge_name: name,
                    src_tag,
                    dst_tag,
                }
            } else {
                AlterTarget::DropFrom {
                    edge_name: name,
                    src_tag,
                    dst_tag,
                }
            };
            return Ok(Stmt::Alter(AlterStmt { span, target }));
        }

        let (additions, deletions, changes, new_comment) = self.parse_alter_operations(ctx)?;
        if additions.is_empty() && deletions.is_empty() && changes.is_empty() && new_comment.is_none()
        {
            return Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                "ALTER TAG/EDGE requires ADD, DROP, CHANGE, or COMMENT".to_string(),
                ctx.current_position(),
            ));
        }

        let span = ctx.span_since(start_span.start);
        if is_tag {
            Ok(Stmt::Alter(AlterStmt {
                span,
                target: AlterTarget::Tag {
                    tag_name: name,
                    additions,
                    deletions,
                    changes,
                    new_comment,
                },
            }))
        } else {
            Ok(Stmt::Alter(AlterStmt {
                span,
                target: AlterTarget::Edge {
                    edge_name: name,
                    additions,
                    deletions,
                    changes,
                    new_comment,
                },
            }))
        }
    }

    fn parse_alter_operations(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<AlterOpsResult, ParseError> {
        let mut additions = Vec::new();
        let mut deletions = Vec::new();
        let mut changes = Vec::new();
        let mut new_comment: Option<String> = None;

        loop {
            if ctx.match_token(TokenKind::Add) {
                additions.extend(self.parse_property_defs(ctx)?);
            } else if ctx.match_token(TokenKind::Drop) {
                ctx.expect_token(TokenKind::LParen)?;
                loop {
                    deletions.push(ctx.expect_identifier()?);
                    if !ctx.match_token(TokenKind::Comma) {
                        break;
                    }
                }
                ctx.expect_token(TokenKind::RParen)?;
            } else if ctx.match_token(TokenKind::Change) {
                ctx.expect_token(TokenKind::LParen)?;
                loop {
                    let old_name = ctx.expect_identifier()?;
                    let new_name = ctx.expect_identifier()?;
                    ctx.expect_token(TokenKind::Colon)?;
                    let data_type = self.parse_data_type(ctx)?;
                    changes.push(PropertyChange {
                        old_name,
                        new_name,
                        data_type,
                    });
                    if !ctx.match_token(TokenKind::Comma) {
                        break;
                    }
                }
                ctx.expect_token(TokenKind::RParen)?;
            } else if ctx.match_token(TokenKind::Comment) {
                if new_comment.is_some() {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        "duplicate COMMENT clause".to_string(),
                        ctx.current_position(),
                    ));
                }
                ctx.expect_token(TokenKind::Assign)?;
                new_comment = Some(ctx.expect_string_literal()?);
            } else {
                break;
            }
        }

        Ok((additions, deletions, changes, new_comment))
    }
}
