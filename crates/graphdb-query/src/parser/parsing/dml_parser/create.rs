use super::*;

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
    ) -> Result<crate::parser::ast::pattern::Pattern, ParseError> {
        use crate::parser::ast::pattern::*;

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
            Ok(Pattern::Path(PathPattern {
                span,
                elements,
                name: None,
            }))
        } else {
            Ok(Pattern::Node(start_node))
        }
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
                if ctx.match_token(TokenKind::Dot) {
                    let table = ctx.expect_identifier()?;
                    let pos = ctx.current_position();
                    return Err(ParseError::new(
                        crate::parser::core::error::ParseErrorKind::UnexpectedToken,
                        crate::attached::qualified_reference_message(&label, &table),
                        pos,
                    ));
                }
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
    ) -> Result<crate::parser::ast::pattern::EdgePattern, ParseError> {
        use crate::parser::ast::pattern::*;

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
            let edge_type = ctx.expect_identifier()?;
            if ctx.match_token(TokenKind::Dot) {
                let table = ctx.expect_identifier()?;
                let pos = ctx.current_position();
                return Err(ParseError::new(
                    crate::parser::core::error::ParseErrorKind::UnexpectedToken,
                    crate::attached::qualified_reference_message(&edge_type, &table),
                    pos,
                ));
            }
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
            path_semantic: None,
            recursive_comprehension: None,
        })
    }
}
