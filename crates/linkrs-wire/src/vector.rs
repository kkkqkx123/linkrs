//! Vector protocol DTOs.
//!
//! Transport-independent request/response shapes for vector index management,
//! shared between the server handlers and CLI clients.
//!
//! Only the shapes actually served over HTTP live here: async rebuild/clear
//! tasks. Search, scroll, payload, and index listing travel as plain JSON
//! values because the server handlers define their own response shapes.

use serde::{Deserialize, Serialize};

/// Rebuild a vector index request.
///
/// The rebuild runs asynchronously: the handler returns a rebuild id
/// immediately and the caller polls the rebuild status endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RebuildVectorIndexRequest {
    /// Space (namespace) ID
    pub space_id: u64,
    /// Tag (vertex type) name
    pub tag_name: String,
    /// Field name
    pub field_name: String,
}

/// Rebuild a vector index response (accepted async task).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RebuildVectorIndexResponse {
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

/// Vector rebuild task status.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VectorRebuildStatusResponse {
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
    /// Vertex snapshots scanned from primary storage
    #[serde(default)]
    pub vectors_scanned: u64,
    /// Point operations applied (backfill + replay)
    #[serde(default)]
    pub vectors_applied: u64,
    /// Snapshots skipped as unrepresentable
    #[serde(default)]
    pub vectors_skipped: u64,
    /// Failure reason, if failed
    #[serde(default)]
    pub error: Option<String>,
}

/// Clear a vector index request.
///
/// Explicit destructive operation: drops all indexed points without
/// backfill. Requires `force = true`.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ClearVectorIndexRequest {
    /// Space (namespace) ID
    pub space_id: u64,
    /// Tag (vertex type) name
    pub tag_name: String,
    /// Field name
    pub field_name: String,
    /// Explicit confirmation of the destructive clear
    pub force: bool,
}

/// Clear a vector index response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ClearVectorIndexResponse {
    /// Always true on success
    pub ok: bool,
    /// Space (namespace) ID
    pub space_id: u64,
    /// Tag (vertex type) name
    pub tag_name: String,
    /// Field name
    pub field_name: String,
}
