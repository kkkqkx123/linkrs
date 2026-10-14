use super::*;
use crate::parser::parsing::pattern_common;

impl DmlParser {
    /// Parse the Cypher-style CREATE data statement (the CREATE token has already been consumed)
    /// Support for grammar:
    ///   CREATE (n:Label {prop: value})
    ///   CREATE (a)-[:Type {prop: value}]->(b)
    ///   CREATE (a:Label1)-[:Type]->(b:Label2)
    pub fn parse_create_data_after_token(
        &mut self,
        ctx: &mut ParseContext,
        start_span: crate::parser::ast::types::Span,
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

        let span = ctx.span_since(start_span.start);

        Ok(Stmt::Create(CreateStmt {
            span,
            target: CreateTarget::Path { patterns },
            if_not_exists: false,
        }))
    }

    /// Parse a Cypher data pattern: a node followed by any number of edge
    /// hops (`-[...]->`, `<-[...]-`, `-[...]-`), mirroring how the
    /// traversal parser spells edges in MATCH.
    fn parse_create_pattern(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<crate::parser::ast::pattern::Pattern, ParseError> {
        use crate::parser::ast::pattern::*;

        let start_node = self.parse_node_pattern(ctx)?;

        // An edge hop starts with an optional `<-` (BackArrow) followed by a
        // mandatory `-` (Minus), so only those tokens continue the path.
        // The braceless `(a)->(b)` form carries no edge body and stays
        // unsupported, matching the traversal parser.
        if !matches!(
            ctx.current_token().kind,
            TokenKind::Minus | TokenKind::BackArrow
        ) {
            return Ok(Pattern::Node(start_node));
        }

        let start = start_node.span.start;
        let mut elements = vec![PathElement::Node(start_node)];
        while matches!(
            ctx.current_token().kind,
            TokenKind::Minus | TokenKind::BackArrow
        ) {
            let edge = self.parse_edge_pattern(ctx)?;
            elements.push(PathElement::CreateEdge(edge));

            let node = self.parse_node_pattern(ctx)?;
            elements.push(PathElement::Node(node));
        }

        let end_span = ctx.current_span();
        Ok(Pattern::Path(PathPattern {
            span: ctx.merge_span(start, end_span.end),
            elements,
            name: None,
        }))
    }

    /// Parse node pattern: (var:Label {prop: value})
    fn parse_node_pattern(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<crate::parser::ast::pattern::NodePattern, ParseError> {
        use crate::parser::ast::pattern::*;

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
                let label = ctx.expect_identifier()?;
                pattern_common::reject_qualified_reference(ctx, &label)?;
                labels.push(label);
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

        let span = ctx.span_since(start_span.start);

        Ok(NodePattern {
            span,
            variable,
            labels,
            properties,
            predicates: Vec::new(),
        })
    }

    /// Parse edge pattern: -[:Type {prop: value}]-> or <-[:Type {prop: value}]-
    /// Parse a Cypher edge pattern: an optional incoming `<-` (BackArrow),
    /// a mandatory `-` (Minus), the bracketed edge body, and a mandatory
    /// trailing direction. This is the same grammar the traversal parser
    /// uses for MATCH edges.
    fn parse_edge_pattern(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<crate::parser::ast::pattern::CreateEdgePattern, ParseError> {
        use crate::parser::ast::pattern::*;

        let start_span = ctx.current_span();

        let incoming =
            ctx.match_token(TokenKind::BackArrow) || ctx.match_token(TokenKind::LeftArrow);
        ctx.expect_token(TokenKind::Minus)?;

        ctx.expect_token(TokenKind::LBracket)?;

        let variable = if ctx.current_token().kind.is_identifier() {
            Some(ctx.expect_identifier()?)
        } else {
            None
        };

        let mut edge_types = Vec::new();
        if ctx.match_token(TokenKind::Colon) {
            let edge_type = ctx.expect_identifier()?;
            pattern_common::reject_qualified_reference(ctx, &edge_type)?;
            edge_types.push(edge_type);
        }

        let properties = if ctx.match_token(TokenKind::LBrace) {
            let props = self.parse_property_map(ctx)?;
            ctx.expect_token(TokenKind::RBrace)?;
            Some(props)
        } else {
            None
        };

        ctx.expect_token(TokenKind::RBracket)?;

        // The trailing direction closes the hop: `->` or `-`.
        let closing = ctx.current_token().kind.clone();
        let direction =
            pattern_common::resolve_edge_direction(incoming, &closing).ok_or_else(|| {
                ParseError::new(
                    crate::parser::core::error::ParseErrorKind::SyntaxError,
                    "Expected '-', '->', or '<-' after edge pattern".to_string(),
                    ctx.current_position(),
                )
            })?;
        ctx.next_token();

        let span = ctx.span_since(start_span.start);

        Ok(CreateEdgePattern {
            span,
            variable,
            edge_types,
            properties,
            direction,
        })
    }
}
