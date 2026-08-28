//! Fulltext protocol DTOs.
//!
//! Transport-independent request/response shapes for fulltext index management
//! and search, shared between the server handlers and CLI clients.
//!
//! These types provide structured typing for fulltext-specific operations,
//! mirroring the vector wire DTOs for consistency.

use serde::{Deserialize, Serialize};

/// Fulltext search request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextSearchRequest {
    /// Index name to search
    pub index_name: String,
    /// Search query string
    pub query: String,
    /// Maximum number of results to return
    #[serde(default)]
    pub limit: Option<usize>,
    /// Offset for pagination
    #[serde(default)]
    pub offset: Option<usize>,
}

/// Fulltext search response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextSearchResponse {
    /// Search results
    pub results: Vec<FulltextSearchResult>,
    /// Total number of matching documents
    pub total_hits: usize,
    /// Search execution time in milliseconds
    pub took_ms: u64,
}

/// One scored hit of a [`FulltextSearchResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextSearchResult {
    /// Document ID
    pub doc_id: String,
    /// BM25 relevance score
    pub score: f32,
    /// Optional highlight information for matched fields
    #[serde(default)]
    pub highlights: Option<Vec<HighlightResult>>,
}

/// Highlight information for a matched field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightResult {
    /// Field name that matched
    pub field: String,
    /// Highlighted text fragments
    pub fragments: Vec<String>,
}

/// Create a fulltext index request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFulltextIndexRequest {
    /// Index name
    pub index_name: String,
    /// Schema (tag) name
    pub schema_name: String,
    /// Fields to index
    pub fields: Vec<FulltextFieldDef>,
    /// Whether to skip creation if index already exists
    #[serde(default)]
    pub if_not_exists: Option<bool>,
}

/// Fulltext field definition for index creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextFieldDef {
    /// Field name
    pub field_name: String,
    /// Analyzer to use (e.g., "standard", "jieba", "raw")
    #[serde(default)]
    pub analyzer: Option<String>,
    /// Field boost factor for scoring
    #[serde(default)]
    pub boost: Option<f32>,
}

/// Drop fulltext index request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropFulltextIndexRequest {
    /// Index name to drop
    pub index_name: String,
    /// Whether to skip error if index doesn't exist
    #[serde(default)]
    pub if_exists: Option<bool>,
}

/// Fulltext index information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextIndexInfo {
    /// Index name
    pub index_name: String,
    /// Space (namespace) ID
    pub space_id: u64,
    /// Tag (vertex type) name
    pub tag_name: String,
    /// Field name
    pub field_name: String,
    /// Index status
    pub status: String,
    /// Number of documents indexed
    pub doc_count: u64,
}

/// List fulltext indexes response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListFulltextIndexesResponse {
    /// All fulltext indexes
    pub indexes: Vec<FulltextIndexInfo>,
}
