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
                // Disambiguate: if the current token is a literal, it's syntax 2 (src -> dst OF edge_type)
                let is_literal = ctx.current_token().kind.is_literal();

                let (edge_type, edges) = if is_literal {
                    // Syntax 2: <src> -> <dst> [@rank] OF <edge_type>
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
                    ctx.expect_token(TokenKind::Of)?;
                    let edge_type = Some(ctx.expect_identifier()?);

                    (edge_type, edges)
                } else {
                    // Syntax 1: <edge_type> <src> -> <dst> [@rank] [, ...]
                    let edge_type = Some(ctx.expect_identifier()?);

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

            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            Ok(Stmt::Delete(DeleteStmt {
                span,
                target,
                where_clause: None,
                with_edge,
                detach,
            }))
        })
    }
}
