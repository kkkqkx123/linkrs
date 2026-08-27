//! Vector protocol DTOs.
//!
//! Transport-independent request/response shapes for vector search and
//! payload index management, shared between the server handlers and CLI
//! clients. Generic payload/filter types come from `graphdb-core::core::vector`
//! so the wire layer never depends on the storage engine.

use serde::{Deserialize, Serialize};

pub use graphdb_core::core::vector::{Payload, PayloadSchemaType, PayloadSelector, VectorFilter};

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
