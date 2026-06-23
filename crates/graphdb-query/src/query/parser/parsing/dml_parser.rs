//! Data Modification Statement Parsing Module
//!
//! Responsible for parsing statements related to data modification, including INSERT, DELETE, UPDATE, MERGE, etc.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression as CoreExpression;
use crate::core::types::EdgeDirection;
use crate::query::parser::ast::stmt::*;
use crate::query::parser::core::error::ParseError;
use crate::query::parser::core::token::TokenKindExt;
use crate::query::parser::parsing::clause_parser::ClauseParser;
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::parsing::traversal_parser::TraversalParser;
use crate::query::parser::parsing::ExprParser;
use crate::query::parser::TokenKind;

/// Data Modification Parser
pub struct DmlParser;

impl DmlParser {
    pub fn new() -> Self {
        Self
    }

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
        start_span: crate::query::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        use crate::query::parser::ast::stmt::{SetClause, UpdateStmt, UpdateTarget};
        use crate::query::parser::parsing::clause_parser::ClauseParser;

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
                // Extract vid from WHERE clause condition.
                // The WHERE clause is used to specify the vertex ID, so we clear it
                // after extraction since the ID is now in the target.
                let vid_value = Self::extract_id_from_condition(where_clause.as_ref(), &["vid"])
                    .unwrap_or_else(|| crate::core::Value::Null(crate::core::NullType::Null));
                let vid_expr = CoreExpression::Literal(vid_value);
                let vid_expr_meta = crate::core::types::expr::ExpressionMeta::with_span(
                    vid_expr,
                    ctx.current_span(),
                );
                let vid_id = ctx
                    .expression_context_clone()
                    .register_expression(vid_expr_meta);
                let vid = ContextualExpression::new(vid_id, ctx.expression_context_clone());
                let end_span = ctx.current_span();
                let span = ctx.merge_span(start_span.start, end_span.end);
                return Ok(Stmt::Update(UpdateStmt {
                    span,
                    target: UpdateTarget::TagOnVertex {
                        vid: Box::new(vid),
                        tag_name,
                    },
                    set_clause,
                    where_clause: None,
                    yield_clause,
                    is_upsert,
                }));
            }
            self.parse_update_vertex(ctx)?
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
                    Some(self.parse_expression(ctx)?)
                } else {
                    None
                };
                // Extract src and dst from WHERE clause.
                // The WHERE clause is used to specify src/dst, so we clear it
                // after extraction since the IDs are now in the target.
                let make_literal_expr =
                    |ctx: &mut ParseContext, value: crate::core::Value| -> ContextualExpression {
                        let expr = CoreExpression::Literal(value);
                        let meta = crate::core::types::expr::ExpressionMeta::with_span(
                            expr,
                            ctx.current_span(),
                        );
                        let id = ctx.expression_context_clone().register_expression(meta);
                        ContextualExpression::new(id, ctx.expression_context_clone())
                    };
                let src_value =
                    Self::extract_id_from_condition(where_clause.as_ref(), &["src", "source"])
                        .unwrap_or_else(|| crate::core::Value::Null(crate::core::NullType::Null));
                let dst_value = Self::extract_id_from_condition(
                    where_clause.as_ref(),
                    &["dst", "dest", "destination"],
                )
                .unwrap_or_else(|| crate::core::Value::Null(crate::core::NullType::Null));
                let src = make_literal_expr(ctx, src_value);
                let dst = make_literal_expr(ctx, dst_value);
                let end_span = ctx.current_span();
                let span = ctx.merge_span(start_span.start, end_span.end);
                return Ok(Stmt::Update(UpdateStmt {
                    span,
                    target: UpdateTarget::Edge {
                        edge_type: Some(edge_type),
                        src,
                        dst,
                        rank: None,
                    },
                    set_clause,
                    where_clause: None,
                    is_upsert,
                    yield_clause: None,
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
                // Check for old-style edge update syntax: src -> dst OF edge_type
                // Try to parse as expression first, then check if followed by ->
                let expr = self.parse_expression(ctx)?;
                if ctx.check_token(TokenKind::Arrow) {
                    // This is edge update syntax: src -> dst [@rank] OF edge_type
                    self.parse_update_edge_short(ctx, expr)?
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
            Some(self.parse_expression(ctx)?)
        } else {
            None
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Update(UpdateStmt {
            span,
            target,
            set_clause,
            where_clause,
            is_upsert,
            yield_clause: None,
        }))
    }

    fn parse_update_vertex(&mut self, ctx: &mut ParseContext) -> Result<UpdateTarget, ParseError> {
        let vid = self.parse_expression(ctx)?;
        Ok(UpdateTarget::Vertex(vid))
    }

    /// Extract a literal value from a WHERE clause expression matching `id(<var_name>) == <literal>`.
    fn extract_id_from_condition(
        where_clause: Option<&ContextualExpression>,
        var_names: &[&str],
    ) -> Option<crate::core::Value> {
        let expr = where_clause?.get_expression()?;
        Self::extract_id_from_expr(&expr, var_names)
    }

    fn extract_id_from_expr(
        expr: &CoreExpression,
        var_names: &[&str],
    ) -> Option<crate::core::Value> {
        match expr {
            CoreExpression::Binary { left, op, right } => {
                use crate::core::types::operators::BinaryOperator;
                match op {
                    BinaryOperator::Equal => {
                        if let CoreExpression::Function { name, args } = left.as_ref() {
                            if name == "id" && args.len() == 1 {
                                if let CoreExpression::Variable(v) = &args[0] {
                                    if var_names.iter().any(|n| *n == *v) {
                                        if let CoreExpression::Literal(value) = right.as_ref() {
                                            return Some(value.clone());
                                        }
                                    }
                                }
                            }
                        }
                        None
                    }
                    BinaryOperator::And => Self::extract_id_from_expr(left, var_names)
                        .or_else(|| Self::extract_id_from_expr(right, var_names)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn parse_update_edge(&mut self, ctx: &mut ParseContext) -> Result<UpdateTarget, ParseError> {
        // Check whether it is UPSERT EDGE syntax: src -> dst @rank OF edge_type
        // or UPDATE EDGE Syntax: OF edge_type FROM src TO dst [@rank]
        // or UPDATE EDGE Syntax (short): src -> dst [@rank] OF edge_type
        let is_upsert = ctx.is_upsert_mode();
        let is_literal = ctx.peek_token().kind.is_literal();

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
    }

    /// Parse short edge update syntax: src -> dst [@rank] OF edge_type
    /// This is called after parsing the src expression and seeing the -> token
    fn parse_update_edge_short(
        &mut self,
        ctx: &mut ParseContext,
        src: ContextualExpression,
    ) -> Result<UpdateTarget, ParseError> {
        use crate::query::parser::ast::stmt::UpdateTarget;

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
    }

    /// Analysis of the DELETE statement
    pub fn parse_delete_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        use crate::query::parser::ast::stmt::{DeleteStmt, DeleteTarget};

        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Delete)?;

        // Check whether there are any keywords such as VERTEX, EDGE, or TAG.
        let target = if ctx.match_token(TokenKind::Vertex) {
            // DELETE VERTEX vid [, vid ...]
            let mut vids = vec![];
            loop {
                vids.push(self.parse_expression(ctx)?);
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            DeleteTarget::Vertices(vids)
        } else if ctx.match_token(TokenKind::Edge) {
            // Two syntaxes:
            // 1) DELETE EDGE <edge_type> <src> -> <dst> [@rank] [, ...]
            // 2) DELETE EDGE <src> -> <dst> [@rank] OF <edge_type> [, ...]
            // Disambiguate: if the next token is a literal, it's syntax 2 (src -> dst OF edge_type)
            let is_literal = ctx.peek_token().kind.is_literal();

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
        } else if ctx.match_token(TokenKind::Tag) {
            // DELETE TAG tag_name [, tag_name ...] FROM vid [, vid ...]
            let mut tags = vec![];

            // Check whether it is a wildcard character (*).
            if ctx.match_token(TokenKind::Star) {
                tags.push("*".to_string());
            } else {
                // Parse the list of tags
                loop {
                    let tag_name = ctx.expect_identifier()?;
                    tags.push(tag_name);
                    if !ctx.match_token(TokenKind::Comma) {
                        break;
                    }
                }
            }

            // The “FROM” keyword in a query
            ctx.expect_token(TokenKind::From)?;

            // Analyzing the list of vertex IDs
            let mut vids = vec![];
            loop {
                vids.push(self.parse_expression(ctx)?);
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }

            let is_all_tags = tags.iter().any(|t| t == "*");

            DeleteTarget::Tags {
                tag_names: tags,
                vertex_ids: vids,
                is_all_tags,
            }
        } else {
            // The default interpretation is the deletion of vertices.
            let mut vids = vec![];
            loop {
                vids.push(self.parse_expression(ctx)?);
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            DeleteTarget::Vertices(vids)
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
        }))
    }

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
                crate::query::parser::core::error::ParseErrorKind::UnexpectedToken,
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
        start_span: crate::query::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        use crate::query::parser::ast::stmt::{InsertStmt, InsertTarget, TagInsertSpec, VertexRow};

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
        // 2. tag_name(prop1, prop2), tag2_name(prop3, prop4)（NebulaGraph 标准语法）
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
            // 检查是否是 NebulaGraph 标准语法：tag_name(prop1, prop2), tag2_name(prop3, prop4)
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

        // Analysis of the VALUES keyword
        if ctx.check_token(TokenKind::Values) {
            ctx.next_token(); // Consumption values
        }

        // Analysis of the list of inserted values
        let mut values = vec![];
        loop {
            // Analyzing the video…
            let vid = self.parse_expression(ctx)?;

            // Parse the attribute list (supports multiple tag value groups)
            let mut tag_values = vec![];
            if ctx.match_token(TokenKind::Colon) {
                ctx.expect_token(TokenKind::LParen)?;
                let mut props = vec![];
                loop {
                    let value = self.parse_expression(ctx)?;
                    props.push(value);
                    if !ctx.match_token(TokenKind::Comma) {
                        break;
                    }
                }
                ctx.expect_token(TokenKind::RParen)?;
                tag_values.push(props);

                // Additional value groups for multi-tag insert (colon-separated groups)
                while ctx.match_token(TokenKind::Colon) {
                    ctx.expect_token(TokenKind::LParen)?;
                    let mut props = vec![];
                    loop {
                        let value = self.parse_expression(ctx)?;
                        props.push(value);
                        if !ctx.match_token(TokenKind::Comma) {
                            break;
                        }
                    }
                    ctx.expect_token(TokenKind::RParen)?;
                    tag_values.push(props);
                }
            }

            values.push(VertexRow { vid, tag_values });

            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Insert(InsertStmt {
            span,
            target: InsertTarget::Vertices { tags, values },
            if_not_exists,
        }))
    }

    /// Analysis of INSERT EDGE
    fn parse_insert_edge(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::query::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        use crate::query::parser::ast::stmt::{InsertStmt, InsertTarget};

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
    }

    /// Parse the MERGE statement
    pub fn parse_merge_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Merge)?;

        // Support MERGE EDGE/VERTEX ON ... SET ... WHERE ... (UPSERT-style syntax)
        if ctx.check_token(TokenKind::Edge) || ctx.check_token(TokenKind::Vertex) {
            ctx.set_upsert_mode(true);
            let result = self.parse_update_after_token(ctx, start_span);
            ctx.set_upsert_mode(false);
            return result;
        }

        let pattern = TraversalParser::new().parse_pattern(ctx)?;

        let on_create = if ctx.match_token(TokenKind::On) && ctx.match_token(TokenKind::Create) {
            Some(ClauseParser::new().parse_set_clause(ctx)?)
        } else {
            None
        };

        let on_match = if ctx.match_token(TokenKind::On) && ctx.match_token(TokenKind::Match) {
            Some(ClauseParser::new().parse_set_clause(ctx)?)
        } else {
            None
        };

        Ok(Stmt::Merge(MergeStmt {
            span: start_span,
            pattern,
            on_create,
            on_match,
        }))
    }

    /// Parse the expression
    fn parse_expression(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ContextualExpression, ParseError> {
        let mut expr_parser = ExprParser::new(ctx);
        expr_parser.parse_expression_with_context(ctx, ctx.expression_context_clone())
    }

    /// Parse the Cypher-style CREATE data statement (the CREATE token has already been consumed)
    /// Support for grammar:
    ///   CREATE (n:Label {prop: value})
    ///   CREATE (a)-[:Type {prop: value}]->(b)
    ///   CREATE (a:Label1)-[:Type]->(b:Label2)
    pub fn parse_create_data_after_token(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::query::parser::ast::types::Span,
    ) -> Result<Stmt, ParseError> {
        // List of analysis modes (multiple modes can be separated by commas)
        let mut patterns = Vec::new();

        loop {
            let pattern = self.parse_create_pattern(ctx)?;
            patterns.push(pattern);

            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Create(CreateStmt {
            span,
            target: CreateTarget::Path { patterns },
            if_not_exists: false,
        }))
    }

    /// Parse the schema in the CREATE statement
    fn parse_create_pattern(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<crate::query::parser::ast::pattern::Pattern, ParseError> {
        use crate::query::parser::ast::pattern::*;

        let start_node = self.parse_node_pattern(ctx)?;

        // Check whether there is an edge pattern (using Arrow or LeftArrow).
        if ctx.check_token(TokenKind::Arrow) || ctx.check_token(TokenKind::LeftArrow) {
            let edge = self.parse_edge_pattern(ctx)?;
            let end_node = self.parse_node_pattern(ctx)?;

            let span = ctx.merge_span(start_node.span.start, end_node.span.end);
            let elements = vec![
                PathElement::Node(start_node),
                PathElement::Edge(edge),
                PathElement::Node(end_node),
            ];
            Ok(Pattern::Path(PathPattern { span, elements }))
        } else {
            Ok(Pattern::Node(start_node))
        }
    }

    /// Parse node pattern: (var:Label {prop: value})
    fn parse_node_pattern(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<crate::query::parser::ast::pattern::NodePattern, ParseError> {
        use crate::query::parser::ast::pattern::*;

        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::LParen)?;

        // Optional variable names
        let variable = if ctx.current_token().kind.is_identifier() {
            Some(ctx.expect_identifier()?)
        } else {
            None
        };

        let mut labels = Vec::new();
        if ctx.match_token(TokenKind::Colon) {
            loop {
                labels.push(ctx.expect_identifier()?);
                if !ctx.match_token(TokenKind::Colon) {
                    break;
                }
            }
        }

        let properties = if ctx.match_token(TokenKind::LBrace) {
            let props = self.parse_property_map(ctx)?;
            ctx.expect_token(TokenKind::RBrace)?;
            Some(props)
        } else {
            None
        };

        ctx.expect_token(TokenKind::RParen)?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(NodePattern {
            span,
            variable,
            labels,
            properties,
            predicates: Vec::new(),
        })
    }

    /// Parse edge pattern: -[:Type {prop: value}]-> or <-[:Type {prop: value}]-
    fn parse_edge_pattern(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<crate::query::parser::ast::pattern::EdgePattern, ParseError> {
        use crate::query::parser::ast::pattern::*;

        let start_span = ctx.current_span();

        let direction = if ctx.match_token(TokenKind::LeftArrow) {
            EdgeDirection::In
        } else if ctx.match_token(TokenKind::Arrow) || ctx.match_token(TokenKind::RightArrow) {
            EdgeDirection::Out
        } else {
            EdgeDirection::Both
        };

        ctx.expect_token(TokenKind::LBracket)?;

        let variable = if ctx.current_token().kind.is_identifier() {
            Some(ctx.expect_identifier()?)
        } else {
            None
        };

        let mut edge_types = Vec::new();
        if ctx.match_token(TokenKind::Colon) {
            edge_types.push(ctx.expect_identifier()?);
        }

        let properties = if ctx.match_token(TokenKind::LBrace) {
            let props = self.parse_property_map(ctx)?;
            ctx.expect_token(TokenKind::RBrace)?;
            Some(props)
        } else {
            None
        };

        ctx.expect_token(TokenKind::RBracket)?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(EdgePattern {
            span,
            variable,
            edge_types,
            properties,
            predicates: Vec::new(),
            direction,
            range: None,
        })
    }

    /// Parse property map: {prop1: value1, prop2: value2}
    fn parse_property_map(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ContextualExpression, ParseError> {
        let _start_span = ctx.current_span();
        let mut properties = Vec::new();

        if !ctx.check_token(TokenKind::RBrace) {
            loop {
                let key = ctx.expect_identifier()?;
                ctx.expect_token(TokenKind::Colon)?;
                let value = self.parse_expression(ctx)?;
                let value_expr = value
                    .expression()
                    .ok_or_else(|| {
                        ParseError::new_simple(
                            "Expression not registered in context".to_string(),
                            ctx.current_position(),
                        )
                    })?
                    .inner()
                    .clone();
                properties.push((key, value_expr));

                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }

        let expr = CoreExpression::Map(properties);
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.expression_context().register_expression(expr_meta);
        Ok(ContextualExpression::new(
            id,
            ctx.expression_context_clone(),
        ))
    }
}

impl Default for DmlParser {
    fn default() -> Self {
        Self::new()
    }
}
