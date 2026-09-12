//! Vector protocol DTOs.
//!
//! Transport-independent request/response shapes for vector search and
//! payload index management, shared between the server handlers and CLI
//! clients. Generic payload/filter types come from `graphdb-core::core::vector`
//! so the wire layer never depends on the storage engine.

use serde::{Deserialize, Serialize};

pub use graphdb_core::vector::{Payload, PayloadSchemaType, PayloadSelector, VectorFilter};

/// Vector search request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchRequest {
    pub collection: String,
    pub vector: Vec<f32>,
    pub top_k: usize,
    #[serde(default)]
    pub filter: Option<VectorFilter>,
    #[serde(default)]
    pub with_payload: Option<bool>,
    #[serde(default)]
    pub with_vector: Option<bool>,
    /// Returned-payload field projection (include / exclude lists).
    #[serde(default)]
    pub payload_selector: Option<PayloadSelector>,
}

/// Vector search response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchResponse {
    pub results: Vec<VectorSearchResult>,
}

/// One scored hit of a [`VectorSearchResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchResult {
    pub id: String,
    /// "Higher is better" similarity score, normalized across backends.
    pub score: f32,
    #[serde(default)]
    pub payload: Option<Payload>,
    #[serde(default)]
    pub vector: Option<Vec<f32>>,
}

/// Create a payload field index on a collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePayloadIndexRequest {
    pub collection: String,
    pub field: String,
    pub schema_type: PayloadSchemaType,
}

/// Delete the payload field index on a collection's field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletePayloadIndexRequest {
    pub collection: String,
    pub field: String,
}

/// One declared payload index of [`ListPayloadIndexesResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayloadIndexInfo {
    pub field: String,
    pub schema_type: PayloadSchemaType,
}

/// All declared payload indexes of one collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPayloadIndexesResponse {
    pub collection: String,
    pub indexes: Vec<PayloadIndexInfo>,
}

/// Rebuild a vector index request.
///
/// The rebuild runs asynchronously: the handler returns a rebuild id
/// immediately and the caller polls the rebuild status endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildVectorIndexRequest {
    /// Space (namespace) ID
    pub space_id: u64,
    /// Tag (vertex type) name
    pub tag_name: String,
    /// Field name
    pub field_name: String,
}

/// Rebuild a vector index response (accepted async task).
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
