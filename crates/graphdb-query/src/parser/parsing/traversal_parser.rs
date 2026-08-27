//! Graph Traversal Statement Parsing Module
//!
//! Responsible for parsing statements related to graph traversal, including MATCH, GO, FIND PATH, GET SUBGRAPH, etc.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression as CoreExpression;
use crate::core::types::graph_schema::EdgeDirection;
use crate::parser::ast::pattern::{
    EdgePattern, EdgeRange, NodePattern, PathElement, PathPattern, Pattern, VariablePattern,
};
use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::clause_parser::ClauseParser;
use crate::parser::parsing::dml_parser::DmlParser;
use crate::parser::parsing::expr_parser::parse_expression_with_context;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

/// Graph Traversal Parser
pub struct TraversalParser;

/// The body of one MATCH clause, used when merging consecutive MATCH clauses.
struct ParsedMatchClause {
    patterns: Vec<Pattern>,
    where_clause: Option<ContextualExpression>,
    where_explicit: bool,
    return_clause: Option<ReturnClause>,
    delete_clause: Option<MatchDeleteClause>,
}

impl TraversalParser {
    pub fn new() -> Self {
        Self
    }

    /// Analyzing the MATCH statement
    pub fn parse_match_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();

        // Check whether it is an OPTIONAL MATCH.
        let mut optional = ctx.match_token(TokenKind::Optional);

        ctx.expect_token(TokenKind::Match)?;

        let first = self.parse_match_clause(ctx)?;
        let mut patterns = first.patterns;
        let mut where_clause = first.where_clause;
        let mut where_explicit = first.where_explicit;
        let mut return_clause = first.return_clause;
        let mut delete_clause = first.delete_clause;

        // Consecutive plain MATCH clauses (`MATCH a MATCH b ...`) are merged
        // into a single statement: all patterns combine, WHERE clauses are
        // AND-ed, and RETURN/DELETE come from the last clause providing one.
        // An OPTIONAL MATCH continuation (`MATCH a OPTIONAL MATCH b ...`)
        // is merged the same way and marks the whole statement as optional.
        // Chains starting with OPTIONAL MATCH are left unchanged; a trailing
        // MATCH there is still reported by the outer parser.
        while !optional
            && return_clause.is_none()
            && delete_clause.is_none()
            && (ctx.check_token(TokenKind::Match)
                || (ctx.check_token(TokenKind::Optional)
                    && ctx.peek_token().kind == TokenKind::Match))
        {
            if ctx.check_token(TokenKind::Optional) {
                ctx.expect_token(TokenKind::Optional)?;
                optional = true;
            }
            ctx.expect_token(TokenKind::Match)?;
            let next = self.parse_match_clause(ctx)?;
            patterns.extend(next.patterns);

            match (where_explicit, next.where_explicit) {
                (false, true) => where_clause = next.where_clause,
                (true, true) => {
                    if let (Some(left), Some(right)) = (&where_clause, &next.where_clause) {
                        if let Some(combined) = ctx.expression_context().and(left, right) {
                            where_clause = Some(combined);
                        }
                    }
                }
                _ => {}
            }
            where_explicit |= next.where_explicit;

            if let Some(rc) = next.return_clause {
                return_clause = Some(rc);
            }
            if let Some(dc) = next.delete_clause {
                delete_clause = Some(dc);
            }
        }

