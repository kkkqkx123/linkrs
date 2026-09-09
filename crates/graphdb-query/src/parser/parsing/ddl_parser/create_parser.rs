use crate::parser::ast::stmt::*;

use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

use super::DdlParser;

/// Whether a type identifier spelled with a leading identifier denotes a
/// builtin type (including DECIMAL/UNION/FIXED_STRING/VECTOR, which carry
/// parameters), as opposed to a user-defined type alias. Used to decide
/// whether the original spelling — rather than the resolved canonical type —
/// must be kept as the underlying text for alias dependency tracking.
fn is_identifier_spelled_builtin_type(name: &str) -> bool {
    match name.to_uppercase().as_str() {
        "DECIMAL" | "UNION" | "FIXED_STRING" | "FIXEDSTRING" | "VECTOR" => true,
        _ => name
            .to_uppercase()
            .parse::<crate::parser::ast::types::DataType>()
            .is_ok(),
    }
}

type TagEdgeDefsResult = (
    Vec<graphdb_core::types::PropertyDef>,
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

            if ctx.match_token(TokenKind::As) {
                ctx.expect_token(TokenKind::LParen)?;
                let mut depth = 1;
                let mut query_text = String::new();
                while depth > 0 {
                    match ctx.current_token().kind {
                        crate::parser::TokenKind::LParen => {
                            depth += 1;
                            query_text.push('(');
                            ctx.next_token();
                        }
                        crate::parser::TokenKind::RParen => {
                            depth -= 1;
                            if depth > 0 {
                                query_text.push(')');
                            }
                            ctx.next_token();
                        }
                        crate::parser::TokenKind::Eof => {
                            return Err(ParseError::new(
                                ParseErrorKind::SyntaxError,
                                "Unexpected end of input in subquery".to_string(),
                                ctx.current_position(),
                            ));
                        }
                        _ => {
                            if !query_text.is_empty() {
                                query_text.push(' ');
                            }
                            query_text.push_str(&ctx.current_token().lexeme);
                            ctx.next_token();
                        }
                    }
                }
                let end_span = ctx.current_span();
                let span = ctx.merge_span(start_span.start, end_span.end);
                return Ok(Stmt::Create(CreateStmt {
                    span,
                    target: CreateTarget::TagAsQuery { name, query_text },
                    if_not_exists,
                }));
            }

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
            let name = ctx.expect_dcl_name()?;

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

            self.parse_space_with_params(ctx, &mut comment)?;

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
            let username = ctx.expect_dcl_name()?;
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
        start_span: crate::parser::ast::types::Span,
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
            let name = ctx.expect_dcl_name()?;

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

            self.parse_space_with_params(ctx, &mut comment)?;

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
                    index_type: crate::parser::ast::stmt::IndexType::Tag,
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
                    index_type: crate::parser::ast::stmt::IndexType::Tag,
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
                    index_type: crate::parser::ast::stmt::IndexType::Edge,
                    name,
                    on,
                    properties,
                },
                if_not_exists,
            }))
        } else if ctx.match_token(TokenKind::Sequence) {
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;
            let mut start = None;
            let mut increment = None;
            let mut min_value = None;
            let mut max_value = None;
            let mut cycle = false;

            // Parse optional clauses: START, INCREMENT, MINVALUE, MAXVALUE, CYCLE/NOCYCLE
            loop {
                if ctx.check_keyword("START") && start.is_none() {
                    ctx.next_token();
                    ctx.expect_token(TokenKind::Assign)?;
                    start = Some(ctx.expect_integer_literal()?);
                } else if ctx.check_keyword("INCREMENT") && increment.is_none() {
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
                    cycle = true;
                } else if ctx.check_keyword("NOCYCLE") {
                    ctx.next_token();
                    cycle = false;
                } else {
                    break;
                }
            }

            Ok(Stmt::Create(CreateStmt {
                span: start_span,
                target: CreateTarget::Sequence {
                    name,
                    start,
                    increment,
                    min_value,
                    max_value,
                    cycle,
                },
                if_not_exists,
            }))
        } else if ctx.check_keyword("MACRO") {
            ctx.consume_keyword("MACRO")?;
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::LParen)?;
            let mut params = vec![];
            while !ctx.check_token(TokenKind::RParen) {
                let param_name = ctx.expect_identifier()?;
                let default_value = if ctx.match_token(TokenKind::Assign) {
                    let expr = crate::parser::parsing::expr_parser::parse_expression_with_context(
                        ctx,
                        ctx.expression_context_clone(),
                    )?;
                    Some(expr)
                } else {
                    None
                };
                params.push(MacroParam {
                    name: param_name,
                    default_value,
                });
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
            ctx.consume_keyword("AS")?;
            let body = crate::parser::parsing::expr_parser::parse_expression_with_context(
                ctx,
                ctx.expression_context_clone(),
            )?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::CreateMacro(CreateMacroStmt {
                span,
                name,
                params,
                body,
                if_not_exists,
            }))
        } else if ctx.check_keyword("TYPE") {
            ctx.consume_keyword("TYPE")?;
            let mut if_not_exists = false;
            if ctx.match_token(TokenKind::If) {
                ctx.expect_token(TokenKind::Not)?;
                ctx.expect_token(TokenKind::Exists)?;
                if_not_exists = true;
            }
            let name = ctx.expect_identifier()?;
            ctx.consume_keyword("AS")?;
            // The underlying type is either a builtin type or a reference to
            // another alias. Builtins parse directly; an unknown leading
            // identifier is kept as raw alias text (DataType::Unknown) and
            // resolved at plan time through the type-alias catalog, where
            // cycle detection also runs.
            let ckpt = ctx.checkpoint();
            // Capture the original spelling of the underlying type. When it
            // denotes a user-defined alias, `parse_data_type` resolves it to
            // the underlying builtin and `parsed.to_string()` no longer
            // mentions the alias. Preserving the original identifier text is
            // required for alias-to-alias dependency tracking and the
            // drop-referenced-alias check in the type-alias catalog.
            let leading_is_identifier =
                matches!(ctx.current_token().kind, TokenKind::Identifier(_));
            let leading_lexeme = ctx.current_token().lexeme.clone();
            let (underlying_type, underlying_type_text) = match self.parse_data_type(ctx) {
                Ok(parsed) => {
                    // A leading identifier that is neither a builtin spelling
                    // nor a DECIMAL/UNION/FIXED_STRING/VECTOR spelling must be
                    // an alias reference — keep it verbatim. Genuine builtin
                    // spellings retain their canonical `to_string()` form.
                    let text = if leading_is_identifier
                        && !is_identifier_spelled_builtin_type(&leading_lexeme)
                    {
                        leading_lexeme
                    } else {
                        parsed.to_string()
                    };
                    (parsed, text)
                }
                Err(parse_err) => {
                    if !leading_is_identifier {
                        return Err(parse_err);
                    }
                    ctx.restore(ckpt);
                    let alias_name = ctx.expect_identifier()?;
                    (DataType::Unknown, alias_name.clone())
                }
            };
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::CreateType(CreateTypeStmt {
                span,
                name,
                underlying_type,
                underlying_type_text,
                if_not_exists,
            }))
        } else {
            Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                "Expected TAG, EDGE, SPACE, INDEX, SEQUENCE, MACRO, or TYPE after CREATE"
                    .to_string(),
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

    /// Parse optional space parameters: (vid_type=..., comment='...') and
    /// WITH key=value clauses (e.g. WITH DIMENSION=128).
    fn parse_space_with_params(
        &mut self,
        ctx: &mut ParseContext,
        comment: &mut Option<String>,
    ) -> Result<(), ParseError> {
        while ctx.match_token(TokenKind::With) {
            let key = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::Assign)?;
            let value = if let TokenKind::StringLiteral(s) = ctx.current_token().kind.clone() {
                ctx.next_token();
                s
            } else {
                ctx.expect_integer_literal()?.to_string()
            };
            if key.eq_ignore_ascii_case("comment") {
                *comment = Some(value);
            }
        }
        Ok(())
    }
}
