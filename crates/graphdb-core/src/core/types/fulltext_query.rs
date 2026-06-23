//! Full-Text Query Type Definitions
//!
//! This module defines types related to full-text search queries, including query types,
//! search options, highlight configuration, and result types.

use crate::core::Value;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Full-text query type enumeration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FulltextQuery {
    /// Simple text query
    Simple(String),
    /// Multi-field query
    MultiField(Vec<FieldQuery>),
    /// Boolean query
    Boolean {
        must: Vec<FulltextQuery>,
        should: Vec<FulltextQuery>,
        must_not: Vec<FulltextQuery>,
        minimum_should_match: Option<usize>,
    },
    /// Phrase query
    Phrase { text: String, slop: u32 },
    /// Prefix query
    Prefix { field: String, prefix: String },
    /// Fuzzy query
    Fuzzy {
        field: String,
        value: String,
        distance: u8,
        transpositions: bool,
    },
    /// Range query
    Range {
        field: String,
        lower: Option<String>,
        upper: Option<String>,
        include_lower: bool,
        include_upper: bool,
    },
    /// Wildcard query
    Wildcard { field: String, pattern: String },
}

impl FulltextQuery {
    /// Create a simple text query
    pub fn simple(text: String) -> Self {
        FulltextQuery::Simple(text)
    }

    /// Create a multi-field query
    pub fn multi_field(fields: Vec<FieldQuery>) -> Self {
        FulltextQuery::MultiField(fields)
    }

    /// Create a boolean query
    pub fn boolean(
        must: Vec<FulltextQuery>,
        should: Vec<FulltextQuery>,
        must_not: Vec<FulltextQuery>,
    ) -> Self {
        FulltextQuery::Boolean {
            must,
            should,
            must_not,
            minimum_should_match: None,
        }
    }

    /// Create a phrase query
    pub fn phrase(text: String, slop: u32) -> Self {
        FulltextQuery::Phrase { text, slop }
    }

    /// Create a prefix query
    pub fn prefix(field: String, prefix: String) -> Self {
        FulltextQuery::Prefix { field, prefix }
    }

    /// Create a fuzzy query
    pub fn fuzzy(field: String, value: String, distance: u8) -> Self {
        FulltextQuery::Fuzzy {
            field,
            value,
            distance,
            transpositions: true,
        }
    }
}

/// Field query for multi-field search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldQuery {
    pub field: String,
    pub query: String,
    pub boost: f32,
}

impl FieldQuery {
    pub fn new(field: String, query: String) -> Self {
        Self {
            field,
            query,
            boost: 1.0,
        }
    }

    pub fn with_boost(mut self, boost: f32) -> Self {
        self.boost = boost;
        self
    }
}

/// Full-text query options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextQueryOptions {
    /// Maximum number of results to return
    pub limit: usize,
    /// Offset for pagination
    pub offset: usize,
    /// Whether to return explanation
    pub explain: bool,
    /// Highlight configuration
    pub highlight: Option<HighlightOptions>,
    /// Sort configuration
    pub sort: Vec<SortField>,
    /// Whether to track total hits
    pub track_total_hits: bool,
    /// Minimum score threshold
    pub min_score: Option<f64>,
}

impl Default for FulltextQueryOptions {
    fn default() -> Self {
        Self {
            limit: 10,
            offset: 0,
            explain: false,
            highlight: None,
            sort: Vec::new(),
            track_total_hits: true,
            min_score: None,
        }
    }
}

impl FulltextQueryOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    pub fn with_highlight(mut self, highlight: bool) -> Self {
        if highlight {
            self.highlight = Some(HighlightOptions::default());
        }
        self
    }

    pub fn with_explain(mut self, explain: bool) -> Self {
        self.explain = explain;
        self
    }
}

/// Highlight configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightOptions {
    /// Fields to highlight
    pub fields: Vec<String>,
    /// Pre-tag for highlighting
    pub pre_tag: String,
    /// Post-tag for highlighting
    pub post_tag: String,
    /// Fragment size
    pub fragment_size: usize,
    /// Maximum number of fragments
    pub num_fragments: usize,
    /// Encoder type
    pub encoder: String,
    /// Boundary detector
    pub boundary_detector: String,
}

impl Default for HighlightOptions {
    fn default() -> Self {
        Self {
            fields: Vec::new(),
            pre_tag: "<em>".to_string(),
            post_tag: "</em>".to_string(),
            fragment_size: 100,
            num_fragments: 3,
            encoder: "default".to_string(),
            boundary_detector: "sentence".to_string(),
        }
    }
}

