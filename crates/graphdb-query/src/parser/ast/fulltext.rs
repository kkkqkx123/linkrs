//! Full-Text Search AST Definitions
//!
//! This module defines the Abstract Syntax Tree (AST) nodes for full-text search queries,
//! including CREATE FULLTEXT INDEX, SEARCH, and related statements.

use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::span::Span;
use graphdb_core::types::FulltextEngineType;
use graphdb_core::Value;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::stmt::OrderByClause;
use super::types::{LimitClause, SkipClause};

// ============================================================================
// Full-Text Index DDL Statements
// ============================================================================

/// CREATE FULLTEXT INDEX statement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFulltextIndex {
    pub span: Span,
    pub index_name: String,
    pub schema_name: String,
    pub fields: Vec<IndexFieldDef>,
    pub engine_type: FulltextEngineType,
    pub options: IndexOptions,
    pub if_not_exists: bool,
}

/// Index field definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexFieldDef {
    pub field_name: String,
    pub analyzer: Option<String>,
    pub boost: Option<f32>,
}

/// Index options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexOptions {
    pub bm25_config: Option<BM25Options>,
    pub common_options: HashMap<String, Value>,
}

/// BM25 specific options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BM25Options {
    pub k1: Option<f32>,
    pub b: Option<f32>,
    pub field_weights: HashMap<String, f32>,
    pub analyzer: Option<String>,
    pub store_original: Option<bool>,
}

/// DROP FULLTEXT INDEX statement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropFulltextIndex {
    pub span: Span,
    pub index_name: String,
    pub if_exists: bool,
}

/// ALTER FULLTEXT INDEX statement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlterFulltextIndex {
    pub span: Span,
    pub index_name: String,
    pub actions: Vec<AlterIndexAction>,
}

/// Alter index action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlterIndexAction {
    AddField(IndexFieldDef),
    DropField(String),
    SetOption(String, Value),
    Rebuild,
    Optimize,
}

/// SHOW FULLTEXT INDEX statement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShowFulltextIndex {
    pub span: Span,
    pub pattern: Option<String>,
    pub from_schema: Option<String>,
}

/// DESCRIBE FULLTEXT INDEX statement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DescribeFulltextIndex {
    pub span: Span,
    pub index_name: String,
}

// ============================================================================
// Full-Text Search DML Statements
// ============================================================================

/// SEARCH statement
#[derive(Debug, Clone)]
pub struct SearchStatement {
    pub span: Span,
    pub index_name: String,
    pub query: FulltextQueryExpr,
    pub yield_clause: Option<FulltextYieldClause>,
    pub where_clause: Option<ContextualExpression>,
    pub order_clause: Option<OrderByClause>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
}

/// Full-text query expression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FulltextQueryExpr {
    /// Simple text query: MATCH 'database'
    Simple(String),
    /// Field-specific query: title:'database'
    Field(String, String),
    /// Multi-field query: title:'database' OR content:'database'
    MultiField(Vec<(String, String)>),
    /// Boolean query: title:'database' AND content:'optimization'
    Boolean {
        must: Vec<FulltextQueryExpr>,
        should: Vec<FulltextQueryExpr>,
        must_not: Vec<FulltextQueryExpr>,
    },
    /// Phrase query: "database optimization"
    Phrase(String),
    /// Prefix query: data*
    Prefix(String),
    /// Fuzzy query: database~
    Fuzzy(String, Option<u8>),
    /// Range query: [2020 TO 2023]
    Range {
        field: String,
        lower: Option<String>,
        upper: Option<String>,
        include_lower: bool,
        include_upper: bool,
    },
    /// Wildcard query: data*ase
    Wildcard(String),
}

/// YIELD clause for full-text search
#[derive(Debug, Clone)]
pub struct FulltextYieldClause {
    pub items: Vec<FulltextYieldItem>,
}

/// Yield item for full-text search
#[derive(Debug, Clone)]
pub struct FulltextYieldItem {
    pub expr: ContextualExpression,
    pub alias: Option<String>,
}

// ============================================================================
// MATCH clause extensions for full-text search
// ============================================================================

