use gpui_kit::{App, Task};
use linkrs_client::ClientError;
use linkrs_wire::batch::{
    BatchItem, BatchStatusResponse, BatchType, ExecuteBatchResponse,
};

use crate::connection::SessionStore;
use crate::runtime;

/// Create a batch task.
pub fn create(
    cx: &App,
    space_id: u64,
    batch_type: BatchType,
    batch_size: usize,
) -> Task<Result<String, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().create_batch(space_id, batch_type, batch_size).await
    })
}

/// Add items to a batch.
pub fn add_items(
    cx: &App,
    batch_id: &str,
    items: Vec<BatchItem>,
) -> Task<Result<usize, ClientError>> {
    let manager = SessionStore::manager(cx);
    let batch_id = batch_id.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().add_batch_items(&batch_id, items).await
    })
}

/// Execute a batch.
pub fn execute(cx: &App, batch_id: &str) -> Task<Result<ExecuteBatchResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    let batch_id = batch_id.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().execute_batch(&batch_id).await
    })
}

/// Get batch status.
pub fn status(cx: &App, batch_id: &str) -> Task<Result<BatchStatusResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    let batch_id = batch_id.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_batch_status(&batch_id).await
    })
}

/// Cancel a batch.
pub fn cancel(cx: &App, batch_id: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let batch_id = batch_id.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().cancel_batch(&batch_id).await
    })
}

/// Delete a batch.
pub fn delete(cx: &App, batch_id: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let batch_id = batch_id.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().delete_batch(&batch_id).await
    })
}
