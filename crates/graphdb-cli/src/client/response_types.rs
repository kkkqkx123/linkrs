//! HTTP response types

use serde::Deserialize;

/// Login response
#[derive(Debug, Deserialize)]
pub(crate) struct LoginResponse {
    pub session_id: i64,
    pub username: String,
}

/// Transaction response
#[derive(Debug, Deserialize)]
pub(crate) struct TransactionResponse {
    pub transaction_id: u64,
    pub status: String,
}

/// Batch query response: one [`QueryResult`] per input statement, in order.
#[derive(Debug, Deserialize)]
pub(crate) struct BatchQueryResponse {
    pub results: Vec<crate::client::types::QueryResult>,
}

/// Create batch response
#[derive(Debug, Deserialize)]
pub(crate) struct CreateBatchResponse {
    pub batch_id: String,
}

/// Add batch items response
#[derive(Debug, Deserialize)]
pub(crate) struct AddBatchItemsResponse {
    pub accepted: usize,
}

/// Execute batch response
#[derive(Debug, Deserialize)]
pub(crate) struct ExecuteBatchResponse {
    pub batch_id: String,
    pub status: BatchStatusEnum,
    pub result: BatchResultData,
}

/// Batch result data
#[derive(Debug, Deserialize)]
pub(crate) struct BatchResultData {
    pub vertices_inserted: usize,
    pub edges_inserted: usize,
    pub errors: Vec<BatchErrorData>,
}

/// Batch error data
#[derive(Debug, Deserialize)]
pub(crate) struct BatchErrorData {
    pub index: usize,
    pub item_type: BatchItemType,
    pub error: String,
}

/// Batch item type
#[derive(Debug, Deserialize)]
pub(crate) enum BatchItemType {
    Vertex,
    Edge,
}

/// Batch status response
#[derive(Debug, Deserialize)]
pub(crate) struct BatchStatusResponse {
    pub batch_id: String,
    pub status: BatchStatusEnum,
    pub progress: BatchProgress,
}

/// Batch status enum
#[derive(Debug, Deserialize)]
pub(crate) enum BatchStatusEnum {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Batch progress
#[derive(Debug, Deserialize)]
pub(crate) struct BatchProgress {
    pub total: usize,
    pub processed: usize,
    pub succeeded: usize,
    pub failed: usize,
}

/// Vector search response
#[derive(Debug, Deserialize)]
pub(crate) struct VectorSearchResponse {
    pub total: usize,
    pub results: Vec<VectorMatchData>,
}

/// Vector match data
#[derive(Debug, Deserialize)]
pub(crate) struct VectorMatchData {
    pub vid: serde_json::Value,
    pub score: f32,
    pub properties: std::collections::HashMap<String, serde_json::Value>,
}
