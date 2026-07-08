//! Full-Text Parser
//!
//! This module implements the parser for full-text search SQL statements,
//! including CREATE FULLTEXT INDEX, SEARCH, and related queries.

use crate::core::types::FulltextEngineType;
use crate::core::Value;
use crate::query::parser::ast::fulltext::{
    AlterFulltextIndex, AlterIndexAction, BM25Options, CreateFulltextIndex, DescribeFulltextIndex,
    DropFulltextIndex, FulltextMatchCondition, FulltextOrderDirection, FulltextQueryExpr,
    FulltextYieldClause, FulltextYieldItem, IndexFieldDef, IndexOptions, LookupFulltext,
    MatchFulltext, OrderClause, OrderItem, SearchStatement, ShowFulltextIndex, WhereClause,
    WhereCondition, YieldExpression,
};
use crate::query::parser::ast::stmt::Stmt;
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::TokenKind;
use std::collections::HashMap;

/// Parse full-text search statements from ParseContext
pub fn parse_fulltext(ctx: &mut ParseContext) -> Result<Stmt, crate::query::parser::ParseError> {
    if ctx.check_keyword("CREATE") {
        return parse_create_fulltext_index(ctx);
    } else if ctx.check_keyword("DROP") {
        return parse_drop_fulltext_index(ctx);
    } else if ctx.check_keyword("ALTER") {
        return parse_alter_fulltext_index(ctx);
    } else if ctx.check_keyword("SHOW") {
        return parse_show_fulltext_index(ctx);
    } else if ctx.check_keyword("DESCRIBE") || ctx.check_keyword("DESC") {
        return parse_describe_fulltext_index(ctx);
    } else if ctx.check_keyword("SEARCH") {
        // Check if it's SEARCH VECTOR or SEARCH INDEX
        if ctx.check_keyword_sequence(&["SEARCH", "VECTOR"]) {
            // Forward to vector parser
            return crate::query::parser::parsing::vector_parser::parse_vector(ctx);
        }
        // Check if it's SEARCH INDEX - if so, we need to consume INDEX here
        let is_search_index = ctx.check_keyword_sequence(&["SEARCH", "INDEX"]);
        // Consume SEARCH and continue with fulltext parsing
        ctx.consume_keyword("SEARCH")?;
        if is_search_index {
            // Also consume INDEX here before calling parse_search_statement_after_search
            ctx.consume_keyword("INDEX")?;
        }
        return parse_search_statement_after_search(ctx);
    } else if ctx.check_keyword("LOOKUP") {
        // Check if it's LOOKUP VECTOR
        if ctx.check_keyword_sequence(&["LOOKUP", "VECTOR"]) {
            // Forward to vector parser
            return crate::query::parser::parsing::vector_parser::parse_vector(ctx);
        }
        return parse_lookup_fulltext(ctx);
    } else if ctx.check_keyword("MATCH") {
        return parse_match_fulltext(ctx);
    }

    Err(crate::query::parser::ParseError::new(
        crate::query::parser::core::error::ParseErrorKind::SyntaxError,
        "Not a full-text search statement".to_string(),
        ctx.current_position(),
    ))
}

pub fn parse_create_fulltext_index(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("CREATE")?;
    parse_create_fulltext_index_after_create(ctx)
}

