//! Graph Traversal Statement Parsing Module
//!
//! Responsible for parsing statements related to graph traversal, including MATCH, GO, FIND PATH, GET SUBGRAPH, etc.

use crate::parser::ast::hint::JoinHintAst;
use crate::parser::ast::pattern::{
    EdgePattern, EdgeRange, NodePattern, PathElement, PathPattern, Pattern, VariablePattern,
};
use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::expr_parser::parse_expression_with_context;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::parsing::pattern_common;
use crate::parser::TokenKind;
use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::types::expr::Expression as CoreExpression;

mod find_path;
mod get_subgraph;
mod go;
mod r#match;

/// Graph Traversal Parser
pub struct TraversalParser;

/// The body of one MATCH clause, used when merging consecutive MATCH clauses.
pub(crate) struct ParsedMatchClause {
    patterns: Vec<Pattern>,
    join_hint: Option<JoinHintAst>,
    where_clause: Option<ContextualExpression>,
    where_explicit: bool,
    return_clause: Option<ReturnClause>,
    delete_clause: Option<MatchDeleteClause>,
}

impl TraversalParser {
    pub fn new() -> Self {
        Self
    }

    /// Analysis mode
    pub fn parse_pattern(&mut self, ctx: &mut ParseContext) -> Result<Pattern, ParseError> {
        let start_span = ctx.current_span();

        if let TokenKind::Identifier(_) = ctx.current_token().kind {
            let ckpt = ctx.checkpoint();
            if let Ok(candidate) = ctx.expect_identifier() {
                if ctx.match_token(TokenKind::Assign) {
                    return self.parse_named_path(ctx, candidate, start_span);
                }
            }
            ctx.restore(ckpt);
        }

        self.parse_unnamed_pattern(ctx, start_span)
    }

