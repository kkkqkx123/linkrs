use super::*;

impl DmlParser {
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

        // Consume the shared `ON` token once, then branch on CREATE vs MATCH.
        // Both clauses may appear (each at most once, in any order), mirroring
        // Cypher: `MERGE ... ON CREATE SET ... ON MATCH SET ...`.
        let (mut on_create, mut on_match) = if ctx.match_token(TokenKind::On) {
            if ctx.match_token(TokenKind::Create) {
                (Some(Self::parse_merge_set_clause(ctx)?), None)
            } else if ctx.match_token(TokenKind::Match) {
                (None, Some(Self::parse_merge_set_clause(ctx)?))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };
        if ctx.match_token(TokenKind::On) {
            if ctx.match_token(TokenKind::Create) {
                if on_create.is_some() {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        "Duplicate ON CREATE clause in MERGE statement".to_string(),
                        ctx.current_position(),
                    ));
                }
                on_create = Some(Self::parse_merge_set_clause(ctx)?);
            } else if ctx.match_token(TokenKind::Match) {
                if on_match.is_some() {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        "Duplicate ON MATCH clause in MERGE statement".to_string(),
                        ctx.current_position(),
                    ));
                }
                on_match = Some(Self::parse_merge_set_clause(ctx)?);
            } else {
                return Err(ParseError::new(
                    ParseErrorKind::SyntaxError,
                    "Expected CREATE or MATCH after ON in MERGE statement".to_string(),
                    ctx.current_position(),
                ));
            }
        }

        // A bare `SET ...` clause following the pattern (e.g.
        // `MERGE (v:Tag {..}) SET v.prop = ..`) applies to both the matched
        // and the created branch, mirroring the post-merge update semantics.
        // Its assignments are merged into any ON CREATE / ON MATCH clause
        // already present so no parsed assignment is silently discarded.
        if ctx.match_token(TokenKind::Set) {
            let bare = ClauseParser::new().parse_set_clause(ctx)?;
            if !bare.assignments.is_empty() {
                if let Some(clause) = on_create.as_mut() {
                    clause.assignments.extend(bare.assignments.iter().cloned());
                } else {
                    on_create = Some(bare.clone());
                }
                if let Some(clause) = on_match.as_mut() {
                    clause.assignments.extend(bare.assignments);
                } else {
                    on_match = Some(bare);
                }
            }
        }

        Ok(Stmt::Merge(MergeStmt {
            span: start_span,
            pattern,
            on_create,
            on_match,
        }))
    }

    /// Parse the SET clause following ON CREATE / ON MATCH in a MERGE statement.
    ///
    /// Consumes the optional `SET` keyword before delegating to the clause
    /// parser, matching how `parse_update_after_token` handles SET clauses.
    fn parse_merge_set_clause(ctx: &mut ParseContext) -> Result<SetClause, ParseError> {
        if ctx.match_token(TokenKind::Set) {
            ClauseParser::new().parse_set_clause(ctx)
        } else {
            Ok(SetClause {
                span: ctx.current_span(),
                assignments: Vec::new(),
            })
        }
    }
}