pub fn parse_create_fulltext_index_after_create(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("FULLTEXT")?;
    ctx.consume_keyword("INDEX")?;

    let if_not_exists = if ctx.check_keyword("IF") {
        ctx.consume_keyword("IF")?;
        ctx.consume_keyword("NOT")?;
        ctx.consume_keyword("EXISTS")?;
        true
    } else {
        false
    };

    let index_name = ctx.consume_identifier()?;
    ctx.consume_keyword("ON")?;
    let schema_name = ctx.consume_identifier()?;

    ctx.expect_token(TokenKind::LParen)?;
    let mut fields = Vec::new();

    loop {
        let field_name = ctx.consume_identifier()?;

        let mut field_def = IndexFieldDef::new(field_name);

        if ctx.check_keyword("ANALYZER") {
            ctx.consume_keyword("ANALYZER")?;
            field_def.analyzer = Some(ctx.consume_string()?);
        }

        if ctx.check_keyword("BOOST") {
            ctx.consume_keyword("BOOST")?;
            field_def.boost = Some(ctx.consume_float()? as f32);
        }

        fields.push(field_def);

        if !ctx.consume_optional_token(",") {
            break;
        }
    }

    ctx.expect_token(TokenKind::RParen)?;

    ctx.consume_keyword("ENGINE")?;
    if ctx.check_keyword("BM25") {
        ctx.consume_keyword("BM25")?;
    } else {
        return Err(crate::query::parser::ParseError::new(
            crate::query::parser::core::error::ParseErrorKind::SyntaxError,
            "Expected BM25 engine type".to_string(),
            ctx.current_position(),
        ));
    }
    let engine_type = FulltextEngineType::Bm25;

    let options = IndexOptions {
        bm25_config: None,
        common_options: HashMap::new(),
    };

    let mut options = options;

    if ctx.check_keyword("OPTIONS") {
        ctx.consume_keyword("OPTIONS")?;
        ctx.expect_token(TokenKind::LParen)?;

        loop {
            let key = ctx.consume_identifier()?;
            ctx.expect_token(TokenKind::Assign)?;

            match key.to_lowercase().as_str() {
                "k1" => {
                    if options.bm25_config.is_none() {
                        options.bm25_config = Some(BM25Options {
                            k1: None,
                            b: None,
                            field_weights: HashMap::new(),
                            analyzer: None,
                            store_original: None,
                        });
                    }
                    options.bm25_config.as_mut().unwrap().k1 = Some(ctx.consume_float()? as f32);
                }
                "b" => {
                    if options.bm25_config.is_none() {
                        options.bm25_config = Some(BM25Options {
                            k1: None,
                            b: None,
                            field_weights: HashMap::new(),
                            analyzer: None,
                            store_original: None,
                        });
                    }
                    options.bm25_config.as_mut().unwrap().b = Some(ctx.consume_float()? as f32);
                }
                "analyzer" => {
                    if options.bm25_config.is_none() {
                        options.bm25_config = Some(BM25Options {
                            k1: None,
                            b: None,
                            field_weights: HashMap::new(),
                            analyzer: None,
                            store_original: None,
                        });
                    }
                    options.bm25_config.as_mut().unwrap().analyzer = Some(ctx.consume_string()?);
                }
                _ => {
                    let value = ctx.consume_value()?;
                    options.common_options.insert(key, value);
                }
            }

            if !ctx.consume_optional_token(",") {
                break;
            }
        }

        ctx.expect_token(TokenKind::RParen)?;
    }

    let mut create = CreateFulltextIndex::new(
        ctx.current_span(),
        index_name,
        schema_name,
        fields,
        engine_type,
    );
    create.if_not_exists = if_not_exists;
    create.options = options;

    Ok(Stmt::CreateFulltextIndex(create))
}

pub fn parse_drop_fulltext_index(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("DROP")?;
    parse_drop_fulltext_index_after_drop(ctx)
}

pub fn parse_drop_fulltext_index_after_drop(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("FULLTEXT")?;
    ctx.consume_keyword("INDEX")?;

    let if_exists = if ctx.check_keyword("IF") {
        ctx.consume_keyword("IF")?;
        ctx.consume_keyword("EXISTS")?;
        true
    } else {
        false
    };

    let index_name = ctx.consume_identifier()?;

    let drop = DropFulltextIndex {
        span: ctx.current_span(),
        index_name,
        if_exists,
    };

    Ok(Stmt::DropFulltextIndex(drop))
}

pub fn parse_alter_fulltext_index(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("ALTER")?;
    parse_alter_fulltext_index_after_alter(ctx)
}

