use super::*;

impl DmlParser {
    /// Parse COPY statement
    /// Syntax:
    ///   COPY VERTEX <tag> FROM 'path' [WITH (HEADER [true|false], DELIMITER ',' , BATCH_SIZE n)]
    ///   COPY EDGE <edge_type> FROM 'path' [WITH ...]
    ///   COPY <tag> FROM 'path' // defaults to VERTEX
    pub fn parse_copy_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        use crate::parser::ast::stmt::{CopyDirection, CopyStmt, CopyTarget};

        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Copy)?;

        // Parse optional VERTEX/EDGE keyword and target name
        let target = if ctx.match_token(TokenKind::Vertex) {
            let tag = ctx.expect_identifier()?;
            CopyTarget::Vertex(tag)
        } else if ctx.match_token(TokenKind::Edge) {
            let edge = ctx.expect_identifier()?;
            CopyTarget::Edge(edge)
        } else if ctx.match_token(TokenKind::Tag) {
            let tag = ctx.expect_identifier()?;
            CopyTarget::Vertex(tag)
        } else {
            // Bare identifier defaults to vertex tag
            let name = ctx.expect_identifier()?;
            CopyTarget::Vertex(name)
        };

        let direction = if ctx.match_token(TokenKind::To) {
            CopyDirection::To
        } else {
            ctx.expect_token(TokenKind::From)?;
            CopyDirection::From
        };
        let mut file_paths = Vec::new();
        if ctx.check_token(TokenKind::LParen) {
            ctx.expect_token(TokenKind::LParen)?;
            loop {
                if ctx.check_token(TokenKind::RParen) {
                    ctx.next_token();
                    break;
                }
                file_paths.push(ctx.expect_string_literal()?);
                if ctx.match_token(TokenKind::Comma) {
                    continue;
                }
                ctx.expect_token(TokenKind::RParen)?;
                break;
            }
            if file_paths.is_empty() {
                return Err(ParseError::new(
                    crate::parser::core::error::ParseErrorKind::SyntaxError,
                    "COPY FROM file list must contain at least one file".to_string(),
                    ctx.current_position(),
                ));
            }
        } else {
            file_paths.push(ctx.expect_string_literal()?);
        }
        // Optional `BY COLUMN` marker right after the file list: column-wise
        // merge for multi-file imports. Also accepted as a trailing marker
        // after the option list below.
        let mut by_column = false;
        if ctx.check_token(TokenKind::By) || ctx.check_keyword("BY") {
            if !ctx.match_token(TokenKind::By) {
                let _ = ctx.consume_keyword("BY");
            }
            if ctx.check_keyword("COLUMN") {
                let _ = ctx.consume_keyword("COLUMN");
                by_column = true;
            } else {
                return Err(ParseError::new(
                    crate::parser::core::error::ParseErrorKind::SyntaxError,
                    "Expected COLUMN after BY in COPY statement".to_string(),
                    ctx.current_position(),
                ));
            }
        }
        if direction == CopyDirection::To && file_paths.len() > 1 {
            return Err(ParseError::new(
                crate::parser::core::error::ParseErrorKind::SyntaxError,
                "COPY TO supports a single file path".to_string(),
                ctx.current_position(),
            ));
        }
        if direction == CopyDirection::To && by_column {
            return Err(ParseError::new(
                crate::parser::core::error::ParseErrorKind::SyntaxError,
                "BY COLUMN is only valid for COPY FROM".to_string(),
                ctx.current_position(),
            ));
        }

        // Defaults
        let mut header = true;
        let mut delimiter = ',';
        let mut batch_size: Option<usize> = None;

        // Consume the option name token (dedicated keyword or bare identifier).
        fn consume_option_name(ctx: &mut ParseContext, kind: TokenKind, name: &str) {
            if !ctx.match_token(kind) {
                let _ = ctx.consume_keyword(name);
            }
        }

        // Consume an optional `=` / `EQ` between an option name and its value.
        fn consume_optional_assign(ctx: &mut ParseContext) {
            if !ctx.match_token(TokenKind::Assign) {
                let _ = ctx.match_token(TokenKind::Eq);
            }
        }

        let parse_delimiter_value = |ctx: &mut ParseContext| -> Result<char, ParseError> {
            consume_optional_assign(ctx);
            match ctx.current_token().kind.clone() {
                TokenKind::StringLiteral(s) | TokenKind::Identifier(s) => {
                    ctx.next_token();
                    s.chars().next().ok_or_else(|| {
                        ParseError::new(
                            crate::parser::core::error::ParseErrorKind::SyntaxError,
                            "DELIMITER must be a single character".to_string(),
                            ctx.current_position(),
                        )
                    })
                }
                other => Err(ParseError::new(
                    crate::parser::core::error::ParseErrorKind::UnexpectedToken,
                    format!("Expected delimiter string, found {other:?}"),
                    ctx.current_position(),
                )),
            }
        };

        let parse_bool_value = |ctx: &mut ParseContext| -> Option<bool> {
            match ctx.current_token().kind.clone() {
                TokenKind::BooleanLiteral(b) => {
                    ctx.next_token();
                    Some(b)
                }
                TokenKind::Identifier(s) if s.eq_ignore_ascii_case("true") => {
                    ctx.next_token();
                    Some(true)
                }
                TokenKind::Identifier(s) if s.eq_ignore_ascii_case("false") => {
                    ctx.next_token();
                    Some(false)
                }
                _ => None,
            }
        };

        // Option list: `WITH (OPT v, OPT v)` / `WITH OPT OPT ...` / `(OPT, ...)`.
        let mut in_parens = false;
        if (ctx.match_token(TokenKind::With) || !ctx.check_token(TokenKind::With))
            && ctx.match_token(TokenKind::LParen)
        {
            in_parens = true;
        }

        loop {
            if ctx.match_token(TokenKind::Comma) {
                continue;
            }
            if in_parens && ctx.match_token(TokenKind::RParen) {
                break;
            }

            if ctx.check_token(TokenKind::Header) || ctx.check_keyword("HEADER") {
                consume_option_name(ctx, TokenKind::Header, "HEADER");
                header = parse_bool_value(ctx).unwrap_or(true);
                continue;
            }
            if ctx.check_token(TokenKind::Delimiter) || ctx.check_keyword("DELIMITER") {
                consume_option_name(ctx, TokenKind::Delimiter, "DELIMITER");
                delimiter = parse_delimiter_value(ctx)?;
                continue;
            }
            if ctx.check_keyword("BATCH_SIZE") || ctx.check_keyword("BATCH") {
                if ctx.check_keyword("BATCH_SIZE") {
                    let _ = ctx.consume_keyword("BATCH_SIZE");
                } else {
                    let _ = ctx.consume_keyword("BATCH");
                    let _ = ctx.consume_keyword("SIZE");
                }
                consume_optional_assign(ctx);
                let batch = ctx.expect_integer_literal()?;
                if batch < 0 {
                    return Err(ParseError::new(
                        crate::parser::core::error::ParseErrorKind::SyntaxError,
                        "BATCH_SIZE must be positive".to_string(),
                        ctx.current_position(),
                    ));
                }
                batch_size = Some(batch as usize);
                continue;
            }
            if ctx.check_token(TokenKind::Csv) || ctx.check_keyword("CSV") {
                consume_option_name(ctx, TokenKind::Csv, "CSV");
                continue;
            }
            // `NO HEADER` is the explicit spelling of `HEADER false`.
            if ctx.check_token(TokenKind::No) || ctx.check_keyword("NO") {
                ctx.next_token();
                if ctx.check_token(TokenKind::Header) || ctx.check_keyword("HEADER") {
                    consume_option_name(ctx, TokenKind::Header, "HEADER");
                    header = false;
                    continue;
                }
                break;
            }

            if in_parens {
                if matches!(
                    ctx.current_token().kind,
                    TokenKind::Eof | TokenKind::Semicolon
                ) {
                    break;
                }
                // Unknown option inside parens: stop consuming rather than
                // guessing; the statement-level parser reports the leftover.
                break;
            }
            // A second `WITH` continues the option list (`WITH a WITH b`).
            if ctx.match_token(TokenKind::With) {
                if ctx.match_token(TokenKind::LParen) {
                    in_parens = true;
                }
                continue;
            }
            break;
        }

        // Trailing `BY COLUMN` marker after the option list.
        if !by_column && (ctx.check_token(TokenKind::By) || ctx.check_keyword("BY")) {
            if !ctx.match_token(TokenKind::By) {
                let _ = ctx.consume_keyword("BY");
            }
            if ctx.check_keyword("COLUMN") {
                let _ = ctx.consume_keyword("COLUMN");
                by_column = true;
            } else {
                return Err(ParseError::new(
                    crate::parser::core::error::ParseErrorKind::SyntaxError,
                    "Expected COLUMN after BY in COPY statement".to_string(),
                    ctx.current_position(),
                ));
            }
        }
        if direction == CopyDirection::To && by_column {
            return Err(ParseError::new(
                crate::parser::core::error::ParseErrorKind::SyntaxError,
                "BY COLUMN is only valid for COPY FROM".to_string(),
                ctx.current_position(),
            ));
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Copy(CopyStmt {
            span,
            target,
            direction,
            file_paths,
            by_column,
            header,
            delimiter,
            batch_size,
        }))
    }
}
