use super::parse_expression;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;
use graphdb_core::types::expr::{Expression, SubqueryBody};

pub(crate) fn parse_sql_subquery_body(
    ctx: &mut ParseContext<'_>,
) -> Result<SubqueryBody, ParseError> {
    ctx.next_token(); // SELECT (lexed as an identifier, not a keyword)

    // SELECT items: the first expression is the subquery return expression,
    // additional comma-separated items are consumed for syntax validation.
    let first_item = parse_expression(ctx)?;
    while ctx.match_token(TokenKind::Comma) {
        parse_expression(ctx)?;
    }

    ctx.expect_token(TokenKind::From)?;
    let tag = ctx.expect_identifier()?;

    // `SELECT col FROM tag` maps to a MATCH-style subquery over the tag:
    // pattern `(tag)`, and bare identifiers in SELECT / WHERE are rewritten
    // into property accesses on the tag variable (`col` -> `tag.col`).
    let pattern_str = format!("({})", tag);
    let return_expr = Some(Box::new(rewrite_sql_identifiers(first_item.expr, &tag)));

    let where_clause = if ctx.match_token(TokenKind::Where) {
        let expr = parse_expression(ctx)?;
        Some(Box::new(rewrite_sql_identifiers(expr.expr, &tag)))
    } else {
        None
    };

    Ok(SubqueryBody {
        id: 0,
        patterns: vec![pattern_str],
        where_clause,
        return_expr,
    })
}