pub fn parse_alter_fulltext_index_after_alter(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("FULLTEXT")?;
    ctx.consume_keyword("INDEX")?;

    let index_name = ctx.consume_identifier()?;
    let mut actions = Vec::new();

    loop {
        if ctx.check_keyword("ADD") {
            ctx.consume_keyword("ADD")?;
            ctx.consume_keyword("FIELD")?;

            let field_name = ctx.consume_identifier()?;
            let mut field_def = IndexFieldDef::new(field_name);

            if ctx.check_keyword("ANALYZER") {
                ctx.consume_keyword("ANALYZER")?;
                field_def.analyzer = Some(ctx.consume_string()?);
            }

            actions.push(AlterIndexAction::AddField(field_def));
        } else if ctx.check_keyword("DROP") {
            ctx.consume_keyword("DROP")?;
            ctx.consume_keyword("FIELD")?;
            let field_name = ctx.consume_identifier()?;
            actions.push(AlterIndexAction::DropField(field_name));
        } else if ctx.check_keyword("SET") {
            ctx.consume_keyword("SET")?;
            let key = ctx.consume_identifier()?;
            ctx.expect_token(TokenKind::Eq)?;
            let value = ctx.consume_value()?;
            actions.push(AlterIndexAction::SetOption(key, value));
        } else if ctx.check_keyword("REBUILD") {
            ctx.consume_keyword("REBUILD")?;
            actions.push(AlterIndexAction::Rebuild);
        } else if ctx.check_keyword("OPTIMIZE") {
            ctx.consume_keyword("OPTIMIZE")?;
            actions.push(AlterIndexAction::Optimize);
        } else {
            return Err(crate::query::parser::ParseError::new(
                crate::query::parser::core::error::ParseErrorKind::SyntaxError,
                "Expected ALTER INDEX action".to_string(),
                ctx.current_position(),
            ));
        }

        if !ctx.consume_optional_token(",") {
            break;
        }
    }

    let alter = AlterFulltextIndex {
        span: ctx.current_span(),
        index_name,
        actions,
    };

    Ok(Stmt::AlterFulltextIndex(alter))
}

fn parse_show_fulltext_index(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("SHOW")?;
    ctx.consume_keyword("FULLTEXT")?;
    ctx.consume_keyword("INDEX")?;

    let mut pattern = None;
    let mut from_schema = None;

    if ctx.check_keyword("LIKE") {
        ctx.consume_keyword("LIKE")?;
        pattern = Some(ctx.consume_string()?);
    }

    if ctx.check_keyword("FROM") || ctx.check_keyword("IN") {
        ctx.consume_keyword("FROM")?;
        from_schema = Some(ctx.consume_identifier()?);
    }

    let show = ShowFulltextIndex {
        span: ctx.current_span(),
        pattern,
        from_schema,
    };

    Ok(Stmt::ShowFulltextIndex(show))
}

fn parse_describe_fulltext_index(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("DESCRIBE")?;
    ctx.consume_keyword("FULLTEXT")?;
    ctx.consume_keyword("INDEX")?;

    let index_name = ctx.consume_identifier()?;

    let describe = DescribeFulltextIndex {
        span: ctx.current_span(),
        index_name,
    };

    Ok(Stmt::DescribeFulltextIndex(describe))
}

fn parse_search_statement_after_search(
    ctx: &mut ParseContext,
) -> Result<Stmt, crate::query::parser::ParseError> {
    // INDEX keyword has already been consumed by the caller
    let index_name = ctx.consume_identifier()?;
    ctx.consume_keyword("MATCH")?;

    let query = parse_fulltext_query_expr(ctx)?;

    let mut search = SearchStatement::new(index_name, query);

    if ctx.check_keyword("YIELD") {
        ctx.consume_keyword("YIELD")?;
        let yield_clause = parse_yield_clause(ctx)?;
        search.yield_clause = Some(yield_clause);
    }

    if ctx.check_keyword("WHERE") {
        ctx.consume_keyword("WHERE")?;
        let where_clause = parse_where_clause(ctx)?;
        search.where_clause = Some(where_clause);
    }

    if ctx.check_keyword("ORDER") {
        ctx.consume_keyword("ORDER")?;
        ctx.consume_keyword("BY")?;
        let order_clause = parse_order_clause(ctx)?;
        search.order_clause = Some(order_clause);
    }

    if ctx.check_keyword("LIMIT") {
        ctx.consume_keyword("LIMIT")?;
        search.limit = Some(ctx.consume_int()? as usize);
    }

    if ctx.check_keyword("OFFSET") {
        ctx.consume_keyword("OFFSET")?;
        let offset = ctx.consume_int()? as usize;
        search.offset = Some(offset);
    }

    Ok(Stmt::Search(search))
}

