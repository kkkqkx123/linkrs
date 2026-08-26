//! Vector Search Parser
//!
//! This module implements the parser for vector search SQL statements,
//! including CREATE VECTOR INDEX, SEARCH VECTOR, and related queries.

use crate::core::types::expr::{create_contextual_expression, Expression};
use crate::query::parser::ast::stmt::Stmt;
use crate::query::parser::ast::stmt::{OrderByClause, OrderByItem};
use crate::query::parser::ast::types::{LimitClause, OrderDirection, SkipClause};
use crate::query::parser::ast::vector::{
    CreateVectorIndex, DropVectorIndex, LookupVector, MatchVector, SearchVectorStatement,
    VectorDistance, VectorIndexConfig, VectorMatchCondition, VectorQueryExpr, VectorQueryType,
    VectorYieldClause, VectorYieldItem,
};
use crate::query::parser::parsing::expr_parser::parse_expression_with_context;
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::TokenKind;

/// Parse vector search statements from ParseContext
pub fn parse_vector(ctx: &mut ParseContext) -> Result<Stmt, crate::query::parser::ParseError> {
    if ctx.check_keyword("CREATE") {
        return parse_create_vector_index(ctx);
    } else if ctx.check_keyword("DROP") {
        return parse_drop_vector_index(ctx);
    } else if ctx.check_keyword("SEARCH") {
        return parse_search_vector_statement(ctx);
    } else if ctx.check_keyword("LOOKUP") {
        return parse_lookup_vector(ctx);
    } else if ctx.check_keyword("MATCH") {
        return parse_match_vector(ctx);
    }

    Err(crate::query::parser::ParseError::new(
        crate::query::parser::core::error::ParseErrorKind::SyntaxError,
        "Not a vector search statement".to_string(),
        ctx.current_position(),
    ))
}

/// Parse CREATE VECTOR INDEX statement
pub fn parse_create_vector_index(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("CREATE")?;
    parse_create_vector_index_after_create(ctx)
}

pub fn parse_create_vector_index_after_create(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    let if_not_exists = if ctx.check_keyword("IF") {
        ctx.consume_keyword("IF")?;
        ctx.consume_keyword("NOT")?;
        ctx.consume_keyword("EXISTS")?;
        true
    } else {
        false
    };

    ctx.consume_keyword("VECTOR")?;
    ctx.consume_keyword("INDEX")?;

    let index_name = ctx.consume_identifier()?;
    ctx.consume_keyword("ON")?;
    let schema_name = ctx.consume_identifier()?;

    ctx.expect_token(TokenKind::LParen)?;
    let field_name = ctx.consume_identifier()?;
    ctx.expect_token(TokenKind::RParen)?;

    ctx.expect_token(TokenKind::With)?;
    let config = parse_vector_index_config(ctx)?;

    let mut create = CreateVectorIndex::new(
        ctx.current_span(),
        index_name,
        schema_name,
        field_name,
        config,
    );
    create.if_not_exists = if_not_exists;

    Ok(Stmt::CreateVectorIndex(create))
}

/// Parse vector index configuration
fn parse_vector_index_config(
    ctx: &mut ParseContext,
) -> Result<VectorIndexConfig, crate::query::parser::ParseError> {
    ctx.expect_token(TokenKind::LParen)?;

    let mut vector_size = None;
    let mut distance = VectorDistance::Cosine;
    let mut hnsw_m = None;
    let mut hnsw_ef_construct = None;

    loop {
        let key = ctx.consume_identifier()?;
        ctx.expect_token(TokenKind::Assign)?;

        match key.to_lowercase().as_str() {
            "vector_size" => {
                vector_size = Some(ctx.consume_int()? as usize);
            }
            "distance" => {
                // Accept both identifier and string literal for distance
                let dist_str = if matches!(ctx.current_token().kind, TokenKind::StringLiteral(_)) {
                    ctx.consume_string()?
                } else {
                    ctx.consume_identifier()?
                };
                distance = match dist_str.to_lowercase().as_str() {
                    "cosine" => VectorDistance::Cosine,
                    "euclidean" => VectorDistance::Euclidean,
                    "dot" => VectorDistance::Dot,
                    "manhattan" => VectorDistance::Manhattan,
                    _ => {
                        return Err(crate::query::parser::ParseError::new(
                            crate::query::parser::core::error::ParseErrorKind::SyntaxError,
                            format!("Unknown distance metric '{}'", dist_str),
                            ctx.current_position(),
                        ))
                    }
                };
            }
            "hnsw_m" => {
                hnsw_m = Some(ctx.consume_int()? as usize);
            }
            "hnsw_ef_construct" => {
                hnsw_ef_construct = Some(ctx.consume_int()? as usize);
            }
            _ => {
                return Err(crate::query::parser::ParseError::new(
                    crate::query::parser::core::error::ParseErrorKind::SyntaxError,
                    format!("Unknown config option '{}'", key),
                    ctx.current_position(),
                ))
            }
        }

        if !ctx.consume_optional_token(",") {
            break;
        }
    }

    ctx.expect_token(TokenKind::RParen)?;

    let vector_size = vector_size.ok_or_else(|| {
        crate::query::parser::ParseError::new(
            crate::query::parser::core::error::ParseErrorKind::SyntaxError,
            "vector_size is required".to_string(),
            ctx.current_position(),
        )
    })?;

    let mut config = VectorIndexConfig::new(vector_size, distance);
    if let Some(m) = hnsw_m {
        config.hnsw_m = Some(m);
    }
    if let Some(ef) = hnsw_ef_construct {
        config.hnsw_ef_construct = Some(ef);
    }

    Ok(config)
}

