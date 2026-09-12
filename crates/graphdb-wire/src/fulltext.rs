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

/// Rebuild a fulltext index request.
///
/// The rebuild runs asynchronously: the handler returns a rebuild id
/// immediately and the caller polls the rebuild status endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildFulltextIndexRequest {
    /// Space (namespace) ID
    pub space_id: u64,
    /// Tag (vertex type) name
    pub tag_name: String,
    /// Field name
    pub field_name: String,
}

/// Rebuild a fulltext index response (accepted async task).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildFulltextIndexResponse {
    /// Async rebuild task id for status polling
    pub rebuild_id: String,
    /// Initial task status (`running`)
    pub status: String,
    /// Space (namespace) ID
    pub space_id: u64,
    /// Tag (vertex type) name
    pub tag_name: String,
    /// Field name
    pub field_name: String,
}

/// Fulltext rebuild task status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextRebuildStatusResponse {
    /// Async rebuild task id
    pub rebuild_id: String,
    /// Task status (`running`, `completed`, `failed`)
    pub status: String,
    /// Current rebuild phase, if known
    #[serde(default)]
    pub phase: Option<String>,
    /// Outbox generation used by this rebuild attempt, if known
    #[serde(default)]
    pub generation: Option<u64>,
    /// Documents scanned from primary storage
    #[serde(default)]
    pub docs_scanned: u64,
    /// Documents applied (backfill + replay)
    #[serde(default)]
    pub docs_applied: u64,
    /// Documents skipped as unrepresentable
    #[serde(default)]
    pub docs_skipped: u64,
    /// Failure reason, if failed
    #[serde(default)]
    pub error: Option<String>,
}

/// Clear a fulltext index request.
///
/// Explicit destructive operation: drops all indexed documents without
/// backfill. Requires `force = true`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearFulltextIndexRequest {
    /// Space (namespace) ID
    pub space_id: u64,
    /// Tag (vertex type) name
    pub tag_name: String,
    /// Field name
    pub field_name: String,
    /// Explicit confirmation of the destructive clear
    pub force: bool,
}

/// Clear a fulltext index response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearFulltextIndexResponse {
    /// Always true on success
    pub ok: bool,
    /// Space (namespace) ID
    pub space_id: u64,
    /// Tag (vertex type) name
    pub tag_name: String,
    /// Field name
    pub field_name: String,
}