fn parse_fulltext_query_expr(
    ctx: &mut ParseContext,
) -> Result<FulltextQueryExpr, crate::query::parser::ParseError> {
    if let Some(text) = ctx.try_consume_string() {
        return Ok(FulltextQueryExpr::Simple(text));
    }

    if ctx.is_identifier_token() {
        let field = ctx.consume_identifier()?;
        if ctx.consume_optional_token(":") {
            let query = ctx.consume_string()?;
            return Ok(FulltextQueryExpr::Field(field, query));
        }
    }

    if let Some(text) = ctx.try_consume_quoted_string() {
        return Ok(FulltextQueryExpr::Phrase(text));
    }

    Err(crate::query::parser::ParseError::new(
        crate::query::parser::core::error::ParseErrorKind::SyntaxError,
        "Expected full-text query expression".to_string(),
        ctx.current_position(),
    ))
}

fn parse_yield_clause(
    ctx: &mut ParseContext,
) -> Result<FulltextYieldClause, crate::query::parser::ParseError> {
    let mut items = Vec::new();

    loop {
        let expr = if ctx.check_keyword("score") {
            ctx.consume_identifier()?;
            YieldExpression::Score(None)
        } else if ctx.check_keyword("highlight") {
            ctx.consume_identifier()?;
            ctx.expect_token(TokenKind::LParen)?;
            let field = ctx.consume_identifier()?;

            let mut params = None;
            if ctx.consume_optional_token(",") {
                params = None;
            }

            ctx.expect_token(TokenKind::RParen)?;
            YieldExpression::Highlight(field, params)
        } else if ctx.check_keyword("matched_fields") {
            ctx.consume_identifier()?;
            YieldExpression::MatchedFields
        } else if ctx.consume_optional_token("*") {
            YieldExpression::All
        } else {
            let field = ctx.consume_identifier()?;
            YieldExpression::Field(field)
        };

        let alias = if ctx.check_keyword("AS") {
            ctx.consume_keyword("AS")?;
            Some(ctx.consume_identifier()?)
        } else {
            None
        };

        items.push(FulltextYieldItem { expr, alias });

        if !ctx.consume_optional_token(",") {
            break;
        }
    }

    Ok(FulltextYieldClause { items })
}

fn parse_where_clause(
    ctx: &mut ParseContext,
) -> Result<WhereClause, crate::query::parser::ParseError> {
    let condition = parse_where_condition(ctx)?;
    Ok(WhereClause { condition })
}

fn parse_where_condition(
    ctx: &mut ParseContext,
) -> Result<WhereCondition, crate::query::parser::ParseError> {
    if ctx.check_keyword("score") {
        ctx.consume_identifier()?;
        let op = parse_comparison_op(ctx)?;
        let value = ctx.consume_value()?;
        Ok(WhereCondition::Comparison("score".to_string(), op, value))
    } else {
        Ok(WhereCondition::Comparison(
            "field".to_string(),
            crate::query::parser::ast::fulltext::ComparisonOp::Eq,
            Value::Null(crate::core::null::NullType::Null),
        ))
    }
}

fn parse_comparison_op(
    ctx: &mut ParseContext,
) -> Result<crate::query::parser::ast::fulltext::ComparisonOp, crate::query::parser::ParseError> {
    if ctx.consume_optional_token("=") {
        Ok(crate::query::parser::ast::fulltext::ComparisonOp::Eq)
    } else if ctx.consume_optional_token("!=") {
        Ok(crate::query::parser::ast::fulltext::ComparisonOp::Ne)
    } else if ctx.consume_optional_token("<") {
        Ok(crate::query::parser::ast::fulltext::ComparisonOp::Lt)
    } else if ctx.consume_optional_token("<=") {
        Ok(crate::query::parser::ast::fulltext::ComparisonOp::Le)
    } else if ctx.consume_optional_token(">") {
        Ok(crate::query::parser::ast::fulltext::ComparisonOp::Gt)
    } else if ctx.consume_optional_token(">=") {
        Ok(crate::query::parser::ast::fulltext::ComparisonOp::Ge)
    } else {
        Err(crate::query::parser::ParseError::new(
            crate::query::parser::core::error::ParseErrorKind::SyntaxError,
            "Expected comparison operator".to_string(),
            ctx.current_position(),
        ))
    }
}