/// Parse DROP VECTOR INDEX statement
pub fn parse_drop_vector_index(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("DROP")?;
    parse_drop_vector_index_after_drop(ctx)
}

pub fn parse_drop_vector_index_after_drop(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("VECTOR")?;
    ctx.consume_keyword("INDEX")?;

    let if_exists = if ctx.check_keyword("IF") {
        ctx.consume_keyword("IF")?;
        ctx.consume_keyword("EXISTS")?;
        true
    } else {
        false
    };

    let index_name = ctx.consume_identifier()?;

    let drop = DropVectorIndex {
        span: ctx.current_span(),
        index_name,
        if_exists,
    };

    Ok(Stmt::DropVectorIndex(drop))
}

/// Parse SEARCH VECTOR statement
pub fn parse_search_vector_statement(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    let span = ctx.current_span();

    ctx.consume_keyword("SEARCH")?;
    ctx.consume_keyword("VECTOR")?;

    let index_name = ctx.consume_identifier()?;

    ctx.consume_keyword("WITH")?;
    let query = parse_vector_query_expr(ctx)?;

    let mut threshold = None;
    if ctx.check_keyword("THRESHOLD") {
        ctx.consume_keyword("THRESHOLD")?;
        threshold = Some(ctx.consume_float()? as f32);
    }

    let mut where_clause = None;
    if ctx.check_keyword("WHERE") {
        ctx.consume_keyword("WHERE")?;
        where_clause = Some(parse_expression_with_context(
            ctx,
            ctx.expression_context_clone(),
        )?);
    }

    let mut order_clause = None;
    if ctx.check_keyword("ORDER") {
        ctx.consume_keyword("ORDER")?;
        ctx.consume_keyword("BY")?;
        order_clause = Some(parse_order_clause(ctx)?);
    }

    let mut limit = None;
    if ctx.check_keyword("LIMIT") {
        ctx.consume_keyword("LIMIT")?;
        let count = ctx.consume_int()? as usize;
        limit = Some(LimitClause {
            span: ctx.current_span(),
            count,
        });
    }

    let mut skip = None;
    if ctx.check_keyword("OFFSET") {
        ctx.consume_keyword("OFFSET")?;
        let count = ctx.consume_int()? as usize;
        skip = Some(SkipClause {
            span: ctx.current_span(),
            count,
        });
    }

    let mut yield_clause = None;
    if ctx.check_keyword("YIELD") || ctx.check_keyword("RETURN") {
        ctx.consume_keyword("YIELD")?;
        yield_clause = Some(parse_vector_yield_clause(ctx)?);
    }

    Ok(Stmt::SearchVector(SearchVectorStatement {
        span,
        index_name,
        query,
        threshold,
        where_clause,
        order_clause,
        limit,
        skip,
        yield_clause,
    }))
}

/// Parse vector query expression
fn parse_vector_query_expr(
    ctx: &mut ParseContext,
) -> Result<VectorQueryExpr, crate::query::parser::ParseError> {
    let span = ctx.current_span();

    let keyword = ctx.consume_identifier()?;
    if ctx.expect_token(TokenKind::Eq).is_err() {
        ctx.expect_token(TokenKind::Assign)?;
    }

    let (query_type, query_data) = if keyword.to_lowercase() == "vector" {
        // vector = [0.1, 0.2, ...]
        let vector_str = parse_vector_literal(ctx)?;
        (VectorQueryType::Vector, vector_str)
    } else if keyword.to_lowercase() == "text" {
        // text = 'search query'
        let text = ctx.consume_string()?;
        (VectorQueryType::Text, text)
    } else if keyword.to_lowercase() == "param" || keyword.to_lowercase() == "parameter" {
        // param = $param_name
        ctx.expect_token(TokenKind::Dollar)?;
        let param = ctx.expect_identifier()?;
        (VectorQueryType::Parameter, param)
    } else {
        return Err(crate::query::parser::ParseError::new(
            crate::query::parser::core::error::ParseErrorKind::SyntaxError,
            format!("Expected 'vector', 'text', or 'param', found '{}'", keyword),
            ctx.current_position(),
        ));
    };

    Ok(VectorQueryExpr {
        span,
        query_type,
        query_data,
    })
}

