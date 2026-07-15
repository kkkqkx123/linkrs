use crate::query::parser::ast::stmt::*;
use crate::query::parser::ast::types::DataType;
use crate::query::parser::core::error::{ParseError, ParseErrorKind};
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::TokenKind;

use super::DdlParser;

type TagEdgeDefsResult = (
    Vec<crate::core::types::PropertyDef>,
    Option<i64>,
    Option<String>,
);

impl DdlParser {
    pub fn parse_create_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Create)?;

        if ctx.match_token(TokenKind::Tag) {
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;
            let (properties, ttl_duration, ttl_col) = self.parse_tag_edge_defs(ctx)?;
            Ok(Stmt::Create(CreateStmt {
                span: start_span,
                target: CreateTarget::Tag {
                    name,
                    properties,
                    ttl_duration,
                    ttl_col,
                },
                if_not_exists,
            }))
        } else if ctx.match_token(TokenKind::Edge) {
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;
            let (properties, ttl_duration, ttl_col) = self.parse_tag_edge_defs(ctx)?;
            let (src_tag, dst_tag) = self.parse_edge_src_dst(ctx)?;
            Ok(Stmt::Create(CreateStmt {
                span: start_span,
                target: CreateTarget::EdgeType {
                    name,
                    properties,
                    ttl_duration,
                    ttl_col,
                    src_tag,
                    dst_tag,
                },
                if_not_exists,
            }))
        } else if ctx.match_token(TokenKind::Space) {
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;

            let mut vid_type = "INT64".to_string();
            let mut comment = None;

            if ctx.match_token(TokenKind::LParen) {
                loop {
                    if ctx.check_token(TokenKind::RParen) {
                        ctx.expect_token(TokenKind::RParen)?;
                        break;
                    }

                    if ctx.match_token(TokenKind::VIdType) {
                        ctx.expect_token(TokenKind::Assign)?;
                        vid_type = self.parse_vid_type_value(ctx)?;
                    } else if ctx.match_token(TokenKind::Comment) {
                        ctx.expect_token(TokenKind::Assign)?;
                        comment = Some(ctx.expect_string_literal()?);
                    }

                    if !ctx.match_token(TokenKind::Comma) {
                        ctx.expect_token(TokenKind::RParen)?;
                        break;
                    }
                }
            }

            Ok(Stmt::Create(CreateStmt {
                span: start_span,
                target: CreateTarget::Space {
                    name,
                    vid_type,
                    comment,
                },
                if_not_exists,
            }))
        } else if ctx.match_token(TokenKind::User) {
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let username = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::With)?;
            ctx.expect_token(TokenKind::Password)?;
            let password = ctx.expect_string_literal()?;

            let mut role = None;
            if ctx.match_token(TokenKind::With) {
                ctx.expect_token(TokenKind::Role)?;
                role = Some(ctx.expect_identifier()?);
            }

            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            Ok(Stmt::CreateUser(CreateUserStmt {
                span,
                username,
                password,
                role,
                if_not_exists,
            }))
        } else {
            Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                "Expected TAG, EDGE, SPACE, or USER after CREATE".to_string(),
                ctx.current_position(),
            ))
        }
    }

    pub fn parse_create_after_token(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::query::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        if ctx.match_token(TokenKind::Tag) {
            if ctx.check_token(TokenKind::Index) {
                ctx.match_token(TokenKind::Index);
                let mut if_not_exists = false;
                if ctx.match_token(TokenKind::If) {
                    ctx.expect_token(TokenKind::Not)?;
                    ctx.expect_token(TokenKind::Exists)?;
                    if_not_exists = true;
                }
                let name = ctx.expect_identifier()?;
                ctx.expect_token(TokenKind::On)?;
                let on = ctx.expect_identifier()?;
                ctx.expect_token(TokenKind::LParen)?;
                let mut properties = vec![];
                loop {
                    properties.push(ctx.expect_identifier()?);
                    if !ctx.match_token(TokenKind::Comma) {
                        break;
                    }
                }
                ctx.expect_token(TokenKind::RParen)?;
                return Ok(Stmt::Create(CreateStmt {
                    span: start_span,
                    target: CreateTarget::Index {
                        index_type: IndexType::Tag,
                        name,
                        on,
                        properties,
                    },
                    if_not_exists,
                }));
            }

            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;
            let (properties, ttl_duration, ttl_col) = self.parse_tag_edge_defs(ctx)?;
            Ok(Stmt::Create(CreateStmt {
                span: start_span,
                target: CreateTarget::Tag {
                    name,
                    properties,
                    ttl_duration,
                    ttl_col,
                },
                if_not_exists,
            }))
        } else if ctx.check_token(TokenKind::Edge) {
            ctx.next_token();

            if ctx.check_token(TokenKind::Index) {
                ctx.next_token();
                let mut if_not_exists = false;
                if ctx.match_token(TokenKind::If) {
                    ctx.expect_token(TokenKind::Not)?;
                    ctx.expect_token(TokenKind::Exists)?;
                    if_not_exists = true;
                }
                let name = ctx.expect_identifier()?;
                ctx.expect_token(TokenKind::On)?;
                let on = ctx.expect_identifier()?;
                ctx.expect_token(TokenKind::LParen)?;
                let mut properties = vec![];
                loop {
                    properties.push(ctx.expect_identifier()?);
                    if !ctx.match_token(TokenKind::Comma) {
                        break;
                    }
                }
                ctx.expect_token(TokenKind::RParen)?;
                return Ok(Stmt::Create(CreateStmt {
                    span: start_span,
                    target: CreateTarget::Index {
                        index_type: IndexType::Edge,
                        name,
                        on,
                        properties,
                    },
                    if_not_exists,
                }));
            }

            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;
            let (properties, ttl_duration, ttl_col) = self.parse_tag_edge_defs(ctx)?;
            let (src_tag, dst_tag) = self.parse_edge_src_dst(ctx)?;
            Ok(Stmt::Create(CreateStmt {
                span: start_span,
                target: CreateTarget::EdgeType {
                    name,
                    properties,
                    ttl_duration,
                    ttl_col,
                    src_tag,
                    dst_tag,
                },
                if_not_exists,
            }))
        } else if ctx.match_token(TokenKind::Space) {
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;

            let mut vid_type = "INT64".to_string();
            let mut comment = None;

            if ctx.match_token(TokenKind::LParen) {
                loop {
                    if ctx.check_token(TokenKind::RParen) {
                        ctx.expect_token(TokenKind::RParen)?;
                        break;
                    }

                    if ctx.match_token(TokenKind::VIdType) {
                        ctx.expect_token(TokenKind::Assign)?;
                        vid_type = self.parse_vid_type_value(ctx)?;
                    } else if ctx.match_token(TokenKind::Comment) {
                        ctx.expect_token(TokenKind::Assign)?;
                        comment = Some(ctx.expect_string_literal()?);
                    }

                    if !ctx.match_token(TokenKind::Comma) {
                        ctx.expect_token(TokenKind::RParen)?;
                        break;
                    }
                }
            }

            Ok(Stmt::Create(CreateStmt {
                span: start_span,
                target: CreateTarget::Space {
                    name,
                    vid_type,
                    comment,
                },
                if_not_exists,
            }))
        } else if ctx.match_token(TokenKind::Index) {
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::On)?;
            let on = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::LParen)?;
            let mut properties = vec![];
            loop {
                properties.push(ctx.expect_identifier()?);
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
            Ok(Stmt::Create(CreateStmt {
                span: start_span,
                target: CreateTarget::Index {
                    index_type: crate::query::parser::ast::stmt::IndexType::Tag,
                    name,
                    on,
                    properties,
                },
                if_not_exists,
            }))
        } else if ctx.match_token(TokenKind::Tag) {
            ctx.expect_token(TokenKind::Index)?;
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::On)?;
            let on = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::LParen)?;
            let mut properties = vec![];
            loop {
                properties.push(ctx.expect_identifier()?);
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
            Ok(Stmt::Create(CreateStmt {
                span: start_span,
                target: CreateTarget::Index {
                    index_type: crate::query::parser::ast::stmt::IndexType::Tag,
                    name,
                    on,
                    properties,
                },
                if_not_exists,
            }))
        } else if ctx.match_token(TokenKind::Edge) {
            ctx.expect_token(TokenKind::Index)?;
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::On)?;
            let on = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::LParen)?;
            let mut properties = vec![];
            loop {
                properties.push(ctx.expect_identifier()?);
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
            Ok(Stmt::Create(CreateStmt {
                span: start_span,
                target: CreateTarget::Index {
                    index_type: crate::query::parser::ast::stmt::IndexType::Edge,
                    name,
                    on,
                    properties,
                },
                if_not_exists,
            }))
        } else {
            Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                "Expected TAG, EDGE, SPACE, or INDEX after CREATE".to_string(),
                ctx.current_position(),
            ))
        }
    }

    fn parse_tag_edge_defs(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<TagEdgeDefsResult, ParseError> {
        let mut properties = Vec::new();
        let mut ttl_duration = None;
        let mut ttl_col = None;

        if ctx.match_token(TokenKind::LParen) {
            while !ctx.check_token(TokenKind::RParen) {
                if ctx.check_token(TokenKind::TtlDuration) {
                    ctx.next_token();
                    ctx.expect_token(TokenKind::Assign)?;
                    ttl_duration = Some(ctx.expect_integer_literal()?);
                } else if ctx.check_token(TokenKind::TtlCol) {
                    ctx.next_token();
                    ctx.expect_token(TokenKind::Assign)?;
                    ttl_col = Some(ctx.expect_identifier()?);
                } else {
                    let prop = self.parse_single_property_def(ctx)?;
                    properties.push(prop);
                }

                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
        }

        Ok((properties, ttl_duration, ttl_col))
    }

    fn parse_edge_src_dst(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<(Option<String>, Option<String>), ParseError> {
        if ctx.match_token(TokenKind::From) {
            let src_tag = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::To)?;
            let dst_tag = ctx.expect_identifier()?;
            Ok((Some(src_tag), Some(dst_tag)))
        } else {
            Ok((None, None))
        }
    }
}