impl HighlightOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_fields(mut self, fields: Vec<String>) -> Self {
        self.fields = fields;
        self
    }

    pub fn with_tags(mut self, pre_tag: String, post_tag: String) -> Self {
        self.pre_tag = pre_tag;
        self.post_tag = post_tag;
        self
    }
}

/// Sort field configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortField {
    pub field: String,
    pub order: SortOrder,
    pub missing: SortMissing,
}

impl SortField {
    pub fn new(field: String, order: SortOrder) -> Self {
        Self {
            field,
            order,
            missing: SortMissing::Last,
        }
    }
}

/// Sort order enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortOrder {
    #[serde(rename = "asc")]
    Asc,
    #[serde(rename = "desc")]
    Desc,
}

/// Sort missing value handling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortMissing {
    #[serde(rename = "first")]
    First,
    #[serde(rename = "last")]
    Last,
}

// ============================================================================
// Full-Text Search Result Types
// ============================================================================

/// Full-text search result
#[derive(Debug, Clone)]
pub struct FulltextSearchResult {
    /// Search result entries
    pub results: Vec<SearchResultEntry>,
    /// Total number of hits
    pub total_hits: usize,
    /// Maximum score
    pub max_score: f64,
    /// Time taken in milliseconds
    pub took_ms: u64,
    /// Whether timed out
    pub timed_out: bool,
    /// Shards information
    pub shards: Option<ShardsInfo>,
}

impl Default for FulltextSearchResult {
    fn default() -> Self {
        Self {
            results: Vec::new(),
            total_hits: 0,
            max_score: 0.0,
            took_ms: 0,
            timed_out: false,
            shards: None,
        }
    }
}

/// Search result entry
#[derive(Debug, Clone)]
pub struct SearchResultEntry {
    /// Document ID
    pub doc_id: Value,
    /// Relevance score
    pub score: f64,
    /// Highlight results
    pub highlights: Option<HashMap<String, Vec<String>>>,
    /// Matched fields
    pub matched_fields: Vec<String>,
    /// Query explanation
    pub explanation: Option<QueryExplanation>,
    /// Sort values
    pub sort_values: Vec<Value>,
    /// Source document data
    pub source: Option<HashMap<String, Value>>,
}

impl SearchResultEntry {
    pub fn new(doc_id: Value, score: f64) -> Self {
        Self {
            doc_id,
            score,
            highlights: None,
            matched_fields: Vec::new(),
            explanation: None,
            sort_values: Vec::new(),
            source: None,
        }
    }
}

/// Query explanation
#[derive(Debug, Clone)]
pub struct QueryExplanation {
    pub value: f64,
    pub description: String,
    pub details: Vec<QueryExplanation>,
}

impl QueryExplanation {
    pub fn new(value: f64, description: String) -> Self {
        Self {
            value,
            description,
            details: Vec::new(),
        }
    }
}

/// Shards information
#[derive(Debug, Clone)]
pub struct ShardsInfo {
    pub total: usize,
    pub successful: usize,
    pub failed: usize,
    pub failures: Vec<ShardFailure>,
}

/// Shard failure information
#[derive(Debug, Clone)]
pub struct ShardFailure {
    pub shard: usize,
    pub index: String,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fulltext_query_simple() {
        let query = FulltextQuery::simple("database".to_string());
        assert!(matches!(query, FulltextQuery::Simple(_)));
    }

    #[test]
    fn test_fulltext_query_multi_field() {
        let field_query = FieldQuery::new("title".to_string(), "database".to_string());
        let query = FulltextQuery::multi_field(vec![field_query]);
        assert!(matches!(query, FulltextQuery::MultiField(_)));
    }

    #[test]
    fn test_field_query_with_boost() {
        let field_query =
            FieldQuery::new("title".to_string(), "database".to_string()).with_boost(2.0);
        assert_eq!(field_query.boost, 2.0);
    }

    #[test]
    fn test_highlight_options() {
        let highlight = HighlightOptions::new()
            .with_fields(vec!["content".to_string()])
            .with_tags("<b>".to_string(), "</b>".to_string());

        assert_eq!(highlight.fields.len(), 1);
        assert_eq!(highlight.pre_tag, "<b>");
        assert_eq!(highlight.post_tag, "</b>");
    }

    #[test]
    fn test_query_options() {
        let options = FulltextQueryOptions::new()
            .with_limit(20)
            .with_offset(10)
            .with_highlight(true)
            .with_explain(true);

        assert_eq!(options.limit, 20);
        assert_eq!(options.offset, 10);
        assert!(options.highlight.is_some());
        assert!(options.explain);
    }
}