        let (order_by, limit, skip) = if let Some(ref rc) = return_clause {
            (rc.order_by.clone(), rc.limit.clone(), rc.skip.clone())
        } else {
            (None, None, None)
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Match(MatchStmt {
            span,
            patterns,
            where_clause,
            return_clause,
            order_by,
            limit,
            skip,
            optional,
            delete_clause,
        }))
    }

    /// Parses the body of one MATCH clause: comma-separated patterns, an
    /// optional WHERE expression (defaulting to a literal true), an optional
    /// RETURN clause, and an optional DELETE clause.
    fn parse_match_clause(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ParsedMatchClause, ParseError> {
        let mut patterns = Vec::new();
        loop {
            let Some(pattern) = ctx.recover_clause(
                |c| {
                    Ok(Some(Pattern::Variable(VariablePattern {
                        span: c.current_span(),
                        name: String::new(),
                    })))
                },
                |c| self.parse_pattern(c).map(Some),
            )?
            else {
                break;
            };
            patterns.push(pattern);
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }

        let (mut where_clause, mut where_explicit) = if ctx.match_token(TokenKind::Where) {
            (
                Some(
                    ctx.recover_clause(Self::create_true_expression, |c| self.parse_expression(c))?,
                ),
                true,
            )
        } else {
            (Some(Self::create_true_expression(ctx)?), false)
        };

        let return_clause = if ctx.match_token(TokenKind::Return) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| ClauseParser::new().parse_return_clause(c).map(Some),
            )?
        } else {
            None
        };

        let delete_clause = if ctx.match_token(TokenKind::Delete) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| self.parse_match_delete_clause(c).map(Some),
            )?
        } else {
            None
        };

        // Accept a WHERE clause after RETURN (`RETURN ... WHERE ...`): the
        // filter is AND-ed with any pre-RETURN WHERE, mirroring the
        // consecutive-clause merge behavior.
        if ctx.match_token(TokenKind::Where) {
            let post =
                ctx.recover_clause(Self::create_true_expression, |c| self.parse_expression(c))?;
            if let Some(ref pre) = where_clause {
                if let Some(combined) = ctx.expression_context().and(pre, &post) {
                    where_clause = Some(combined);
                } else {
                    where_clause = Some(post);
                }
            } else {
                where_clause = Some(post);
            }
            where_explicit = true;
        }

        // Accept a trailing MERGE clause (`MATCH ... MERGE ...`). It is
        // parsed for syntax validation but not stored on the MATCH
        // statement: the matched rows are returned unchanged.
        if ctx.check_token(TokenKind::Merge) {
            DmlParser::new().parse_merge_statement(ctx)?;
        }

        Ok(ParsedMatchClause {
            patterns,
            where_clause,
            where_explicit,
            return_clause,
            delete_clause,
        })
    }

    fn parse_match_delete_clause(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<MatchDeleteClause, ParseError> {
        let start_span = ctx.current_span();

        let target = if ctx.match_token(TokenKind::Vertex) {
            let vertex_ids = self.parse_expression_list(ctx)?;
            MatchDeleteTarget::Vertices(vertex_ids)
        } else if ctx.match_token(TokenKind::Edge) {
            // Two sub-syntaxes:
            // 1) Edge variable: DELETE EDGE e [, e2, ...]
            // 2) Edge refs:     DELETE EDGE a -> b [@rank] [, a2 -> b2 @rank2, ...]
            // Disambiguate: parse first expression, then check for Arrow token
            let first_expr = self.parse_expression(ctx)?;
            if ctx.check_token(TokenKind::Arrow) {
                // Syntax 2: a -> b [@rank] [, ...]
                let mut edge_refs = Vec::new();
                let mut current_src = first_expr;
                loop {
                    ctx.expect_token(TokenKind::Arrow)?;
                    let dst = self.parse_expression(ctx)?;
                    let rank = if ctx.match_token(TokenKind::At) {
                        Some(self.parse_expression(ctx)?)
                    } else {
                        None
                    };
                    edge_refs.push((current_src, dst, rank));
                    if ctx.match_token(TokenKind::Comma) {
                        current_src = self.parse_expression(ctx)?;
                    } else {
                        break;
                    }
                }
                MatchDeleteTarget::EdgeRefs(edge_refs)
            } else {
                // Syntax 1: edge variable e [, e2, ...]
                let mut edge_refs = vec![first_expr];
                while ctx.match_token(TokenKind::Comma) {
                    edge_refs.push(self.parse_expression(ctx)?);
                }
                MatchDeleteTarget::Edges(edge_refs)
            }
        } else {
            return Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                "Expected VERTEX or EDGE after DELETE".to_string(),
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

        Ok(MatchDeleteClause {
            span,
            target,
            with_edge,
        })
    }

    /// Analyzing GO statements
    pub fn parse_go_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Go)?;

        let steps = self.parse_steps(ctx)?;

        // Consumption of optional STEP/STEP keywords
        ctx.match_token(TokenKind::Step);

        ctx.expect_token(TokenKind::From)?;
        let from_span = ctx.current_span();
        let from_clause = ctx.recover_clause(
            |c| {
                Ok(FromClause {
                    span: c.current_span(),
                    vertices: Vec::new(),
                })
            },
            |c| {
                let vertices = self.parse_expression_list(c)?;
                Ok(FromClause {
                    span: from_span,
                    vertices,
                })
            },
        )?;

        let over = if ctx.match_token(TokenKind::Over) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| ClauseParser::new().parse_over_clause(c).map(Some),
            )?
        } else {
            None
        };

        let where_clause = if ctx.match_token(TokenKind::Where) {
            Some(ctx.recover_clause(Self::create_true_expression, |c| self.parse_expression(c))?)
        } else {
            Some(Self::create_true_expression(ctx)?)
        };

        let yield_clause = if ctx.match_token(TokenKind::Yield) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| ClauseParser::new().parse_yield_clause(c).map(Some),
            )?
        } else {
            None
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Go(GoStmt {
            span,
            steps,
            from: from_clause,
            over,
            where_clause,
            yield_clause,
        }))
    }

    /// Analysis of the FIND PATH statement
    pub fn parse_find_path_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Find)?;

        // Path type analysis: SHORTEST, ALL
        let shortest = if ctx.match_token(TokenKind::Shortest) {
            true
        } else {
            !ctx.match_token(TokenKind::All)
        };

        ctx.expect_token(TokenKind::Path)?;

        // Optional options: WITH LOOP / WITH CYCLE
        let mut with_loop = false;
        let mut with_cycle = false;
        while ctx.match_token(TokenKind::With) {
            if ctx.match_token(TokenKind::Loop) {
                with_loop = true;
            } else if ctx.match_token(TokenKind::Cycle) {
                with_cycle = true;
            }
        }

        ctx.expect_token(TokenKind::From)?;
        let from_span = ctx.current_span();
        let from_clause = ctx.recover_clause(
            |c| {
                Ok(FromClause {
                    span: c.current_span(),
                    vertices: Vec::new(),
                })
            },
            |c| {
                let vertices = self.parse_expression_list(c)?;
                Ok(FromClause {
                    span: from_span,
                    vertices,
                })
            },
        )?;

        let to_present = ctx.recover_clause(
            |_| Ok(false),
            |c| {
                c.expect_token(TokenKind::To)?;
                Ok(true)
            },
        )?;
        let to_vertex = if to_present {
            ctx.recover_clause(Self::create_true_expression, |c| self.parse_expression(c))?
        } else {
            Self::create_true_expression(ctx)?
        };

        let over_present = ctx.recover_clause(
            |_| Ok(false),
            |c| {
                c.expect_token(TokenKind::Over)?;
                Ok(true)
            },
        )?;
        let over = if over_present {
            ctx.recover_clause(
                |c| {
                    Ok(OverClause {
                        span: c.current_span(),
                        edge_types: Vec::new(),
                        direction: EdgeDirection::Out,
                    })
                },
                |c| ClauseParser::new().parse_over_clause(c),
            )?
        } else {
            OverClause {
                span: ctx.current_span(),
                edge_types: Vec::new(),
                direction: EdgeDirection::Out,
            }
        };

        // Optional: Up to N steps
        let mut max_steps = None;
        if ctx.match_token(TokenKind::Upto) {
            let steps = ctx.recover_clause(
                |_| Ok(None),
                |c| c.expect_integer_literal().map(|n| n as usize).map(Some),
            )?;
            if let Some(n) = steps {
                max_steps = Some(n);
                ctx.expect_token(TokenKind::Step)?;
            }
        }

        // Optional WEIGHT clause
        let weight_expression = if ctx.match_token(TokenKind::Weight) {
            ctx.recover_clause(|_| Ok(None), |c| c.expect_identifier().map(Some))?
        } else {
            None
        };

        // Optional WHERE clause
        let where_clause = if ctx.match_token(TokenKind::Where) {
            Some(ctx.recover_clause(Self::create_true_expression, |c| self.parse_expression(c))?)
        } else {
            Some(Self::create_true_expression(ctx)?)
        };

        // Optional YIELD clause
        let yield_clause = if ctx.match_token(TokenKind::Yield) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| ClauseParser::new().parse_yield_clause(c).map(Some),
            )?
        } else {
            None
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::FindPath(FindPathStmt {
            span,
            from: from_clause,
            to: to_vertex,
            over: Some(over),
            where_clause,
            shortest,
            max_steps,
            limit: None,
            skip: None,
            yield_clause,
            weight_expression,
            heuristic_expression: None,
            with_loop,
            with_cycle,
        }))
    }

    /// Analysis of the GET SUBGRAPH statement
    pub fn parse_subgraph_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Get)?;

        ctx.expect_token(TokenKind::Subgraph)?;

        let with_prop = ctx.match_token(TokenKind::With) && ctx.match_token(TokenKind::Prop);

        let _with_edge = if !with_prop {
            ctx.match_token(TokenKind::With) && ctx.match_token(TokenKind::Edge)
        } else {
            false
        };

        let steps = if matches!(ctx.current_token().kind, TokenKind::IntegerLiteral(_)) {
            // Canonical form: GET SUBGRAPH <n> STEPS FROM ...
            let n = ctx.expect_integer_literal()?;
            ctx.expect_token(TokenKind::Step)?;
            Steps::Fixed(n as usize)
        } else if ctx.match_token(TokenKind::Step) {
            // Alternate form: GET SUBGRAPH STEPS <n> FROM ...
            self.parse_steps(ctx)?
        } else {
            Steps::Fixed(1)
        };

        // Support optional FROM keyword for backward compatibility
        ctx.match_token(TokenKind::From);
        let from_span = ctx.current_span();
        let from_clause = ctx.recover_clause(
            |c| {
                Ok(FromClause {
                    span: c.current_span(),
                    vertices: Vec::new(),
                })
            },
            |c| {
                let vertices = self.parse_expression_list(c)?;
                Ok(FromClause {
                    span: from_span,
                    vertices,
                })
            },
        )?;

        let over = if ctx.match_token(TokenKind::Over) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| ClauseParser::new().parse_over_clause(c).map(Some),
            )?
        } else {
            None
        };

        let where_clause = if ctx.match_token(TokenKind::Where) {
            Some(ctx.recover_clause(Self::create_true_expression, |c| self.parse_expression(c))?)
        } else {
            Some(Self::create_true_expression(ctx)?)
        };

        let yield_clause = if ctx.match_token(TokenKind::Yield) {
            ctx.recover_clause(
                |_| Ok(None),
                |c| ClauseParser::new().parse_yield_clause(c).map(Some),
            )?
        } else {
            None
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Subgraph(SubgraphStmt {
            span,
            steps,
            from: from_clause,
            over,
            where_clause,
            yield_clause,
        }))
    }

    /// Analysis mode
    pub fn parse_pattern(&mut self, ctx: &mut ParseContext) -> Result<Pattern, ParseError> {
        let start_span = ctx.current_span();

        // Check whether it is in node mode (starting with ()).
        if ctx.match_token(TokenKind::LParen) {
            let node = self.parse_node_pattern(ctx, start_span)?;

            // Check whether there is a chain edge pattern.
            if ctx.check_token(TokenKind::LeftArrow)
                || ctx.check_token(TokenKind::RightArrow)
                || ctx.check_token(TokenKind::Minus)
                || ctx.check_token(TokenKind::Arrow)
                || ctx.check_token(TokenKind::BackArrow)
            {
                return self.parse_path_pattern(ctx, node);
            }

            return Ok(Pattern::Node(node));
        }

        // Check whether it is in variable mode.
        if let TokenKind::Identifier(ref name) = ctx.current_token().kind.clone() {
            let name = name.clone();
            let span = ctx.current_span();
            ctx.next_token();
            return Ok(Pattern::Variable(VariablePattern { span, name }));
        }

        Err(ParseError::new(
            ParseErrorKind::SyntaxError,
            "Expected pattern (node or path)".to_string(),
            ctx.current_position(),
        ))
    }

    /// Analyzing the node pattern
    fn parse_node_pattern(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::parser::ast::types::Span,
    ) -> Result<NodePattern, ParseError> {
        let mut variable = None;
        let mut labels = Vec::new();
        let mut properties = None;

        // Analyzing variable names (optional)
        if let TokenKind::Identifier(ref name) = ctx.current_token().kind.clone() {
            let name = name.clone();
            ctx.next_token();

            // A bare identifier without a trailing colon is a variable
            // reference with no tag restriction (`(n)` matches any tag);
            // use `(:label)` or `(var:label)` to restrict tags.
            variable = Some(name);
        }

        // Analyzing the tags
        if ctx.match_token(TokenKind::Colon) {
            // Parse the list of tags (multiple tags are supported, e.g.: Person:Actor)
            loop {
                let label = ctx.expect_identifier()?;
                labels.push(label);
                if !ctx.check_token(TokenKind::Colon) {
                    break;
                }
                ctx.next_token(); // Consume the next colon.
            }
        }

        // Parse attribute (optional)
        if ctx.match_token(TokenKind::LBrace) {
            properties = Some(self.parse_properties_expr(ctx)?);
            ctx.expect_token(TokenKind::RBrace)?;
        }

        // Expected a right parenthesis.
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

    /// Analyzing path patterns
    fn parse_path_pattern(
        &mut self,
        ctx: &mut ParseContext,
        start_node: NodePattern,
    ) -> Result<Pattern, ParseError> {
        let start_span = start_node.span;
        let mut elements = vec![PathElement::Node(start_node)];

        // Analyzing the chained structure of edges and nodes
        while ctx.check_token(TokenKind::LeftArrow)
            || ctx.check_token(TokenKind::RightArrow)
            || ctx.check_token(TokenKind::Minus)
            || ctx.check_token(TokenKind::Arrow)
            || ctx.check_token(TokenKind::BackArrow)
        {
            let edge = self.parse_edge_pattern(ctx)?;
            elements.push(PathElement::Edge(edge));

            // It is expected that a node follows.
            if ctx.match_token(TokenKind::LParen) {
                let node_span = ctx.current_span();
                let node = self.parse_node_pattern(ctx, node_span)?;
                elements.push(PathElement::Node(node));
            } else {
                break;
            }
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Pattern::Path(PathPattern { span, elements }))
    }

    /// Analyzing the border mode
    fn parse_edge_pattern(&mut self, ctx: &mut ParseContext) -> Result<EdgePattern, ParseError> {
        let start_span = ctx.current_span();
        let mut direction = EdgeDirection::Out;

        if ctx.match_token(TokenKind::BackArrow) || ctx.match_token(TokenKind::LeftArrow) {
            direction = EdgeDirection::In;
        }

        ctx.expect_token(TokenKind::Minus)?;

        let mut variable = None;
        let mut edge_types = Vec::new();
        let mut properties = None;
        let mut range = None;

        if ctx.match_token(TokenKind::LBracket) {
            if let TokenKind::Identifier(ref name) = ctx.current_token().kind.clone() {
                let name = name.clone();
                ctx.next_token();

                if ctx.check_token(TokenKind::Colon) {
                    variable = Some(name);
                } else {
                    edge_types.push(name);
                }
            }

            if ctx.match_token(TokenKind::Colon) {
                loop {
                    // Handle optional colon before edge type (e.g., :KNOWS|:FOLLOWS or :KNOWS|FOLLOWS)
                    ctx.match_token(TokenKind::Colon);
                    let edge_type = ctx.expect_identifier()?;
                    edge_types.push(edge_type);
                    if !ctx.match_token(TokenKind::Pipe) {
                        break;
                    }
                }
            }

            if ctx.match_token(TokenKind::LBrace) {
                properties = Some(self.parse_properties_expr(ctx)?);
                ctx.expect_token(TokenKind::RBrace)?;
            }

            if ctx.match_token(TokenKind::Star) {
                if ctx.match_token(TokenKind::LBracket) {
                    let min = if matches!(ctx.current_token().kind, TokenKind::IntegerLiteral(_)) {
                        let n = ctx.expect_integer_literal()? as usize;
                        Some(n)
                    } else {
                        None
                    };

                    if ctx.match_token(TokenKind::DotDot) {
                        let max =
                            if matches!(ctx.current_token().kind, TokenKind::IntegerLiteral(_)) {
                                let n = ctx.expect_integer_literal()? as usize;
                                Some(n)
                            } else {
                                None
                            };
                        range = Some(EdgeRange::new(min, max));
                    } else if let Some(min_val) = min {
                        range = Some(EdgeRange::fixed(min_val));
                    } else {
                        range = Some(EdgeRange::any());
                    }

                    ctx.expect_token(TokenKind::RBracket)?;
                } else if matches!(ctx.current_token().kind, TokenKind::IntegerLiteral(_)) {
                    let min = ctx.expect_integer_literal()? as usize;
                    if ctx.match_token(TokenKind::DotDot) {
                        let max =
                            if matches!(ctx.current_token().kind, TokenKind::IntegerLiteral(_)) {
                                let n = ctx.expect_integer_literal()? as usize;
                                Some(n)
                            } else {
                                None
                            };
                        range = Some(EdgeRange::new(Some(min), max));
                    } else {
                        range = Some(EdgeRange::fixed(min));
                    }
                } else {
                    range = Some(EdgeRange::any());
                }
            }

            ctx.expect_token(TokenKind::RBracket)?;
        }

        if ctx.match_token(TokenKind::Arrow) {
            if direction == EdgeDirection::In {
                direction = EdgeDirection::Both;
            } else {
                direction = EdgeDirection::Out;
            }
        } else if ctx.match_token(TokenKind::Minus) {
            if direction == EdgeDirection::Out {
                direction = EdgeDirection::Both;
            }
        } else if ctx.match_token(TokenKind::RightArrow) {
            if direction == EdgeDirection::In {
                direction = EdgeDirection::Both;
            } else {
                direction = EdgeDirection::Out;
            }
        } else {
            return Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                "Expected '-', '->', or '<-' after edge pattern".to_string(),
                ctx.current_position(),
            ));
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(EdgePattern {
            span,
            variable,
            edge_types,
            properties,
            predicates: Vec::new(),
            direction,
            range,
        })
    }

    /// Analysis steps
    fn parse_steps(&mut self, ctx: &mut ParseContext) -> Result<Steps, ParseError> {
        // Try to parse the numbers or ranges.
        let token = ctx.current_token();
        match token.kind {
            TokenKind::IntegerLiteral(n) => {
                ctx.next_token();
                Ok(Steps::Fixed(n as usize))
            }
            _ => {
                // Default: 1 step
                Ok(Steps::Fixed(1))
            }
        }
    }

    /// Parse a list of expressions
    fn parse_expression_list(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Vec<ContextualExpression>, ParseError> {
        let mut expressions = Vec::new();

        loop {
            expressions.push(self.parse_expression(ctx)?);
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }

        Ok(expressions)
    }

    /// Analyzing attribute expressions
    fn parse_properties_expr(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ContextualExpression, ParseError> {
        let mut properties = Vec::new();

        while !ctx.check_token(TokenKind::RBrace) {
            let key = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::Colon)?;
            let value = self.parse_expression(ctx)?;
            properties.push((key, value));
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }

        let mut mapped_properties = Vec::new();
        for (k, v) in properties {
            let v_expr = v
                .expression()
                .ok_or_else(|| {
                    ParseError::new_simple(
                        "Expression not registered in context".to_string(),
                        ctx.current_position(),
                    )
                })?
                .inner()
                .clone();
            mapped_properties.push((k, v_expr));
        }

        let expr = CoreExpression::map(mapped_properties);

        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.expression_context().register_expression(expr_meta);
        Ok(ContextualExpression::new(
            id,
            ctx.expression_context_clone(),
        ))
    }

    /// Create the default true expression.
    fn create_true_expression(ctx: &mut ParseContext) -> Result<ContextualExpression, ParseError> {
        let expr = CoreExpression::literal(true);
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.expression_context().register_expression(expr_meta);
        Ok(ContextualExpression::new(
            id,
            ctx.expression_context_clone(),
        ))
    }

    /// Analyzing the expression
    fn parse_expression(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ContextualExpression, ParseError> {
        parse_expression_with_context(ctx, ctx.expression_context_clone())
    }
}

impl Default for TraversalParser {
    fn default() -> Self {
        Self::new()
    }
}
