use super::*;

impl DmlParser {
    /// Analyzing the INSERT statement
    pub fn parse_insert_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Insert)?;

        // Check whether it is VERTEX or EDGE.
        let target = if ctx.match_token(TokenKind::Vertex) {
            self.parse_insert_vertex(ctx, start_span)?
        } else if ctx.match_token(TokenKind::Edge) {
            self.parse_insert_edge(ctx, start_span)?
        } else {
            return Err(ParseError::new(
                crate::parser::core::error::ParseErrorKind::UnexpectedToken,
                "Expected VERTEX or EDGE after INSERT".to_string(),
                ctx.current_position(),
            ));
        };

        Ok(target)
    }

    /// Analysis of INSERT VERTEX
    fn parse_insert_vertex(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        use crate::parser::ast::stmt::{InsertStmt, InsertTarget, TagInsertSpec, VertexRow};

        // Analysis of the IF NOT EXISTS clause (optional)
        let mut if_not_exists = false;
        if ctx.match_token(TokenKind::If) {
            ctx.expect_token(TokenKind::Not)?;
            ctx.expect_token(TokenKind::Exists)?;
            if_not_exists = true;
        }

        // Analyzing the TAG list
        // Two grammatical styles are supported:
        // 1. ON tag1, tag2 (optional)
        // 2. tag_name(prop1, prop2), tag2_name(prop3, prop4) (NebulaGraph standard syntax)
        let mut tags = vec![];
        if ctx.match_token(TokenKind::On) {
            // Syntax: ON tag1, tag2
            loop {
                let tag_name = ctx.expect_identifier()?;
                tags.push(TagInsertSpec {
                    tag_name,
                    prop_names: vec![],
                    is_default_props: false,
                });
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        } else {
            // Check whether it is the NebulaGraph standard syntax: tag_name(prop1, prop2), tag2_name(prop3, prop4)
            if ctx.is_identifier_token() {
                loop {
                    let tag_name = ctx.expect_identifier()?;
                    let mut prop_names = vec![];

                    // Check whether there is a list of attribute names.
                    if ctx.match_token(TokenKind::LParen) {
                        loop {
                            let prop_name = ctx.expect_identifier()?;
                            prop_names.push(prop_name);
                            if !ctx.match_token(TokenKind::Comma) {
                                break;
                            }
                        }
                        ctx.expect_token(TokenKind::RParen)?;
                    }

                    tags.push(TagInsertSpec {
                        tag_name,
                        prop_names,
                        is_default_props: false,
                    });

                    // Check to see if there are any additional tags.
                    if !ctx.match_token(TokenKind::Comma) {
                        break;
                    }
                }
            }
        }

        // A vertex carries exactly one tag: multi-tag INSERT is rejected here.
        if tags.len() != 1 {
            return Err(ParseError::new(
                crate::parser::core::error::ParseErrorKind::UnexpectedToken,
                "INSERT VERTEX accepts exactly one tag".to_string(),
                ctx.current_position(),
            ));
        }

        // Analysis of the VALUES keyword
        if ctx.check_token(TokenKind::Values) {
            ctx.next_token(); // Consumption values
        }

        // Analysis of the list of inserted values
        let mut values = vec![];
        loop {
            // Analyzing the video…
            let vid = self.parse_expression(ctx)?;

            // Parse the single attribute list `: (...)`. A second value
            // group would address another tag and is rejected.
            let mut props = vec![];
            if ctx.match_token(TokenKind::Colon) {
                ctx.expect_token(TokenKind::LParen)?;
                loop {
                    let value = self.parse_expression(ctx)?;
                    props.push(value);
                    if !ctx.match_token(TokenKind::Comma) {
                        break;
                    }
                }
                ctx.expect_token(TokenKind::RParen)?;
                if ctx.check_token(TokenKind::Colon) {
                    return Err(ParseError::new(
                        crate::parser::core::error::ParseErrorKind::UnexpectedToken,
                        "INSERT VERTEX accepts exactly one tag value group".to_string(),
                        ctx.current_position(),
                    ));
                }
            }

            values.push(VertexRow { vid, values: props });

            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        let tag = tags.pop().ok_or_else(|| {
            ParseError::new(
                crate::parser::core::error::ParseErrorKind::UnexpectedToken,
                "INSERT VERTEX requires exactly one tag".to_string(),
                ctx.current_position(),
            )
        })?;
        Ok(Stmt::Insert(InsertStmt {
            span,
            target: InsertTarget::Vertices { tag, values },
            if_not_exists,
        }))
    }

    /// Analysis of INSERT EDGE
    fn parse_insert_edge(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        use crate::parser::ast::stmt::{InsertStmt, InsertTarget};

        let mut if_not_exists = false;
        if ctx.match_token(TokenKind::If) {
            ctx.expect_token(TokenKind::Not)?;
            ctx.expect_token(TokenKind::Exists)?;
            if_not_exists = true;
        }

        // Analyzing the list of edge types and attribute names
        let edge_name = ctx.expect_identifier()?;
        let mut prop_names = vec![];

        if ctx.match_token(TokenKind::LParen) {
            // Support empty property list: EDGE_NAME()
            if !ctx.check_token(TokenKind::RParen) {
                loop {
                    let prop_name = ctx.expect_identifier()?;
                    prop_names.push(prop_name);
                    if !ctx.match_token(TokenKind::Comma) {
                        break;
                    }
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
        }

        if ctx.check_token(TokenKind::Values) {
            ctx.next_token();
        }

        // `->` is an edge arrow inside this statement, not the JSON access
        // operator, so expression parsing must not treat `-> 'key'` as a
        // postfix JSON get.
        ctx.with_edge_syntax_mode(|ctx| {
            let mut edges = vec![];
            loop {
                let src = self.parse_expression(ctx)?;
                ctx.expect_token(TokenKind::Arrow)?;
                let dst = self.parse_expression(ctx)?;

                let rank = if ctx.match_token(TokenKind::At) {
                    Some(self.parse_expression(ctx)?)
                } else {
                    None
                };

                let mut values = vec![];
                if ctx.match_token(TokenKind::Colon) {
                    ctx.expect_token(TokenKind::LParen)?;
                    // Support empty value list: :()
                    if !ctx.check_token(TokenKind::RParen) {
                        loop {
                            let value = self.parse_expression(ctx)?;
                            values.push(value);
                            if !ctx.match_token(TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    ctx.expect_token(TokenKind::RParen)?;
                }

                edges.push((src, dst, rank, values));

                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }

            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            Ok(Stmt::Insert(InsertStmt {
                span,
                target: InsertTarget::Edge {
                    edge_name,
                    prop_names,
                    edges,
                },
                if_not_exists,
            }))
        })
    }
}
