//! DDL Statement Parsing Module
//!
//! Responsible for parsing statements in the Data Definition Language (DDL), including CREATE, DROP, ALTER, DESC, etc.

use crate::core::types::PropertyDef;
use crate::query::parser::ast::stmt::*;
use crate::query::parser::ast::types::DataType;
use crate::query::parser::core::error::{ParseError, ParseErrorKind};
use crate::query::parser::parsing::expr_parser::ExprParser;
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::TokenKind;

/// Tag/Edge definitions result type alias
type TagEdgeDefsResult = (Vec<PropertyDef>, Option<i64>, Option<String>);

/// Alter operations result type alias
type AlterOpsResult = (Vec<PropertyDef>, Vec<String>, Vec<PropertyChange>);

/// DDL parser
pub struct DdlParser;

impl DdlParser {
    pub fn new() -> Self {
        Self
    }

    /// Analysis of the CREATE statement
    pub fn parse_create_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Create)?;

        if ctx.match_token(TokenKind::Tag) {
            // Analysis of the IF NOT EXISTS clause (located after the TAG)
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
            // Analysis of the IF NOT EXISTS clause (following the EDGE keyword)
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
            // Analysis of the CREATE SPACE command
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;

            // Analysis of the optional parameters (vid_type, comment)
            let mut vid_type = "INT64".to_string();
            let mut comment = None;

            // Analysis (vid_type=INT64, comment="xxx") format
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

                    // Check to see if there are any more parameters.
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
            // Analysis of the CREATE USER command
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

    /// Analysis of the CREATE statement (the CREATE token has already been processed/used).
    pub fn parse_create_after_token(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::query::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        if ctx.match_token(TokenKind::Tag) {
            // Check if it's CREATE TAG INDEX
            if ctx.check_token(TokenKind::Index) {
                // CREATE TAG INDEX name ON tag_name(property)
                ctx.match_token(TokenKind::Index); // consume INDEX
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

            // Analysis of the IF NOT EXISTS clause (located after the TAG)
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
            ctx.next_token(); // consume EDGE

            // Check if it's CREATE EDGE INDEX
            if ctx.check_token(TokenKind::Index) {
                ctx.next_token(); // consume INDEX
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

            // Analysis of the IF NOT EXISTS clause (used after EDGE)
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
            // Analysis of the “CREATE SPACE” command
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;

            // Analysis of the optional parameters (vid_type, comment)
            let mut vid_type = "INT64".to_string();
            let mut comment = None;

            // Analysis in the format (vid_type=INT64, comment="xxx")
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

                    // Check to see if there are any additional parameters.
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

    /// Parse the DROP statement
    pub fn parse_drop_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Drop)?;

        // Check for DROP FULLTEXT INDEX
        if ctx.check_keyword("FULLTEXT") {
            return crate::query::parser::parsing::fulltext_parser::parse_drop_fulltext_index_after_drop(ctx);
        }

        let target = if ctx.match_token(TokenKind::Space) {
            let mut if_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Exists)?;
                if_exists = true;
            }
            let space_name = ctx.expect_identifier()?;
            return Ok(Stmt::Drop(DropStmt {
                span: start_span,
                target: DropTarget::Space(space_name),
                if_exists,
            }));
        } else if ctx.match_token(TokenKind::Tag) {
            // Check if it's DROP TAG INDEX
            if ctx.check_token(TokenKind::Index) {
                ctx.next_token(); // consume INDEX
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
            let username = ctx.expect_identifier()?;

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

    /// Analyzing the DESC statement
    pub fn parse_desc_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Desc)?;

        // Check whether it is “DESCRIBE USER”.
        if ctx.check_token(TokenKind::User) {
            return self.parse_describe_user_internal(ctx, start_span);
        }

        let target = if ctx.match_token(TokenKind::Space) {
            DescTarget::Space(ctx.expect_identifier()?)
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

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Desc(DescStmt { span, target }))
    }

    /// Analysis of the internal methods of DESCRIBE USER
    fn parse_describe_user_internal(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::query::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        ctx.expect_token(TokenKind::User)?;

        let username = ctx.expect_identifier()?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::DescribeUser(DescribeUserStmt { span, username }))
    }

    /// Analysis of the SHOW CREATE statement
    pub fn parse_show_create_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Show)?;
        ctx.expect_token(TokenKind::Create)?;

        let target = if ctx.match_token(TokenKind::Space) {
            ShowCreateTarget::Space(ctx.expect_identifier()?)
        } else if ctx.match_token(TokenKind::Tag) {
            ShowCreateTarget::Tag(ctx.expect_identifier()?)
        } else if ctx.match_token(TokenKind::Edge) {
            ShowCreateTarget::Edge(ctx.expect_identifier()?)
        } else if ctx.match_token(TokenKind::Index) {
            ShowCreateTarget::Index(ctx.expect_identifier()?)
        } else {
            return Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                "Expected SPACE, TAG, EDGE, or INDEX after SHOW CREATE".to_string(),
                ctx.current_position(),
            ));
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::ShowCreate(ShowCreateStmt { span, target }))
    }

    /// Analyzing the ALTER statement
    pub fn parse_alter_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Alter)?;

        // Check for ALTER FULLTEXT INDEX
        if ctx.check_keyword("FULLTEXT") {
            return crate::query::parser::parsing::fulltext_parser::parse_alter_fulltext_index_after_alter(ctx);
        }

        // Check whether it is an ALTER USER command.
        if ctx.check_token(TokenKind::User) {
            return self.parse_alter_user_internal(ctx, start_span);
        }

        let (is_tag, name, additions, deletions, changes) = if ctx.match_token(TokenKind::Tag) {
            let tag_name = ctx.expect_identifier()?;
            let (additions, deletions, changes) = self.parse_alter_operations(ctx)?;
            (true, tag_name, additions, deletions, changes)
        } else if ctx.match_token(TokenKind::Edge) {
            let edge_name = ctx.expect_identifier()?;
            let (additions, deletions, changes) = self.parse_alter_operations(ctx)?;
            (false, edge_name, additions, deletions, changes)
        } else {
            return Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                "Expected TAG, EDGE, or USER".to_string(),
                ctx.current_position(),
            ));
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        if is_tag {
            Ok(Stmt::Alter(AlterStmt {
                span,
                target: AlterTarget::Tag {
                    tag_name: name,
                    additions,
                    deletions,
                    changes,
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
                },
            }))
        }
    }

    /// Analysis of ALTER operations (ADD/DROP/CHANGE)
    fn parse_alter_operations(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<AlterOpsResult, ParseError> {
        let mut additions = Vec::new();
        let mut deletions = Vec::new();
        let mut changes = Vec::new();

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
            } else {
                break;
            }
        }

        Ok((additions, deletions, changes))
    }

    /// Analysis of the internal methods of ALTER USER
    fn parse_alter_user_internal(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::query::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        ctx.expect_token(TokenKind::User)?;

        let username = ctx.expect_identifier()?;

        let mut password = None;
        let mut new_role = None;
        let mut is_locked = None;

        // Analyzing the WITH PASSWORD or SET clauses
        if ctx.match_token(TokenKind::With) {
            if ctx.match_token(TokenKind::Password) {
                password = Some(ctx.expect_string_literal()?);
            } else if ctx.match_token(TokenKind::Role) {
                new_role = Some(ctx.expect_identifier()?);
            }
        }

        // The SET ROLE = ... and SET LOCKED = ... syntax are also supported.
        while ctx.match_token(TokenKind::Set) {
            if ctx.match_token(TokenKind::Role) {
                ctx.expect_token(TokenKind::Eq)?;
                new_role = Some(ctx.expect_identifier()?);
            } else if ctx.match_token(TokenKind::Locked) {
                ctx.expect_token(TokenKind::Eq)?;
                let value = ctx.expect_identifier()?;
                is_locked = Some(value.to_lowercase() == "true");
            }
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::AlterUser(AlterUserStmt {
            span,
            username,
            password,
            new_role,
            is_locked,
        }))
    }

    /// Analysis of the list of attribute definitions
    pub fn parse_property_defs(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Vec<PropertyDef>, ParseError> {
        let mut defs = Vec::new();
        if ctx.match_token(TokenKind::LParen) {
            while !ctx.match_token(TokenKind::RParen) {
                let name = ctx.expect_identifier()?;
                let _ = ctx.match_token(TokenKind::Colon);

                // Parse data types, with support for keywords or identifiers.
                let dtype = self.parse_data_type(ctx)?;

                // Analysis of optional column attributes: NOT NULL / NULL
                let mut nullable = true;
                if ctx.check_token(TokenKind::Not) {
                    // Check whether it is NOT NULL by looking ahead.
                    ctx.next_token(); // “Consumption NOT”
                    if ctx.check_token(TokenKind::Null) {
                        ctx.next_token(); // Consuming NULL
                        nullable = false;
                    }
                } else if ctx.match_token(TokenKind::Null) {
                    nullable = true;
                }

                // Analysis of the term “DEFAULT”
                let mut default = None;
                if ctx.match_token(TokenKind::Default) {
                    default = Some(self.parse_value_literal(ctx)?);
                }

                // Analysis of the COMMENT
                let mut comment = None;
                if ctx.match_token(TokenKind::Comment) {
                    comment = Some(ctx.expect_string_literal()?);
                }

                defs.push(PropertyDef {
                    name,
                    data_type: dtype,
                    nullable,
                    default,
                    comment,
                });
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }
        Ok(defs)
    }

    /// Parsing literal values (used for DEFAULT)
    fn parse_value_literal(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<crate::core::Value, ParseError> {
        use crate::core::Value;

        // First, obtain a copy of the token type to avoid potential conflicts when borrowing it.
        let token_kind = ctx.current_token().kind.clone();

        // Check for function call: identifier followed by '('
        if matches!(token_kind, TokenKind::Identifier(_))
            && ctx.peek_token().kind == TokenKind::LParen
        {
            return self.parse_and_eval_function_call(ctx);
        }

        match token_kind {
            TokenKind::StringLiteral(s) => {
                ctx.next_token();
                Ok(Value::String(s))
            }
            TokenKind::IntegerLiteral(n) => {
                ctx.next_token();
                Ok(Value::BigInt(n))
            }
            TokenKind::FloatLiteral(f) => {
                ctx.next_token();
                Ok(Value::Double(f))
            }
            TokenKind::BooleanLiteral(b) => {
                ctx.next_token();
                Ok(Value::Bool(b))
            }
            TokenKind::Null => {
                ctx.next_token();
                Ok(Value::Null(crate::core::NullType::Null))
            }
            TokenKind::Minus => {
                // Working with negative numbers
                ctx.next_token();
                let inner_token_kind = ctx.current_token().kind.clone();
                match inner_token_kind {
                    TokenKind::IntegerLiteral(n) => {
                        ctx.next_token();
                        Ok(Value::BigInt(-n))
                    }
                    TokenKind::FloatLiteral(f) => {
                        ctx.next_token();
                        Ok(Value::Double(-f))
                    }
                    _ => Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        format!(
                            "Expected number after minus sign, found {:?}",
                            inner_token_kind
                        ),
                        ctx.current_position(),
                    )),
                }
            }
            _ => Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                format!("Unsupported default value type: {:?}", token_kind),
                ctx.current_position(),
            )),
        }
    }

    /// Parse a function call expression and immediately evaluate it to get a Value.
    /// Used for DEFAULT values like `DEFAULT now()`.
    fn parse_and_eval_function_call(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<crate::core::Value, ParseError> {
        let mut expr_parser = ExprParser::new(ctx);
        let parse_result = expr_parser.parse_expression(ctx)?;

        use crate::query::executor::expression::evaluation_context::DefaultExpressionContext;
        use crate::query::executor::expression::evaluator::ExpressionEvaluator;

        let mut eval_ctx = DefaultExpressionContext::new();
        ExpressionEvaluator::evaluate(&parse_result.expr, &mut eval_ctx).map_err(|e| {
            ParseError::new(
                ParseErrorKind::SyntaxError,
                format!("Failed to evaluate DEFAULT expression: {}", e),
                ctx.current_position(),
            )
        })
    }

    /// Analysis of TAG/EDGE definitions (including attribute definitions and TTL parameters)
    /// Return (list of attribute definitions, TTL_DURATION, TTL_COL)
    fn parse_tag_edge_defs(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<TagEdgeDefsResult, ParseError> {
        let mut properties = Vec::new();
        let mut ttl_duration = None;
        let mut ttl_col = None;

        if ctx.match_token(TokenKind::LParen) {
            while !ctx.check_token(TokenKind::RParen) {
                // Check whether it is a TTL parameter.
                if ctx.check_token(TokenKind::TtlDuration) {
                    ctx.next_token(); // Consumption of TTL_duration
                    ctx.expect_token(TokenKind::Assign)?;
                    ttl_duration = Some(ctx.expect_integer_literal()?);
                } else if ctx.check_token(TokenKind::TtlCol) {
                    ctx.next_token(); // Consumption TTL_COL
                    ctx.expect_token(TokenKind::Assign)?;
                    ttl_col = Some(ctx.expect_identifier()?);
                } else {
                    // Analyzing the definition of common attributes
                    let prop = self.parse_single_property_def(ctx)?;
                    properties.push(prop);
                }

                // Check to see if there are any additional parameters.
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
        }

        Ok((properties, ttl_duration, ttl_col))
    }

    /// Parse the source and destination tag names for CREATE EDGE:
    /// FROM <src_tag> TO <dst_tag>
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

    /// Analyzing the definition of a single attribute
    fn parse_single_property_def(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<PropertyDef, ParseError> {
        let name = ctx.expect_identifier()?;
        // Support both "name: TYPE" and "name TYPE" formats (colon is optional)
        ctx.match_token(TokenKind::Colon);

        // Parse data types; supports keywords or identifiers.
        let dtype = self.parse_data_type(ctx)?;

        // Analysis of optional column attributes: NOT NULL / NULL
        let mut nullable = true;
        if ctx.check_token(TokenKind::Not) {
            // Check whether it is NOT NULL by looking ahead.
            ctx.next_token(); // “Consumption NOT”
            if ctx.check_token(TokenKind::Null) {
                ctx.next_token(); // Consuming NULL
                nullable = false;
            }
        } else if ctx.match_token(TokenKind::Null) {
            nullable = true;
        }

        // Analysis of the term “DEFAULT”
        let mut default = None;
        if ctx.match_token(TokenKind::Default) {
            default = Some(self.parse_value_literal(ctx)?);
        }

        // Analysis of the COMMENT
        let mut comment = None;
        if ctx.match_token(TokenKind::Comment) {
            comment = Some(ctx.expect_string_literal()?);
        }

        Ok(PropertyDef {
            name,
            data_type: dtype,
            nullable,
            default,
            comment,
        })
    }

    /// Parse vid_type value for CREATE SPACE statement.
    /// Accepts data type keywords (STRING, INT, INT64, FIXED_STRING, etc.) or identifiers.
    fn parse_vid_type_value(&mut self, ctx: &mut ParseContext) -> Result<String, ParseError> {
        let token = ctx.current_token();
        match token.kind {
            // Accept data type keywords as vid_type values
            TokenKind::String => {
                ctx.next_token();
                Ok("STRING".to_string())
            }
            TokenKind::Int
            | TokenKind::Int8
            | TokenKind::Int16
            | TokenKind::Int32
            | TokenKind::Int64 => {
                ctx.next_token();
                Ok("INT64".to_string())
            }
            TokenKind::Float => {
                ctx.next_token();
                Ok("FLOAT".to_string())
            }
            TokenKind::Double => {
                ctx.next_token();
                Ok("DOUBLE".to_string())
            }
            TokenKind::FixedString => {
                ctx.next_token();
                // Handle FIXED_STRING(n) format
                if ctx.current_token().kind == TokenKind::LParen {
                    ctx.next_token();
                    if let TokenKind::IntegerLiteral(length) = ctx.current_token().kind {
                        let len = length;
                        ctx.next_token();
                        if ctx.current_token().kind == TokenKind::RParen {
                            ctx.next_token();
                            Ok(format!("FIXED_STRING({})", len))
                        } else {
                            Err(ParseError::new(
                                ParseErrorKind::SyntaxError,
                                "FIXED_STRING right parenthesis required".to_string(),
                                ctx.current_position(),
                            ))
                        }
                    } else {
                        Err(ParseError::new(
                            ParseErrorKind::SyntaxError,
                            "FIXED_STRING requires length parameter".to_string(),
                            ctx.current_position(),
                        ))
                    }
                } else {
                    Ok("FIXED_STRING(32)".to_string())
                }
            }
            TokenKind::Identifier(ref s) => {
                let type_name = s.clone();
                ctx.next_token();
                Ok(type_name.to_uppercase())
            }
            _ => Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!("Expected vid_type value, found {:?}", token.kind),
                ctx.current_position(),
            )),
        }
    }

    /// Parse data types, supporting keywords (such as STRING, INT) or identifiers.
    pub fn parse_data_type(&mut self, ctx: &mut ParseContext) -> Result<DataType, ParseError> {
        let token = ctx.current_token();
        match token.kind {
            // Keywords for supported data types
            TokenKind::Int
            | TokenKind::Int8
            | TokenKind::Int16
            | TokenKind::Int32
            | TokenKind::Int64 => {
                ctx.next_token();
                Ok(DataType::Int)
            }
            TokenKind::Float => {
                ctx.next_token();
                Ok(DataType::Float)
            }
            TokenKind::Double => {
                ctx.next_token();
                Ok(DataType::Double)
            }
            TokenKind::String => {
                ctx.next_token();
                Ok(DataType::String)
            }
            TokenKind::FixedString => {
                ctx.next_token();
                if ctx.current_token().kind == TokenKind::LParen {
                    ctx.next_token();
                    if let TokenKind::IntegerLiteral(len) = ctx.current_token().kind {
                        let length = len as usize;
                        ctx.next_token();
                        if ctx.current_token().kind == TokenKind::RParen {
                            ctx.next_token();
                            Ok(DataType::FixedString(length))
                        } else {
                            Err(ParseError::new(
                                ParseErrorKind::SyntaxError,
                                "FIXED_STRING Right bracket required".to_string(),
                                ctx.current_position(),
                            ))
                        }
                    } else {
                        Err(ParseError::new(
                            ParseErrorKind::SyntaxError,
                            "FIXED_STRING Need length parameter".to_string(),
                            ctx.current_position(),
                        ))
                    }
                } else {
                    Ok(DataType::FixedString(32))
                }
            }
            TokenKind::Bool => {
                ctx.next_token();
                Ok(DataType::Bool)
            }
            TokenKind::Date => {
                ctx.next_token();
                Ok(DataType::Date)
            }
            TokenKind::Timestamp => {
                ctx.next_token();
                Ok(DataType::Timestamp)
            }
            TokenKind::Datetime => {
                ctx.next_token();
                Ok(DataType::DateTime)
            }
            TokenKind::Geography => {
                ctx.next_token();
                Ok(DataType::Geography)
            }
            TokenKind::KeywordVector => {
                ctx.next_token();
                if ctx.current_token().kind == TokenKind::LParen {
                    ctx.next_token();
                    if let TokenKind::IntegerLiteral(len) = ctx.current_token().kind {
                        let dimension = len as usize;
                        ctx.next_token();
                        if ctx.current_token().kind == TokenKind::RParen {
                            ctx.next_token();
                            Ok(DataType::VectorDense(dimension))
                        } else {
                            Err(ParseError::new(
                                ParseErrorKind::SyntaxError,
                                "VECTOR Right bracket required".to_string(),
                                ctx.current_position(),
                            ))
                        }
                    } else {
                        Err(ParseError::new(
                            ParseErrorKind::SyntaxError,
                            "VECTOR requires dimension parameter".to_string(),
                            ctx.current_position(),
                        ))
                    }
                } else {
                    Ok(DataType::Vector)
                }
            }
            // Data types that support identifier formats (such as "INT", "string", etc.)
            TokenKind::Identifier(ref s) => {
                let type_name = s.clone();
                ctx.next_token();
                match type_name.to_uppercase().as_str() {
                    "INT" | "INTEGER" | "INT8" | "INT16" | "INT32" | "INT64" => Ok(DataType::Int),
                    "FLOAT" => Ok(DataType::Float),
                    "DOUBLE" => Ok(DataType::Double),
                    "STRING" | "VARCHAR" | "TEXT" => Ok(DataType::String),
                    "FIXED_STRING" | "FIXEDSTRING" => {
                        if ctx.current_token().kind == TokenKind::LParen {
                            ctx.next_token();
                            if let TokenKind::IntegerLiteral(len) = ctx.current_token().kind {
                                let length = len as usize;
                                ctx.next_token();
                                if ctx.current_token().kind == TokenKind::RParen {
                                    ctx.next_token();
                                    Ok(DataType::FixedString(length))
                                } else {
                                    Err(ParseError::new(
                                        ParseErrorKind::SyntaxError,
                                        "FIXED_STRING Right bracket required".to_string(),
                                        ctx.current_position(),
                                    ))
                                }
                            } else {
                                Err(ParseError::new(
                                    ParseErrorKind::SyntaxError,
                                    "FIXED_STRING Need length parameter".to_string(),
                                    ctx.current_position(),
                                ))
                            }
                        } else {
                            Ok(DataType::FixedString(32))
                        }
                    }
                    "BOOL" | "BOOLEAN" => Ok(DataType::Bool),
                    "DATE" => Ok(DataType::Date),
                    "TIMESTAMP" => Ok(DataType::Timestamp),
                    "DATETIME" => Ok(DataType::DateTime),
                    "GEOGRAPHY" => Ok(DataType::Geography),
                    "VECTOR" => {
                        if ctx.current_token().kind == TokenKind::LParen {
                            ctx.next_token();
                            if let TokenKind::IntegerLiteral(len) = ctx.current_token().kind {
                                let dimension = len as usize;
                                ctx.next_token();
                                if ctx.current_token().kind == TokenKind::RParen {
                                    ctx.next_token();
                                    Ok(DataType::VectorDense(dimension))
                                } else {
                                    Err(ParseError::new(
                                        ParseErrorKind::SyntaxError,
                                        "VECTOR Right bracket required".to_string(),
                                        ctx.current_position(),
                                    ))
                                }
                            } else {
                                Err(ParseError::new(
                                    ParseErrorKind::SyntaxError,
                                    "VECTOR requires dimension parameter".to_string(),
                                    ctx.current_position(),
                                ))
                            }
                        } else {
                            Ok(DataType::Vector)
                        }
                    }
                    _ => Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        format!("Unknown data type: {}", type_name),
                        ctx.current_position(),
                    )),
                }
            }
            _ => Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!("Expected data type, discovered {:?}", token.kind),
                ctx.current_position(),
            )),
        }
    }
}

impl Default for DdlParser {
    fn default() -> Self {
        Self::new()
    }
}