/// Rewrite bare variable references inside a SQL-style subquery SELECT /
/// WHERE expression into property accesses on the FROM tag variable.
///
/// `SELECT age FROM person WHERE name = 'Alice'` becomes
/// `RETURN person.age WHERE person.name = 'Alice'`.
pub(crate) fn rewrite_sql_identifiers(expr: Expression, tag: &str) -> Expression {
    fn walk(e: Expression, tag: &str) -> Expression {
        match e {
            Expression::Variable(name) => Expression::property(Expression::variable(tag), name),
            Expression::Property { object, property } => Expression::Property {
                object: Box::new(walk(*object, tag)),
                property,
            },
            Expression::StructField { base, field } => Expression::StructField {
                base: Box::new(walk(*base, tag)),
                field,
            },
            Expression::Subscript { collection, index } => Expression::Subscript {
                collection: Box::new(walk(*collection, tag)),
                index: Box::new(walk(*index, tag)),
            },
            Expression::Range {
                collection,
                start,
                end,
            } => Expression::Range {
                collection: Box::new(walk(*collection, tag)),
                start: start.map(|s| Box::new(walk(*s, tag))),
                end: end.map(|e| Box::new(walk(*e, tag))),
            },
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => Expression::ListComprehension {
                variable,
                source: Box::new(walk(*source, tag)),
                filter: filter.map(|f| Box::new(walk(*f, tag))),
                map: map.map(|m| Box::new(walk(*m, tag))),
            },
            Expression::Unary { op, operand } => Expression::Unary {
                op,
                operand: Box::new(walk(*operand, tag)),
            },
            Expression::Binary { left, op, right } => Expression::Binary {
                left: Box::new(walk(*left, tag)),
                op,
                right: Box::new(walk(*right, tag)),
            },
            Expression::Function { name, args } => Expression::Function {
                name,
                args: args.into_iter().map(|a| walk(a, tag)).collect(),
            },
            Expression::Aggregate {
                func,
                args,
                distinct,
                filter,
            } => Expression::Aggregate {
                func,
                args: args.into_iter().map(|a| walk(a, tag)).collect(),
                distinct,
                filter: filter.map(|f| Box::new(walk(*f, tag))),
            },
            Expression::List(items) => {
                Expression::List(items.into_iter().map(|i| walk(i, tag)).collect())
            }
            Expression::Map(pairs) => {
                Expression::Map(pairs.into_iter().map(|(k, v)| (k, walk(v, tag))).collect())
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => Expression::Case {
                test_expr: test_expr.map(|t| Box::new(walk(*t, tag))),
                conditions: conditions
                    .into_iter()
                    .map(|(c, r)| (walk(c, tag), walk(r, tag)))
                    .collect(),
                default: default.map(|d| Box::new(walk(*d, tag))),
            },
            Expression::TypeCast {
                expression,
                target_type,
            } => Expression::TypeCast {
                expression: Box::new(walk(*expression, tag)),
                target_type,
            },
            other => other,
        }
    }
    walk(expr, tag)
}

pub(crate) fn parse_subquery_body(ctx: &mut ParseContext<'_>) -> Result<SubqueryBody, ParseError> {
    let mut patterns = Vec::new();
    let mut where_clause = None;
    let mut return_expr = None;

    if ctx.match_token(TokenKind::Match) {
        let pattern_str = parse_pattern_string(ctx)?;
        patterns.push(pattern_str);
    } else if !matches!(
        ctx.current_token().kind,
        TokenKind::Where | TokenKind::Return | TokenKind::RBrace
    ) {
        // Bare pattern without the MATCH keyword:
        // `EXISTS { a:person-[:knows]->b:person }`.
        let pattern_str = parse_pattern_string(ctx)?;
        patterns.push(pattern_str);
    }

    if ctx.match_token(TokenKind::Where) {
        let expr = parse_expression(ctx)?;
        where_clause = Some(Box::new(expr.expr));
    }

    if ctx.match_token(TokenKind::Return) {
        let expr = parse_expression(ctx)?;
        return_expr = Some(Box::new(expr.expr));
    }

    Ok(SubqueryBody {
        id: 0,
        patterns,
        where_clause,
        return_expr,
    })
}

pub(crate) fn parse_pattern_string(ctx: &mut ParseContext<'_>) -> Result<String, ParseError> {
    let start_pos = ctx.current_position();
    let mut pattern = String::new();

    // Collect pattern tokens until a subquery terminator (RBrace / WHERE /
    // RETURN / MATCH). The terminator is inspected WITHOUT consuming it so
    // `parse_subquery_body` can dispatch on it afterwards.
    let mut tokens = Vec::new();
    loop {
        let terminator = matches!(
            ctx.current_token().kind,
            TokenKind::RBrace | TokenKind::Where | TokenKind::Return | TokenKind::Match
        );
        if terminator {
            break;
        }
        tokens.push(ctx.current_token().clone());
        ctx.next_token();
    }

    if tokens.is_empty() {
        return Err(ParseError::new(
            ParseErrorKind::SyntaxError,
            "Empty pattern in subquery".to_string(),
            start_pos,
        ));
    }

    // Canonicalize the pattern string so it can be re-parsed by the
    // traversal parser later (planning time).
    //
    // Parenthesized patterns (e.g. `MATCH (q:person)`) pass through
    // unchanged. Bare patterns (`a:person-[:knows]->b:person`) have their
    // node segments wrapped in parentheses to yield the standard form
    // `(a:person)-[:knows]->(b:person)`.
    if matches!(tokens[0].kind, TokenKind::LParen) {
        for tok in &tokens {
            pattern.push_str(&tok.lexeme);
            pattern.push(' ');
        }
    } else {
        let mut in_node = false;
        let mut in_brackets = false;
        for tok in &tokens {
            match &tok.kind {
                TokenKind::Identifier(_) => {
                    if !in_node && !in_brackets {
                        pattern.push_str("( ");
                        in_node = true;
                    }
                    pattern.push_str(&tok.lexeme);
                    pattern.push(' ');
                }
                TokenKind::LBracket => {
                    in_brackets = true;
                    pattern.push('[');
                    pattern.push(' ');
                }
                TokenKind::RBracket => {
                    in_brackets = false;
                    pattern.push(']');
                    pattern.push(' ');
                }
                TokenKind::Minus
                | TokenKind::Arrow
                | TokenKind::BackArrow
                | TokenKind::LeftArrow
                | TokenKind::RightArrow => {
                    if in_node {
                        pattern.push_str(") ");
                        in_node = false;
                    }
                    pattern.push_str(&tok.lexeme);
                    pattern.push(' ');
                }
                _ => {
                    pattern.push_str(&tok.lexeme);
                    pattern.push(' ');
                }
            }
        }
        if in_node {
            pattern.push(')');
        }
    }

    Ok(pattern.trim().to_string())
}

pub(crate) fn match_identifier_token(ctx: &mut ParseContext<'_>, expected: &str) -> bool {
    if let TokenKind::Identifier(s) = &ctx.current_token().kind {
        if s.eq_ignore_ascii_case(expected) {
            ctx.next_token();
            return true;
        }
    }
    false
}