/// MATCH with full-text search
#[derive(Debug, Clone)]
pub struct MatchFulltext {
    pub span: Span,
    pub pattern: String,
    pub fulltext_condition: FulltextMatchCondition,
    pub yield_clause: Option<FulltextYieldClause>,
}

/// Full-text match condition in WHERE clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextMatchCondition {
    pub field: String,
    pub query: String,
    pub index_name: Option<String>,
}

/// LOOKUP with full-text search
#[derive(Debug, Clone)]
pub struct LookupFulltext {
    pub span: Span,
    pub schema_name: String,
    pub index_name: String,
    pub query: String,
    pub yield_clause: Option<FulltextYieldClause>,
    pub limit: Option<usize>,
}

// ============================================================================
// Helper Functions
// ============================================================================

impl CreateFulltextIndex {
    pub fn new(
        span: Span,
        index_name: String,
        schema_name: String,
        fields: Vec<IndexFieldDef>,
        engine_type: FulltextEngineType,
    ) -> Self {
        Self {
            span,
            index_name,
            schema_name,
            fields,
            engine_type,
            options: IndexOptions {
                bm25_config: None,
                common_options: HashMap::new(),
            },
            if_not_exists: false,
        }
    }

    pub fn with_if_not_exists(mut self, if_not_exists: bool) -> Self {
        self.if_not_exists = if_not_exists;
        self
    }

    pub fn with_bm25_options(mut self, options: BM25Options) -> Self {
        self.options.bm25_config = Some(options);
        self
    }
}

impl SearchStatement {
    pub fn new(index_name: String, query: FulltextQueryExpr) -> Self {
        Self {
            span: Span::default(),
            index_name,
            query,
            yield_clause: None,
            where_clause: None,
            order_clause: None,
            limit: None,
            skip: None,
        }
    }

    pub fn with_yield(mut self, yield_clause: FulltextYieldClause) -> Self {
        self.yield_clause = Some(yield_clause);
        self
    }

    pub fn with_where(mut self, where_clause: ContextualExpression) -> Self {
        self.where_clause = Some(where_clause);
        self
    }

    pub fn with_order(mut self, order_clause: OrderByClause) -> Self {
        self.order_clause = Some(order_clause);
        self
    }

    pub fn with_limit(mut self, limit: LimitClause) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn with_skip(mut self, skip: SkipClause) -> Self {
        self.skip = Some(skip);
        self
    }
}

impl FulltextYieldClause {
    pub fn new(items: Vec<FulltextYieldItem>) -> Self {
        Self { items }
    }

    pub fn single(expr: ContextualExpression) -> Self {
        Self {
            items: vec![FulltextYieldItem { expr, alias: None }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_fulltext_index() {
        let fields = vec![IndexFieldDef {
            field_name: "content".to_string(),
            analyzer: None,
            boost: None,
        }];

        let create = CreateFulltextIndex::new(
            Span::default(),
            "idx_article_content".to_string(),
            "article".to_string(),
            fields,
            FulltextEngineType::Bm25,
        );

        assert_eq!(create.index_name, "idx_article_content");
        assert_eq!(create.schema_name, "article");
        assert_eq!(create.engine_type, FulltextEngineType::Bm25);
    }

    #[test]
    fn test_search_statement() {
        let query = FulltextQueryExpr::Simple("database".to_string());
        let search = SearchStatement::new("idx_article".to_string(), query);

        assert!(matches!(search.query, FulltextQueryExpr::Simple(_)));
    }

    #[test]
    fn test_yield_clause() {
        use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
        use graphdb_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use std::sync::Arc;

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Function {
            name: "score".to_string(),
            args: vec![],
        };
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        let yield_clause = FulltextYieldClause::single(ContextualExpression::new(id, expr_ctx));
        assert_eq!(yield_clause.items.len(), 1);
    }

    #[test]
    fn test_boolean_query() {
        let must = vec![FulltextQueryExpr::Simple("database".to_string())];
        let should = vec![];
        let must_not = vec![];

        let query = FulltextQueryExpr::Boolean {
            must,
            should,
            must_not,
        };

        assert!(matches!(query, FulltextQueryExpr::Boolean { .. }));
    }
}