fn parse_order_clause(
    ctx: &mut ParseContext,
) -> Result<OrderClause, crate::query::parser::ParseError> {
    let mut items = Vec::new();

    loop {
        let expr = ctx.consume_identifier()?;
        let order = if ctx.check_keyword("ASC") {
            ctx.consume_keyword("ASC")?;
            FulltextOrderDirection::Asc
        } else if ctx.check_keyword("DESC") {
            ctx.consume_keyword("DESC")?;
            FulltextOrderDirection::Desc
        } else {
            FulltextOrderDirection::Asc
        };

        items.push(OrderItem { expr, order });

        if !ctx.consume_optional_token(",") {
            break;
        }
    }

    Ok(OrderClause { items })
}

fn parse_lookup_fulltext(ctx: &mut ParseContext) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("LOOKUP")?;
    ctx.consume_keyword("ON")?;

    let schema_name = ctx.consume_identifier()?;
    ctx.consume_keyword("INDEX")?;
    let index_name = ctx.consume_identifier()?;

    ctx.consume_keyword("WHERE")?;
    let query = ctx.consume_string()?;

    let mut lookup = LookupFulltext {
        span: ctx.current_span(),
        schema_name,
        index_name,
        query,
        yield_clause: None,
        limit: None,
    };

    if ctx.check_keyword("YIELD") {
        ctx.consume_keyword("YIELD")?;
        lookup.yield_clause = Some(parse_yield_clause(ctx)?);
    }

    if ctx.check_keyword("LIMIT") {
        ctx.consume_keyword("LIMIT")?;
        lookup.limit = Some(ctx.consume_int()? as usize);
    }

    Ok(Stmt::LookupFulltext(lookup))
}

fn parse_match_fulltext(ctx: &mut ParseContext) -> Result<Stmt, crate::query::parser::ParseError> {
    ctx.consume_keyword("MATCH")?;
    let pattern = ctx.consume_string()?;

    ctx.consume_keyword("WHERE")?;
    ctx.consume_keyword("FULLTEXT_MATCH")?;
    ctx.expect_token(TokenKind::LParen)?;
    let field = ctx.consume_identifier()?;
    ctx.expect_token(TokenKind::Comma)?;
    let query = ctx.consume_string()?;
    ctx.expect_token(TokenKind::RParen)?;

    let condition = FulltextMatchCondition {
        field,
        query,
        index_name: None,
    };

    let mut match_stmt = MatchFulltext {
        span: ctx.current_span(),
        pattern,
        fulltext_condition: condition,
        yield_clause: None,
    };

    if ctx.check_keyword("YIELD") {
        ctx.consume_keyword("YIELD")?;
        match_stmt.yield_clause = Some(parse_yield_clause(ctx)?);
    }

    Ok(Stmt::MatchFulltext(match_stmt))
}

impl IndexFieldDef {
    fn new(field_name: String) -> Self {
        Self {
            field_name,
            analyzer: None,
            boost: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::query::parser::parsing::parser::Parser;

    #[test]
    fn test_parse_create_fulltext_index() {
        let sql = r#"CREATE FULLTEXT INDEX idx_article_content
                     ON article(title, content)
                     ENGINE BM25"#;

        let mut parser = Parser::new(sql);
        let result = parser.parse();
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_search_statement() {
        let sql = r#"SEARCH INDEX idx_article MATCH 'database'
                     YIELD doc_id, score() AS s
                     LIMIT 10"#;

        let mut parser = Parser::new(sql);
        let result = parser.parse();
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_drop_index() {
        let sql = "DROP FULLTEXT INDEX idx_article";

        let mut parser = Parser::new(sql);
        let result = parser.parse();
        assert!(result.is_ok());
    }
}
