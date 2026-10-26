//! Fulltext protocol DTOs.
//!
//! Transport-independent request/response shapes for fulltext index management
//! and search, shared between the server handlers and CLI clients.
//!
//! Only the shapes actually served over HTTP live here: async rebuild/clear
//! tasks. Search and index listing travel as plain JSON values because the
//! server handlers define their own response shapes.

use serde::{Deserialize, Serialize};

/// Rebuild a fulltext index request.
///
/// The rebuild runs asynchronously: the handler returns a rebuild id
/// immediately and the caller polls the rebuild status endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RebuildFulltextIndexRequest {
    /// Space (namespace) ID
    pub space_id: u64,
    /// Tag (vertex type) name
    pub tag_name: String,
    /// Field name
    pub field_name: String,
}

/// Rebuild a fulltext index response (accepted async task).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
