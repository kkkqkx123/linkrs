//! Vector protocol DTOs.
//!
//! Transport-independent request/response shapes for vector index management,
//! shared between the server handlers and CLI clients.

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

/// Vector index creation receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VectorIndexCreatedResponse {
    pub success: bool,
    pub message: String,
    pub collection_name: String,
}

/// Vector index drop receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VectorIndexDroppedResponse {
    pub success: bool,
    pub message: String,
}

/// One vector point.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VectorPoint {
    pub id: String,
    #[serde(default)]
    pub vector: Option<Vec<f32>>,
    #[serde(default)]
    #[schema(value_type = Option<std::collections::HashMap<String, crate::query::CellValue>>)]
    pub payload: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Vector point lookup payload. Missing points report success with
/// a message instead of a point.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VectorPointResponse {
    pub success: bool,
    #[serde(default)]
    pub point: Option<VectorPoint>,
    #[serde(default)]
    pub message: Option<String>,
}

/// Vector point count payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VectorCountResponse {
    pub success: bool,
    pub count: u64,
}

/// Payload mutation receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PayloadMutationResponse {
    pub success: bool,
    pub message: String,
}