    /// Parse the remainder of a named path binding after `name =`.
    fn parse_named_path(
        &mut self,
        ctx: &mut ParseContext,
        name: String,
        start_span: crate::parser::ast::types::Span,
    ) -> Result<Pattern, ParseError> {
        match self.parse_unnamed_pattern(ctx, ctx.current_span())? {
            Pattern::Path(mut path) => {
                if path.name.is_some() {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        "Duplicate path name in named path binding".to_string(),
                        ctx.current_position(),
                    ));
                }
                path.name = Some(name);
                Ok(Pattern::Path(path))
            }
            Pattern::Node(node) => {
                let span = ctx.span_since(start_span.start);
                Ok(Pattern::Path(PathPattern::with_name(
                    vec![PathElement::Node(node)],
                    span,
                    name,
                )))
            }
            _ => Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                "Expected a node or path pattern after '=' in named path binding".to_string(),
                ctx.current_position(),
            )),
        }
    }

    /// Parse a plain pattern without a leading path-name binding.
    fn parse_unnamed_pattern(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::parser::ast::types::Span,
    ) -> Result<Pattern, ParseError> {
        if ctx.match_token(TokenKind::LParen) {
            let node = self.parse_node_pattern(ctx, start_span)?;

            if ctx.check_token(TokenKind::Minus)
                || ctx.check_token(TokenKind::Arrow)
                || ctx.check_token(TokenKind::BackArrow)
            {
                return self.parse_path_pattern(ctx, node);
            }

            return Ok(Pattern::Node(node));
        }

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

        if let TokenKind::Identifier(ref name) = ctx.current_token().kind.clone() {
            let name = name.clone();
            ctx.next_token();
            variable = Some(name);
        }

        if ctx.match_token(TokenKind::Colon) {
            loop {
                let label = ctx.expect_identifier()?;
                pattern_common::reject_qualified_reference(ctx, &label)?;
                labels.push(label);
                if !ctx.check_token(TokenKind::Colon) {
                    break;
                }
                ctx.next_token();
            }
        }

        if ctx.match_token(TokenKind::LBrace) {
            properties = Some(self.parse_properties_expr(ctx)?);
            ctx.expect_token(TokenKind::RBrace)?;
        }

        ctx.expect_token(TokenKind::RParen)?;

        let span = ctx.span_since(start_span.start);

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

        while ctx.check_token(TokenKind::Minus)
            || ctx.check_token(TokenKind::Arrow)
            || ctx.check_token(TokenKind::BackArrow)
        {
            let edge = self.parse_edge_pattern(ctx)?;
            elements.push(PathElement::Edge(edge));

            if ctx.match_token(TokenKind::LParen) {
                let node_span = ctx.current_span();
                let node = self.parse_node_pattern(ctx, node_span)?;
                elements.push(PathElement::Node(node));
            } else {
                break;
            }
        }

        let span = ctx.span_since(start_span.start);

        Ok(Pattern::Path(PathPattern {
            span,
            elements,
            name: None,
        }))
    }

    /// Analyzing the border mode
    fn parse_edge_pattern(&mut self, ctx: &mut ParseContext) -> Result<EdgePattern, ParseError> {
        let start_span = ctx.current_span();
        let incoming = ctx.match_token(TokenKind::BackArrow);

        ctx.expect_token(TokenKind::Minus)?;

        let mut variable = None;
        let mut edge_types = Vec::new();
        let mut properties = None;
        let mut range = None;
        let mut path_semantic = None;

        if ctx.match_token(TokenKind::LBracket) {
            if let TokenKind::Identifier(ref name) = ctx.current_token().kind.clone() {
                let name = name.clone();
                ctx.next_token();

                if ctx.check_token(TokenKind::Colon) {
                    variable = Some(name);
                } else {
                    pattern_common::reject_qualified_reference(ctx, &name)?;
                    edge_types.push(name);
                }
            }

            if ctx.match_token(TokenKind::Colon) {
                loop {
                    ctx.match_token(TokenKind::Colon);
                    let edge_type = ctx.expect_identifier()?;
                    pattern_common::reject_qualified_reference(ctx, &edge_type)?;
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
                if ctx.match_token(TokenKind::Trail) {
                    path_semantic = Some(PathSemantic::Trail);
                    range = Some(EdgeRange::any());
                } else if ctx.match_token(TokenKind::Acyclic) {
                    path_semantic = Some(PathSemantic::Acyclic);
                    range = Some(EdgeRange::any());
                } else if ctx.match_token(TokenKind::Shortest) {
                    path_semantic = Some(PathSemantic::Shortest);
                    range = Some(EdgeRange::any());
                } else if ctx.match_token(TokenKind::AllShortestPaths)
                    || (ctx.match_token(TokenKind::All) && ctx.match_token(TokenKind::Shortest))
                {
                    path_semantic = Some(PathSemantic::AllShortest);
                    range = Some(EdgeRange::any());
                } else if ctx.match_token(TokenKind::Weighted) {
                    ctx.expect_token(TokenKind::LParen)?;
                    let weight_prop = ctx.expect_identifier()?;
                    ctx.expect_token(TokenKind::RParen)?;
                    path_semantic = Some(PathSemantic::WeightedShortest(weight_prop));
                    range = Some(EdgeRange::any());
                } else if ctx.match_token(TokenKind::LParen) {
                    return Err(ParseError::new(
                        ParseErrorKind::UnsupportedFeature,
                        "Recursive comprehension patterns are not supported".to_string(),
                        ctx.current_position(),
                    ));
                } else if ctx.match_token(TokenKind::LBracket) {
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

        // The trailing direction closes the hop: `->` or `-`.
        let closing = ctx.current_token().kind.clone();
        let direction =
            pattern_common::resolve_edge_direction(incoming, &closing).ok_or_else(|| {
                ParseError::new(
                    ParseErrorKind::SyntaxError,
                    "Expected '-', '->', or '<-' after edge pattern".to_string(),
                    ctx.current_position(),
                )
            })?;
        ctx.next_token();

        let span = ctx.span_since(start_span.start);

        Ok(EdgePattern {
            span,
            variable,
            edge_types,
            properties,
            predicates: Vec::new(),
            direction,
            range,
            path_semantic,
            recursive_comprehension: ctx.take_recursive_comprehension(),
        })
    }

    /// Analysis steps
    fn parse_steps(&mut self, ctx: &mut ParseContext) -> Result<Steps, ParseError> {
        let token = ctx.current_token();
        match token.kind {
            TokenKind::IntegerLiteral(n) => {
                ctx.next_token();
                Ok(Steps::Fixed(n as usize))
            }
            TokenKind::Step | TokenKind::From => Ok(Steps::Fixed(1)),
            _ => Err(ParseError::new(
                ParseErrorKind::UnsupportedFeature,
                "GO step count must be an integer literal".to_string(),
                ctx.current_position(),
            )),
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

        let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.expression_context().register_expression(expr_meta);
        Ok(ContextualExpression::new(
            id,
            ctx.expression_context_clone(),
        ))
    }

    /// Create the default true expression.
    fn create_true_expression(ctx: &mut ParseContext) -> Result<ContextualExpression, ParseError> {
        let expr = CoreExpression::literal(true);
        let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
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