/// Parse vector literal
fn parse_vector_literal(
    ctx: &mut ParseContext,
) -> Result<String, crate::query::parser::ParseError> {
    ctx.expect_token(TokenKind::LBracket)?;
    let mut elements = Vec::new();

    loop {
        let num = ctx.consume_float()?;
        elements.push(format!("{}", num));

        if !ctx.consume_optional_token(",") {
            break;
        }
    }

    ctx.expect_token(TokenKind::RBracket)?;
    Ok(format!("[{}]", elements.join(", ")))
}

/// Parse ORDER BY clause
fn parse_order_clause(
    ctx: &mut ParseContext,
) -> Result<OrderByClause, crate::query::parser::ParseError> {
    let span = ctx.current_span();
    let mut items = Vec::new();

    loop {
        let expression = parse_expression_with_context(ctx, ctx.expression_context_clone())?;
        let direction = if ctx.check_keyword("DESC") {
            ctx.consume_keyword("DESC")?;
            OrderDirection::Desc
        } else {
            let _ = ctx.consume_keyword("ASC");
            OrderDirection::Asc
        };

        items.push(OrderByItem {
            expression,
            direction,
        });

        if !ctx.consume_optional_token(",") {
            break;
        }
    }

    Ok(OrderByClause { span, items })
}

/// Parse YIELD clause
fn parse_vector_yield_clause(
    ctx: &mut ParseContext,
) -> Result<VectorYieldClause, crate::query::parser::ParseError> {
    let mut items = Vec::new();

    loop {
        let expr = if ctx.consume_optional_token("*") {
            create_contextual_expression(Expression::variable("*"))
        } else {
            parse_expression_with_context(ctx, ctx.expression_context_clone())?
        };
        let alias = if ctx.check_keyword("AS") {
            ctx.consume_keyword("AS")?;
            Some(ctx.consume_identifier()?)
        } else {
            None
        };

        items.push(VectorYieldItem { expr, alias });

        if !ctx.consume_optional_token(",") {
            break;
        }
    }

    Ok(VectorYieldClause { items })
}

/// Parse LOOKUP VECTOR statement
pub fn parse_lookup_vector(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    let span = ctx.current_span();

    ctx.consume_keyword("LOOKUP")?;
    ctx.consume_keyword("VECTOR")?;

    let schema_name = ctx.consume_identifier()?;
    let index_name = ctx.consume_identifier()?;

    ctx.consume_keyword("WITH")?;
    let query = parse_vector_query_expr(ctx)?;

    let mut yield_clause = None;
    if ctx.check_keyword("YIELD") || ctx.check_keyword("RETURN") {
        ctx.consume_keyword("YIELD")?;
        yield_clause = Some(parse_vector_yield_clause(ctx)?);
    }

    let mut limit = None;
    if ctx.check_keyword("LIMIT") {
        ctx.consume_keyword("LIMIT")?;
        limit = Some(ctx.consume_int()? as usize);
    }

    Ok(Stmt::LookupVector(LookupVector {
        span,
        schema_name,
        index_name,
        query,
        yield_clause,
        limit,
    }))
}

/// Parse MATCH VECTOR statement
pub fn parse_match_vector(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    let span = ctx.current_span();

    ctx.consume_keyword("MATCH")?;
    // The statement entry is the `MATCH VECTOR` sequence; the VECTOR keyword
    // lexes as a dedicated keyword token and must be consumed explicitly.
    ctx.consume_keyword("VECTOR")?;

    // Parse pattern (simplified)
    let pattern = ctx.consume_string()?;

    ctx.consume_keyword("WHERE")?;

    // Parse vector condition
    let field = ctx.consume_identifier()?;
    let query = parse_vector_query_expr(ctx)?;

    let mut threshold = None;
    if ctx.check_keyword("THRESHOLD") {
        ctx.consume_keyword("THRESHOLD")?;
        threshold = Some(ctx.consume_float()? as f32);
    }

    let vector_condition = VectorMatchCondition {
        field,
        query,
        threshold,
    };

    let mut yield_clause = None;
    if ctx.check_keyword("YIELD") || ctx.check_keyword("RETURN") {
        ctx.consume_keyword("YIELD")?;
        yield_clause = Some(parse_vector_yield_clause(ctx)?);
    }

    Ok(Stmt::MatchVector(MatchVector {
        span,
        pattern,
        vector_condition,
        yield_clause,
    }))
}

