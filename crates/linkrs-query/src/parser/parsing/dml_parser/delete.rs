use super::*;

impl DmlParser {
    /// Analysis of the DELETE statement
    pub fn parse_delete_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        use crate::parser::ast::stmt::{DeleteStmt, DeleteTarget};

        // `->` is an edge arrow inside this statement, not the JSON access
        // operator, so expression parsing must not treat `-> 'key'` as a
        // postfix JSON get.
        ctx.with_edge_syntax_mode(|ctx| {
            let start_span = ctx.current_span();

            // Support DETACH DELETE syntax: DETACH DELETE ...
            let detach = ctx.match_token(TokenKind::Detach);

            ctx.expect_token(TokenKind::Delete)?;

            // Check whether there are any keywords such as VERTEX, EDGE, or TAG.
            let target = if ctx.match_token(TokenKind::Vertex) {
                // DELETE VERTEX <tag> FROM vid [, vid ...]
                let tag = ctx.expect_identifier()?;
                ctx.expect_token(TokenKind::From)?;
                let mut vids = vec![];
                loop {
                    vids.push(self.parse_expression(ctx)?);
                    if !ctx.match_token(TokenKind::Comma) {
                        break;
                    }
                }
                DeleteTarget::Vertices { tag, vids }
            } else if ctx.match_token(TokenKind::Edge) {
                // Two syntaxes:
                // 1) DELETE EDGE <edge_type> <src> -> <dst> [@rank] [, ...]
                // 2) DELETE EDGE <src> -> <dst> [@rank] OF <edge_type> [, ...]
                // Disambiguate by syntax, not by whether the first token is a literal.
                // Short form starts with an endpoint expression immediately followed by `->`.
                // Long form starts with the edge type, then `source -> destination`.
                let first = self.parse_expression(ctx)?;
                let (edge_type, edges) = if ctx.check_token(TokenKind::Arrow) {
                    // Short form: <src> -> <dst> [@rank] [, ...] OF <edge_type>
                    let mut edges = vec![];
                    let mut current_src = first;
                    loop {
                        ctx.expect_token(TokenKind::Arrow)?;
                        let dst = self.parse_expression(ctx)?;

                        let rank = if ctx.match_token(TokenKind::At) {
                            Some(self.parse_expression(ctx)?)
                        } else {
                            None
                        };

                        edges.push((current_src.clone(), dst, rank));

                        if !ctx.match_token(TokenKind::Comma) {
                            break;
                        }
                        current_src = self.parse_expression(ctx)?;
                    }
                    ctx.expect_token(TokenKind::Of)?;
                    let type_name = ctx.expect_identifier()?;
                    (Some(type_name), edges)
                } else {
                    // Long form: <edge_type> <src> -> <dst> [@rank] [, ...]
                    let type_name = match first.expression().map(|meta| meta.inner().clone()) {
                        Some(linkrs_core::types::Expression::Variable(name)) => name,
                        Some(linkrs_core::types::Expression::Literal(linkrs_core::Value::FixedString(s))) => {
                            s.to_string()
                        }
                        Some(linkrs_core::types::Expression::Literal(linkrs_core::Value::String(s))) => {
                            s.to_string()
                        }
                        _ => {
                            return Err(ParseError::new(
                                ParseErrorKind::UnexpectedToken,
                                "Expected edge type name before source expression".to_string(),
                                ctx.current_position(),
                            ));
                        }
                    };
                    let edge_type = Some(type_name);

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

                        edges.push((src, dst, rank));

                        if !ctx.match_token(TokenKind::Comma) {
                            break;
                        }
                    }
                    (edge_type, edges)
                };

                DeleteTarget::Edges { edge_type, edges }
            } else {
                return Err(ParseError::new(
                    crate::parser::core::error::ParseErrorKind::UnexpectedToken,
                    "DELETE VERTEX requires a tag qualifier: DELETE VERTEX <tag> FROM <vid>, ..."
                        .to_string(),
                    ctx.current_position(),
                ));
            };

            let with_edge = if ctx.match_token(TokenKind::With) {
                ctx.expect_token(TokenKind::Edge)?;
                true
            } else {
                false
            };

            let span = ctx.span_since(start_span.start);

            Ok(Stmt::Delete(DeleteStmt {
                span,
                target,
                with_edge,
                detach,
            }))
        })
    }
}
