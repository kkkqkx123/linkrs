//! Vector Search AST Definitions
//!
//! This module defines the Abstract Syntax Tree (AST) nodes for vector search queries,
//! including CREATE VECTOR INDEX, SEARCH VECTOR, and related statements.

use crate::core::types::span::Span;
use crate::core::Value;
use serde::{Deserialize, Serialize};

// ============================================================================
// Vector Index DDL Statements
// ============================================================================

/// CREATE VECTOR INDEX statement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVectorIndex {
    pub span: Span,
    pub index_name: String,
    pub schema_name: String,
    pub field_name: String,
    pub config: VectorIndexConfig,
    pub if_not_exists: bool,
}

/// Vector index configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorIndexConfig {
    pub vector_size: usize,
    pub distance: VectorDistance,
    pub hnsw_m: Option<usize>,
    pub hnsw_ef_construct: Option<usize>,
}

/// DROP VECTOR INDEX statement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropVectorIndex {
    pub span: Span,
    pub index_name: String,
    pub if_exists: bool,
}

// ============================================================================
// Vector Search DML Statements
// ============================================================================

/// SEARCH VECTOR statement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchVectorStatement {
    pub span: Span,
    pub index_name: String,
    pub query: VectorQueryExpr,
    pub threshold: Option<f32>,
    pub where_clause: Option<WhereClause>,
    pub order_clause: Option<OrderClause>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub yield_clause: Option<VectorYieldClause>,
}

/// Vector query expression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorQueryExpr {
    pub span: Span,
    pub query_type: VectorQueryType,
    pub query_data: String,
}

/// Vector query type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorQueryType {
    /// Direct vector: [0.1, 0.2, ...]
    Vector,
    /// Text query (requires embedding service)
    Text,
    /// Parameter reference: $param_name
    Parameter,
}

/// Vector distance metric
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VectorDistance {
    Cosine,
    Euclidean,
    Dot,
}

/// YIELD clause for vector search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorYieldClause {
    pub items: Vec<VectorYieldItem>,
}

/// Yield item for vector search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorYieldItem {
    pub expr: String,
    pub alias: Option<String>,
}

/// WHERE clause (reuse from fulltext)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhereClause {
    pub condition: WhereCondition,
}

/// WHERE condition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WhereCondition {
    /// Comparison: score > 0.5
    Comparison(String, ComparisonOp, Value),
    /// AND condition
    And(Box<WhereCondition>, Box<WhereCondition>),
    /// OR condition
    Or(Box<WhereCondition>, Box<WhereCondition>),
    /// NOT condition
    Not(Box<WhereCondition>),
}

/// Comparison operator
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComparisonOp {
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
}

/// ORDER BY clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderClause {
    pub items: Vec<OrderItem>,
}

/// Order item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderItem {
    pub expr: String,
    pub order: VectorOrderDirection,
}

/// Order direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorOrderDirection {
    #[serde(rename = "asc")]
    Asc,
    #[serde(rename = "desc")]
    Desc,
}

// ============================================================================
// MATCH and LOOKUP extensions for vector search
// ============================================================================

/// MATCH with vector search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchVector {
    pub span: Span,
    pub pattern: String,
    pub vector_condition: VectorMatchCondition,
    pub yield_clause: Option<VectorYieldClause>,
}

/// Vector match condition in WHERE clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorMatchCondition {
    pub field: String,
    pub query: VectorQueryExpr,
    pub threshold: Option<f32>,
}

/// LOOKUP with vector search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupVector {
    pub span: Span,
    pub schema_name: String,
    pub index_name: String,
    pub query: VectorQueryExpr,
    pub yield_clause: Option<VectorYieldClause>,
    pub limit: Option<usize>,
}

// ============================================================================
// Helper Functions
// ============================================================================

impl CreateVectorIndex {
    pub fn new(
        span: Span,
        index_name: String,
        schema_name: String,
        field_name: String,
        config: VectorIndexConfig,
    ) -> Self {
        Self {
            span,
            index_name,
            schema_name,
            field_name,
            config,
            if_not_exists: false,
        }
    }

    pub fn with_if_not_exists(mut self, if_not_exists: bool) -> Self {
        self.if_not_exists = if_not_exists;
        self
    }
}

impl DropVectorIndex {
    pub fn new(span: Span, index_name: String) -> Self {
        Self {
            span,
            index_name,
            if_exists: false,
        }
    }

    pub fn with_if_exists(mut self, if_exists: bool) -> Self {
        self.if_exists = if_exists;
        self
    }
}

impl VectorQueryExpr {
    pub fn vector(span: Span, vector_str: String) -> Self {
        Self {
            span,
            query_type: VectorQueryType::Vector,
            query_data: vector_str,
        }
    }

    pub fn text(span: Span, text: String) -> Self {
        Self {
            span,
            query_type: VectorQueryType::Text,
            query_data: text,
        }
    }

    pub fn parameter(span: Span, param_name: String) -> Self {
        Self {
            span,
            query_type: VectorQueryType::Parameter,
            query_data: param_name,
        }
    }
}

impl VectorIndexConfig {
    pub fn new(vector_size: usize, distance: VectorDistance) -> Self {
        Self {
            vector_size,
            distance,
            hnsw_m: None,
            hnsw_ef_construct: None,
        }
    }

    pub fn with_hnsw(mut self, m: usize, ef_construct: usize) -> Self {
        self.hnsw_m = Some(m);
        self.hnsw_ef_construct = Some(ef_construct);
        self
    }
}