#[cfg(test)]
mod tests {
    use crate::query::parser::ast::stmt::Stmt;
    use crate::query::parser::parsing::parser::Parser;

    #[test]
    fn test_parse_search_vector_statement() {
        let sql = r#"SEARCH VECTOR idx_product_embedding WITH vector=[0.1, 0.2, 0.3]
                     WHERE price < 500 AND score > 0.5
                     ORDER BY price DESC
                     YIELD product_id, name, price
                     LIMIT 10"#;

        let mut parser = Parser::new(sql);
        let result = parser.parse().expect("SEARCH VECTOR should parse");
        let stmt = result.ast.stmt();
        let search = match stmt {
            Stmt::SearchVector(stmt) => stmt,
            _ => panic!("expected SEARCH VECTOR statement"),
        };

        assert!(search.where_clause.is_some());
        assert!(search.where_clause.as_ref().unwrap().is_binary());
        assert!(search.order_clause.is_some());
        assert_eq!(search.order_clause.as_ref().unwrap().items.len(), 1);

        let yield_items = search.yield_clause.as_ref().expect("YIELD should parse");
        assert_eq!(yield_items.items.len(), 3);
    }

    #[test]
    fn test_parse_drop_vector_index_statement() {
        for (sql, if_exists) in [
            ("DROP VECTOR INDEX idx_product_embedding", false),
            ("DROP VECTOR INDEX IF EXISTS idx_product_embedding", true),
        ] {
            let mut parser = Parser::new(sql);
            let result = parser.parse().expect("DROP VECTOR INDEX should parse");
            match result.ast.stmt() {
                Stmt::DropVectorIndex(drop) => {
                    assert_eq!(drop.index_name, "idx_product_embedding");
                    assert_eq!(drop.if_exists, if_exists);
                }
                other => panic!("expected DROP VECTOR INDEX statement, got {:?}", other),
            }
        }
    }

    #[test]
    fn test_parse_lookup_vector_statement() {
        let sql = r#"LOOKUP VECTOR basketball idx_product_embedding WITH vector=[0.1, 0.2]
                     YIELD product_id
                     LIMIT 5"#;

        let mut parser = Parser::new(sql);
        let result = parser.parse().expect("LOOKUP VECTOR should parse");
        match result.ast.stmt() {
            Stmt::LookupVector(lookup) => {
                assert_eq!(lookup.schema_name, "basketball");
                assert_eq!(lookup.index_name, "idx_product_embedding");
                assert_eq!(lookup.limit, Some(5));
                assert!(lookup.yield_clause.is_some());
            }
            other => panic!("expected LOOKUP VECTOR statement, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_match_vector_statement() {
        let sql = r#"MATCH VECTOR "(n:Person)" WHERE embedding vector=[0.1, 0.2]
                     THRESHOLD 0.8"#;

        let mut parser = Parser::new(sql);
        let result = parser.parse().expect("MATCH VECTOR should parse");
        match result.ast.stmt() {
            Stmt::MatchVector(match_stmt) => {
                assert_eq!(match_stmt.pattern, "(n:Person)");
                assert_eq!(match_stmt.vector_condition.field, "embedding");
                assert_eq!(match_stmt.vector_condition.threshold, Some(0.8));
            }
            other => panic!("expected MATCH VECTOR statement, got {:?}", other),
        }
    }

    #[test]
    fn test_plain_match_and_lookup_still_parse() {
        // Plain MATCH must not be swallowed by the MATCH VECTOR dispatch.
        let mut parser = Parser::new("MATCH (n:Person) RETURN n");
        let match_result = parser.parse().expect("plain MATCH should parse");
        assert!(
            matches!(match_result.ast.stmt(), Stmt::Match(_)),
            "plain MATCH stays a traversal"
        );

        // Plain LOOKUP ON TAG must not be swallowed by LOOKUP VECTOR.
        let mut parser = Parser::new("LOOKUP ON TAG player");
        let lookup_result = parser.parse().expect("plain LOOKUP ON TAG should parse");
        assert!(
            matches!(lookup_result.ast.stmt(), Stmt::Lookup(_)),
            "plain LOOKUP stays a util lookup"
        );
    }
}
