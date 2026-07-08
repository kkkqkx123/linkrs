//! Batch Operation Type Definition

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Batch Task ID
pub type BatchId = String;

/// Batch Task Status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    /// Created
    Created,
    /// running
    Running,
    /// done
    Completed,
    /// failed
    Failed,
    /// Cancelled
    Cancelled,
}

/// Batch task type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchType {
    /// Vertex Batch Insertion
    Vertex,
    /// Batch insertion of edges
    Edge,
    /// Mixed batch insertion
    Mixed,
}

/// Batch item type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchItemType {
    /// vertice
    Vertex,
    /// suffix of a noun of locality
    Edge,
}

/// Batch data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum BatchItem {
    #[serde(rename = "vertex")]
    Vertex(VertexData),
    #[serde(rename = "edge")]
    Edge(EdgeData),
}

/// vertex data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VertexData {
    /// Vertex ID
    pub vid: serde_json::Value,
    /// Tag List
    #[serde(default)]
    pub tags: Vec<String>,
    /// causality
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
}

/// boundary data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeData {
    /// side type
    pub edge_type: String,
    /// Start Vertex ID
    pub src_vid: serde_json::Value,
    /// Target Vertex ID
    pub dst_vid: serde_json::Value,
    /// Attribute
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
}

/// Creating Batch Task Requests
#[derive(Debug, Clone, Deserialize)]
pub struct CreateBatchRequest {
    /// Image Space ID
    pub space_id: u64,
    /// Batch Task Type
    pub batch_type: BatchType,
    /// Batch size
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

fn default_batch_size() -> usize {
    1000
}

/// Creating Batch Task Responses
#[derive(Debug, Clone, Serialize)]
pub struct CreateBatchResponse {
    /// Batch Task ID
    pub batch_id: BatchId,
    /// Task status
    pub status: BatchStatus,
    /// Creation time
    pub created_at: String,
}

/// Add bulk item request
#[derive(Debug, Clone, Deserialize)]
pub struct AddBatchItemsRequest {
    /// Batch item list
    pub items: Vec<BatchItem>,
}

/// Adding a Bulk Item Response
#[derive(Debug, Clone, Serialize)]
pub struct AddBatchItemsResponse {
    /// Number accepted
    pub accepted: usize,
    /// Number of buffered
    pub buffered: usize,
    /// Total number of buffers
    pub total_buffered: usize,
}

/// Perform batch task response
#[derive(Debug, Clone, Serialize)]
pub struct ExecuteBatchResponse {
    /// Batch Task ID
    pub batch_id: BatchId,
    /// Task Status
    pub status: BatchStatus,
    /// Implementation results
    pub result: BatchResultData,
    /// Completion time
    pub completed_at: Option<String>,
}

/// Batch results data
#[derive(Debug, Clone, Serialize)]
pub struct BatchResultData {
    /// Number of vertices inserted
    pub vertices_inserted: usize,
    /// Number of inserted edges
    pub edges_inserted: usize,
    /// error message
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<BatchErrorData>,
}

/// Batch Error Data
#[derive(Debug, Clone, Serialize)]
pub struct BatchErrorData {
    /// Index where the error occurred
    pub index: usize,
    /// Type of error
    pub item_type: BatchItemType,
    /// Error message
    pub error: String,
}

/// Status responses for batch tasks
#[derive(Debug, Clone, Serialize)]
pub struct BatchStatusResponse {
    /// Batch Task ID
    pub batch_id: BatchId,
    /// Task Status
    pub status: BatchStatus,
    /// Progress information
    pub progress: BatchProgress,
    /// Creation time
    pub created_at: String,
    /// Update time
    pub updated_at: String,
}

/// Progress of batch tasks
#[derive(Debug, Clone, Serialize)]
pub struct BatchProgress {
    /// Total quantity
    pub total: usize,
    /// Number of items processed
    pub processed: usize,
    /// Number of successes
    pub succeeded: usize,
    /// Number of failures
    pub failed: usize,
    /// Number of buffers
    pub buffered: usize,
}

/// Batch task information (for internal use)
#[derive(Debug, Clone)]
pub struct BatchTask {
    /// Task ID
    pub id: BatchId,
    /// Figure Space ID
    pub space_id: u64,
    /// Batch Type
    pub batch_type: BatchType,
    /// Batch size
    pub batch_size: usize,
    /// Task Status
    pub status: BatchStatus,
    /// Buffered items
    pub buffered_items: Vec<BatchItem>,
    /// Progress
    pub progress: BatchProgress,
    /// Result
    pub result: Option<BatchResultData>,
    /// Creation time
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Update time
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl BatchTask {
    /// Create a new batch task.
    pub fn new(id: BatchId, space_id: u64, batch_type: BatchType, batch_size: usize) -> Self {
        let now = chrono::Utc::now();
        Self {
            id,
            space_id,
            batch_type,
            batch_size,
            status: BatchStatus::Created,
            buffered_items: Vec::new(),
            progress: BatchProgress {
                total: 0,
                processed: 0,
                succeeded: 0,
                failed: 0,
                buffered: 0,
            },
            result: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Update status
    pub fn update_status(&mut self, status: BatchStatus) {
        self.status = status;
        self.updated_at = chrono::Utc::now();
    }

    /// Add a buffer item
    pub fn add_items(&mut self, items: Vec<BatchItem>) -> usize {
        let count = items.len();
        self.buffered_items.extend(items);
        self.progress.buffered = self.buffered_items.len();
        self.progress.total += count;
        self.updated_at = chrono::Utc::now();
        count
    }

    /// Retrieve and clear the buffer items.
    pub fn take_buffered_items(&mut self) -> Vec<BatchItem> {
        let items = std::mem::take(&mut self.buffered_items);
        self.progress.buffered = 0;
        self.updated_at = chrono::Utc::now();
        items
    }

    /// Update progress
    pub fn update_progress(&mut self, succeeded: usize, failed: usize) {
        self.progress.succeeded += succeeded;
        self.progress.failed += failed;
        self.progress.processed += succeeded + failed;
        self.updated_at = chrono::Utc::now();
    }

    /// Set the results
    pub fn set_result(&mut self, result: BatchResultData) {
        self.result = Some(result);
        self.updated_at = chrono::Utc::now();
    }
}
