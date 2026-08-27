//! Batch Operation Type Definition
//!
//! Wire DTOs are shared with the CLI through `graphdb-wire`; this module
//! keeps only the server-internal [`BatchTask`] bookkeeping type.

pub use graphdb_wire::batch::{
    AddBatchItemsRequest, AddBatchItemsResponse, BatchErrorData, BatchId, BatchItem, BatchItemType,
    BatchProgress, BatchResultData, BatchStatus, BatchStatusResponse, BatchType,
    CreateBatchRequest, CreateBatchResponse, EdgeData, ExecuteBatchResponse, VertexData,
};

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
