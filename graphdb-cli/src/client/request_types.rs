//! HTTP request types

use serde::Serialize;
use std::collections::HashMap;

/// Login request
#[derive(Debug, Serialize)]
pub(crate) struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Logout request
#[derive(Debug, Serialize)]
pub(crate) struct LogoutRequest {
    pub session_id: i64,
}

/// Query request
#[derive(Debug, Serialize)]
pub(crate) struct QueryRequest {
    pub query: String,
    pub session_id: i64,
    #[serde(default)]
    pub parameters: HashMap<String, String>,
}

/// Begin transaction request
#[derive(Debug, Serialize)]
pub(crate) struct BeginTransactionRequest {
    pub session_id: i64,
    pub read_only: bool,
    pub timeout_seconds: Option<u64>,
}

/// Transaction action request (commit/rollback)
#[derive(Debug, Serialize)]
pub(crate) struct TransactionActionRequest {
    pub session_id: i64,
}

/// Create space request
#[derive(Debug, Serialize)]
pub(crate) struct CreateSpaceRequest {
    pub name: String,
    pub vid_type: Option<String>,
    pub comment: Option<String>,
}

/// Create tag request
#[derive(Debug, Serialize)]
pub(crate) struct CreateTagRequest {
    pub name: String,
    pub properties: Vec<PropertyDefInput>,
}

/// Create edge type request
#[derive(Debug, Serialize)]
pub(crate) struct CreateEdgeTypeRequest {
    pub name: String,
    pub properties: Vec<PropertyDefInput>,
}

/// Property definition input for schema creation
#[derive(Debug, Serialize)]
pub(crate) struct PropertyDefInput {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

/// Create batch request
#[derive(Debug, Serialize)]
pub(crate) struct CreateBatchRequest {
    pub space_id: u64,
    pub batch_type: String,
    pub batch_size: usize,
}

/// Add batch items request
#[derive(Debug, Serialize)]
pub(crate) struct AddBatchItemsRequest {
    pub items: Vec<BatchItem>,
}

/// Batch item for serialization
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type")]
pub(crate) enum BatchItem {
    #[serde(rename = "vertex")]
    Vertex(VertexData),
    #[serde(rename = "edge")]
    Edge(EdgeData),
}

/// Vertex data for serialization
#[derive(Debug, Serialize, Clone)]
pub(crate) struct VertexData {
    pub vid: serde_json::Value,
    pub tags: Vec<String>,
    pub properties: HashMap<String, serde_json::Value>,
}

/// Edge data for serialization
#[derive(Debug, Serialize, Clone)]
pub(crate) struct EdgeData {
    pub edge_type: String,
    pub src_vid: serde_json::Value,
    pub dst_vid: serde_json::Value,
    pub properties: HashMap<String, serde_json::Value>,
}

/// Validate query request
#[derive(Debug, Serialize)]
pub(crate) struct ValidateQueryRequest {
    pub query: String,
    pub session_id: Option<i64>,
}

/// Update config request
#[derive(Debug, Serialize)]
pub(crate) struct UpdateConfigRequest {
    pub section: String,
    pub key: String,
    pub value: serde_json::Value,
}

/// Create vector index request
#[derive(Debug, Serialize)]
pub(crate) struct CreateVectorIndexRequest {
    pub name: String,
    pub tag: String,
    pub field: String,
    pub dimension: usize,
    pub metric: String,
}

/// Vector search request
#[derive(Debug, Serialize)]
pub(crate) struct VectorSearchRequest {
    pub vector: Vec<f32>,
    pub top_k: usize,
}
