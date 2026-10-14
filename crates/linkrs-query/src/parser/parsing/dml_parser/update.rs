use super::*;

impl DmlParser {
    /// Analyzing the UPDATE statement (in its complete form, including the UPDATE token)
    pub fn parse_update_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Update)?;
        self.parse_update_after_token(ctx, start_span)
    }

    /// Analyzing the UPSERT statement (in its complete form, including the UPSERT token)
    pub fn parse_upsert_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Upsert)?;
        ctx.set_upsert_mode(true);
        let result = self.parse_update_after_token(ctx, start_span);
        ctx.set_upsert_mode(false);
        result
    }

    /// Parse the UPDATE statement after the UPDATE token has been consumed.
    pub fn parse_update_after_token(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        use crate::parser::ast::stmt::{SetClause, UpdateStmt, UpdateTarget};
        use crate::parser::parsing::clause_parser::ClauseParser;

        // Check whether it is UPSERT syntax.
        let is_upsert = ctx.is_upsert_mode();

        let target = if ctx.match_token(TokenKind::Vertex) {
            if is_upsert && ctx.check_token(TokenKind::On) {
                // UPSERT VERTEX ON <tag> SET ... WHERE id(vid) == <n>
                ctx.match_token(TokenKind::On);
                let tag_name = ctx.expect_identifier()?;
                // Parse SET clause
                let set_clause = if ctx.match_token(TokenKind::Set) {
                    ClauseParser::new().parse_set_clause(ctx)?
                } else {
                    SetClause {
                        span: ctx.current_span(),
                        assignments: Vec::new(),
                    }
                };
                // Parse WHERE clause (used to specify vertex ID)
                let where_clause = if ctx.match_token(TokenKind::Where) {
                    ctx.recover_clause(|_| Ok(None), |c| self.parse_expression(c).map(Some))?
                } else {
                    None
                };
                // Parse YIELD clause
                let yield_clause = if ctx.match_token(TokenKind::Yield) {
                    Some(ClauseParser::new().parse_yield_clause(ctx)?)
                } else {
                    None
                };
                // Parse and discard an optional WHEN condition: upsert
                // semantics do not gate the insert-or-update decision on it.
                if ctx.match_token(TokenKind::When) {
                    ctx.recover_clause(|_| Ok(None), |c| self.parse_expression(c).map(Some))?;
                }
                // Extract vid from WHERE clause condition. Only the
                // `id(vid) == <literal>` conjunct is consumed; any remaining
                // terms stay as a regular filter predicate.
                let (vid_value_opt, residual) =
                    Self::extract_id_with_residual(ctx, where_clause.as_ref(), &["vid"]);
                let vid_value = vid_value_opt.ok_or_else(|| {
                    ParseError::new(
                        crate::parser::core::error::ParseErrorKind::SyntaxError,
                        "UPSERT VERTEX ON form requires WHERE id(vid) == <literal>".to_string(),
                        ctx.current_position(),
                    )
                })?;
                let vid_expr = CoreExpression::Literal(vid_value);
                let vid_expr_meta = linkrs_core::types::expr::ExpressionMeta::with_span(
                    vid_expr,
                    ctx.current_span(),
                );
                let vid_id = ctx
                    .expression_context_clone()
                    .register_expression(vid_expr_meta);
                let vid = ContextualExpression::new(vid_id, ctx.expression_context_clone());
                let span = ctx.span_since(start_span.start);
                return Ok(Stmt::Update(UpdateStmt {
                    span,
                    target: UpdateTarget::TagOnVertex {
                        vid: Box::new(vid),
                        tag_name,
                    },
                    set_clause,
                    where_clause: residual,
                    yield_clause,
                    is_upsert,
                }));
            }
            match self.parse_update_vertex(ctx)? {
                UpdateTarget::Vertex(vid) if ctx.match_token(TokenKind::On) => {
                    let tag_name = ctx.expect_identifier()?;
                    UpdateTarget::TagOnVertex {
                        vid: Box::new(vid),
                        tag_name,
                    }
                }
                other => other,
            }
        } else if ctx.match_token(TokenKind::Tag) {
            // UPDATE TAG <tag> SET ... WHERE ... (bulk update scoped to one tag).
            let tag_name = ctx.expect_identifier()?;
            UpdateTarget::Tag(tag_name)
        } else if ctx.match_token(TokenKind::Edge) {
            if is_upsert && ctx.check_token(TokenKind::On) {
                // UPSERT EDGE ON <edge_type> SET ... WHERE id(src) == <n> AND id(dst) == <m>
                ctx.match_token(TokenKind::On);
                let edge_type = ctx.expect_identifier()?;
                // Parse SET clause
                let set_clause = if ctx.match_token(TokenKind::Set) {
                    ClauseParser::new().parse_set_clause(ctx)?
                } else {
                    SetClause {
                        span: ctx.current_span(),
                        assignments: Vec::new(),
                    }
                };
                // Parse WHERE clause
                let where_clause = if ctx.match_token(TokenKind::Where) {
                    ctx.recover_clause(|_| Ok(None), |c| self.parse_expression(c).map(Some))?
                } else {
                    None
                };
                // Optional edge rank (@rank) trailing the WHERE clause.
                let rank = if ctx.match_token(TokenKind::At) {
                    Some(self.parse_expression(ctx)?)
                } else {
                    None
                };
                // Parse YIELD clause
                let yield_clause = if ctx.match_token(TokenKind::Yield) {
                    Some(ClauseParser::new().parse_yield_clause(ctx)?)
                } else {
                    None
                };
                // Parse and discard an optional WHEN condition: upsert
                // semantics do not gate the insert-or-update decision on it.
                if ctx.match_token(TokenKind::When) {
                    ctx.recover_clause(|_| Ok(None), |c| self.parse_expression(c).map(Some))?;
                }
                // Extract src and dst from WHERE clause. Only the
                // `id(src)/id(dst) == <literal>` conjuncts are consumed; any
                // remaining terms stay as a regular filter predicate.
                let (src_value_opt, after_src) =
                    Self::extract_id_with_residual(ctx, where_clause.as_ref(), &["src", "source"]);
                let src_value = src_value_opt.ok_or_else(|| {
                    ParseError::new(
                        crate::parser::core::error::ParseErrorKind::SyntaxError,
                        "UPSERT EDGE ON form requires WHERE id(src) == <literal> AND id(dst) == <literal>"
                            .to_string(),
                        ctx.current_position(),
                    )
                })?;
                let (dst_value_opt, residual) = Self::extract_id_with_residual(
                    ctx,
                    after_src.as_ref(),
                    &["dst", "dest", "destination"],
                );
                let dst_value = dst_value_opt.ok_or_else(|| {
                    ParseError::new(
                        crate::parser::core::error::ParseErrorKind::SyntaxError,
                        "UPSERT EDGE ON form requires WHERE id(src) == <literal> AND id(dst) == <literal>"
                            .to_string(),
                        ctx.current_position(),
                    )
                })?;
                let make_literal_expr =
                    |ctx: &mut ParseContext, value: linkrs_core::Value| -> ContextualExpression {
                        let expr = CoreExpression::Literal(value);
                        let meta = linkrs_core::types::expr::ExpressionMeta::with_span(
                            expr,
                            ctx.current_span(),
                        );
                        let id = ctx.expression_context_clone().register_expression(meta);
                        ContextualExpression::new(id, ctx.expression_context_clone())
                    };
                let src = make_literal_expr(ctx, src_value);
                let dst = make_literal_expr(ctx, dst_value);
                let span = ctx.span_since(start_span.start);
                return Ok(Stmt::Update(UpdateStmt {
                    span,
                    target: UpdateTarget::Edge {
                        edge_type: Some(edge_type),
                        src,
                        dst,
                        rank,
                    },
                    set_clause,
                    where_clause: residual,
                    is_upsert,
                    yield_clause,
                }));
            }
            self.parse_update_edge(ctx)?
        } else {
            // Check whether the syntax is correct for the UPSERT VERTEX vid ON tag_name command.
            if is_upsert {
                let vid = self.parse_expression(ctx)?;
                if ctx.match_token(TokenKind::On) {
                    let tag_name = ctx.expect_identifier()?;
                    UpdateTarget::TagOnVertex {
                        vid: Box::new(vid),
                        tag_name,
                    }
                } else {
                    UpdateTarget::Vertex(vid)
                }
            } else {
                // A reserved keyword cannot start the update target.  Consume
                // the keyword so error recovery cannot reinterpret the rest
                // of the input as a different statement.
                if matches!(
                    ctx.current_token().kind,
                    TokenKind::Set
                        | TokenKind::Where
                        | TokenKind::When
                        | TokenKind::Yield
                        | TokenKind::On
                        | TokenKind::Of
                ) {
                    ctx.next_token();
                    return Err(ParseError::new(
                        crate::parser::core::error::ParseErrorKind::UnexpectedToken,
                        "Expected vertex id or edge target after UPDATE".to_string(),
                        ctx.current_position(),
                    ));
                }
                // Check for old-style edge update syntax: src -> dst OF edge_type
                // Try to parse as expression first, then check if followed by ->
                let expr = self.parse_expression(ctx)?;
                if ctx.check_token(TokenKind::Arrow) {
                    // This is edge update syntax: src -> dst [@rank] OF edge_type
                    self.parse_update_edge_short(ctx, expr)?
                } else if ctx.match_token(TokenKind::On) {
                    let tag_name = ctx.expect_identifier()?;
                    UpdateTarget::TagOnVertex {
                        vid: Box::new(expr),
                        tag_name,
                    }
                } else {
                    // Regular vertex update
                    UpdateTarget::Vertex(expr)
                }
            }
        };

        let set_clause = if ctx.match_token(TokenKind::Set) {
            ClauseParser::new().parse_set_clause(ctx)?
        } else {
            SetClause {
                span: ctx.current_span(),
                assignments: Vec::new(),
            }
        };

        let where_clause = if ctx.match_token(TokenKind::Where) || ctx.match_token(TokenKind::When)
        {
            ctx.recover_clause(|_| Ok(None), |c| self.parse_expression(c).map(Some))?
        } else {
            None
        };

        // Parse YIELD clause
        let yield_clause = if ctx.match_token(TokenKind::Yield) {
            Some(ClauseParser::new().parse_yield_clause(ctx)?)
        } else {
            None
        };

        let span = ctx.span_since(start_span.start);

        Ok(Stmt::Update(UpdateStmt {
            span,
            target,
            set_clause,
            where_clause,
            is_upsert,
            yield_clause,
        }))
    }

    fn parse_update_vertex(&mut self, ctx: &mut ParseContext) -> Result<UpdateTarget, ParseError> {
        let vid = self.parse_expression(ctx)?;
        Ok(UpdateTarget::Vertex(vid))
    }

    fn parse_update_edge(&mut self, ctx: &mut ParseContext) -> Result<UpdateTarget, ParseError> {
        // `->` is an edge arrow inside this statement, not the JSON access
        // operator, so expression parsing must not treat `-> 'key'` as a
        // postfix JSON get.
        ctx.with_edge_syntax_mode(|ctx| {
            // Check whether it is UPSERT EDGE syntax: src -> dst @rank OF edge_type
            // or UPDATE EDGE Syntax: OF edge_type FROM src TO dst [@rank]
            // or UPDATE EDGE Syntax (short): src -> dst [@rank] OF edge_type
            let is_upsert = ctx.is_upsert_mode();
            let is_literal = ctx.current_token().kind.is_literal();

            if is_upsert || is_literal {
                // UPSERT EDGE or short UPDATE EDGE Syntax: src -> dst [@rank] OF edge_type
                let src = self.parse_expression(ctx)?;
                ctx.expect_token(TokenKind::Arrow)?;
                let dst = self.parse_expression(ctx)?;

                let rank = if ctx.match_token(TokenKind::At) {
                    Some(self.parse_expression(ctx)?)
                } else {
                    None
                };

                ctx.expect_token(TokenKind::Of)?;
                let edge_type = Some(ctx.expect_identifier()?);

                Ok(UpdateTarget::Edge {
                    edge_type,
                    src,
                    dst,
                    rank,
                })
            } else {
                // UPDATE EDGE Syntax: OF edge_type FROM src TO dst [@rank]
                ctx.expect_token(TokenKind::Of)?;

                // Analyzing edge types
                let edge_type = ctx.expect_identifier()?;

                // Analyzing src and dst
                ctx.expect_token(TokenKind::From)?;
                let src = self.parse_expression(ctx)?;

                ctx.expect_token(TokenKind::To)?;
                let dst = self.parse_expression(ctx)?;

                // Analysis of @rank (optional)
                let rank = if ctx.match_token(TokenKind::At) {
                    Some(self.parse_expression(ctx)?)
                } else {
                    None
                };

                Ok(UpdateTarget::Edge {
                    edge_type: Some(edge_type),
                    src,
                    dst,
                    rank,
                })
            }
        })
    }

    /// Parse short edge update syntax: src -> dst [@rank] OF edge_type
    /// This is called after parsing the src expression and seeing the -> token
    fn parse_update_edge_short(
        &mut self,
        ctx: &mut ParseContext,
        src: ContextualExpression,
    ) -> Result<UpdateTarget, ParseError> {
        use crate::parser::ast::stmt::UpdateTarget;

        ctx.with_edge_syntax_mode(|ctx| {
            // The -> token
            ctx.expect_token(TokenKind::Arrow)?;

            // Parse dst
            let dst = self.parse_expression(ctx)?;

            // Optional @rank
            let rank = if ctx.match_token(TokenKind::At) {
                Some(self.parse_expression(ctx)?)
            } else {
                None
            };

            // OF edge_type
            ctx.expect_token(TokenKind::Of)?;
            let edge_type = Some(ctx.expect_identifier()?);

            Ok(UpdateTarget::Edge {
                edge_type,
                src,
                dst,
                rank,
            })
        })
    }
}
