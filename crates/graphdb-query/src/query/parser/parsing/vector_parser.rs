//! Vector Search Parser
//!
//! This module implements the parser for vector search SQL statements,
//! including CREATE VECTOR INDEX, SEARCH VECTOR, and related queries.

use crate::query::parser::ast::stmt::Stmt;
use crate::query::parser::ast::vector::{
    CreateVectorIndex, DropVectorIndex, LookupVector, MatchVector, OrderClause, OrderItem,
    SearchVectorStatement, VectorDistance, VectorIndexConfig, VectorMatchCondition,
    VectorOrderDirection, VectorQueryExpr, VectorQueryType, VectorYieldClause, VectorYieldItem,
    WhereClause, WhereCondition,
};
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
        where_clause = Some(parse_where_clause(ctx)?);
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
        limit = Some(ctx.consume_int()? as usize);
    }

    let mut offset = None;
    if ctx.check_keyword("OFFSET") {
        ctx.consume_keyword("OFFSET")?;
        offset = Some(ctx.consume_int()? as usize);
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
        offset,
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

/// Parse WHERE clause (caller must have already consumed the WHERE keyword)
fn parse_where_clause(
    ctx: &mut ParseContext,
) -> Result<WhereClause, crate::query::parser::ParseError> {
    // Simplified WHERE condition parsing - just parse basic comparison
    let left = ctx.consume_identifier()?;

    // Parse comparison operator
    let op = if ctx.check_token(TokenKind::Eq) {
        ctx.consume_token("=")?;
        crate::query::parser::ast::vector::ComparisonOp::Eq
    } else if ctx.check_token(TokenKind::Ne) {
        ctx.consume_token("!=")?;
        crate::query::parser::ast::vector::ComparisonOp::Ne
    } else if ctx.check_token(TokenKind::Lt) {
        ctx.consume_token("<")?;
        crate::query::parser::ast::vector::ComparisonOp::Lt
    } else if ctx.check_token(TokenKind::Le) {
        ctx.consume_token("<=")?;
        crate::query::parser::ast::vector::ComparisonOp::Le
    } else if ctx.check_token(TokenKind::Gt) {
        ctx.consume_token(">")?;
        crate::query::parser::ast::vector::ComparisonOp::Gt
    } else if ctx.check_token(TokenKind::Ge) {
        ctx.consume_token(">=")?;
        crate::query::parser::ast::vector::ComparisonOp::Ge
    } else {
        return Err(crate::query::parser::ParseError::new(
            crate::query::parser::core::error::ParseErrorKind::SyntaxError,
            "Expected comparison operator".to_string(),
            ctx.current_position(),
        ));
    };

    // Parse right side value
    let right = ctx.consume_value()?;

    let condition = WhereCondition::Comparison(left, op, right);
    Ok(WhereClause { condition })
}

/// Parse ORDER BY clause
fn parse_order_clause(
    ctx: &mut ParseContext,
) -> Result<OrderClause, crate::query::parser::ParseError> {
    let mut items = Vec::new();

    loop {
        let expr = ctx.consume_identifier()?;
        let order = if ctx.check_keyword("DESC") {
            ctx.consume_keyword("DESC")?;
            VectorOrderDirection::Desc
        } else {
            let _ = ctx.consume_keyword("ASC");
            VectorOrderDirection::Asc
        };

        items.push(OrderItem { expr, order });

        if !ctx.consume_optional_token(",") {
            break;
        }
    }

    Ok(OrderClause { items })
}

/// Parse YIELD clause
fn parse_vector_yield_clause(
    ctx: &mut ParseContext,
) -> Result<VectorYieldClause, crate::query::parser::ParseError> {
    let mut items = Vec::new();

    loop {
        let expr = ctx.consume_identifier()?;
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
