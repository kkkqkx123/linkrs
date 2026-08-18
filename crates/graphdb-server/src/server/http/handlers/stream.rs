use std::sync::Arc;

use crate::value::to_json as value_to_json;
use axum::{
    extract::{Json, State},
    response::{sse::Event, Sse},
};
use graphdb_wire::query::StreamQueryRequest;
use serde::Serialize;
use serde_json::json;
use tokio_stream::wrappers::ReceiverStream;

use crate::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

/// Streaming results data items
#[derive(Debug, Serialize)]
struct StreamDataItem {
    pub row: serde_json::Value,
    pub index: usize,
}

/// Streaming results metadata
#[derive(Debug, Serialize)]
struct StreamMetadata {
    pub rows_returned: usize,
    pub execution_time_ms: u64,
    pub columns: Vec<String>,
}

/// Execute the query and stream the results
pub async fn execute_stream<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Json(request): Json<StreamQueryRequest>,
) -> Result<
    Sse<impl tokio_stream::Stream<Item = Result<Event, HttpError>> + Send + 'static>,
    HttpError,
> {
    let buffer_capacity = request.event_buffer_capacity.clamp(1, 1000);
    let server = state.server.clone();

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, HttpError>>(buffer_capacity);

    tokio::spawn(async move {
        let start_time = std::time::Instant::now();
        let graph_service = server.get_graph_service();

        // Get a streaming result handle (chunk-at-a-time).
        let stream_result = match graph_service
            .execute_stream(request.session_id, &request.query)
            .await
        {
            Ok(stream) => stream,
            Err(e) => {
                let error_msg = json!({
                    "error": true,
                    "message": e,
                    "code": "QUERY_ERROR"
                });
                let _ = tx
                    .send(Ok(Event::default()
                        .event("error")
                        .data(error_msg.to_string())))
                    .await;
                let _ = tx.send(Ok(Event::default().event("done").data("{}"))).await;
                return;
            }
        };

        // Send schema event BEFORE any row data, using column names from the
        // plan (available even for empty results via the fallback mechanism).
        let schema_sent: Arc<std::sync::atomic::AtomicBool> =
            Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(columns) = stream_result.column_names() {
            schema_sent.store(true, std::sync::atomic::Ordering::Relaxed);
            let schema = json!({
                "columns": columns,
                "column_count": columns.len(),
            });
            if let Ok(schema_str) = serde_json::to_string(&schema) {
                let _ = tx
                    .send(Ok(Event::default().event("schema").data(schema_str)))
                    .await;
            }
        }

        // Spawn a blocking task to pull chunks synchronously
        // and send rows through the channel.
        let tx_pull = tx.clone();
        let stream_result_pull = stream_result.clone();
        let schema_sent_pull = schema_sent.clone();
        let pull_handle = tokio::task::spawn_blocking(move || {
            let mut row_index: usize = 0;
            loop {
                match stream_result_pull.next_chunk() {
                    Ok(Some(chunk)) => {
                        let columns = chunk.col_names();

                        // Send schema event on first chunk if not already done.
                        if !schema_sent_pull.load(std::sync::atomic::Ordering::Relaxed) {
                            schema_sent_pull.store(true, std::sync::atomic::Ordering::Relaxed);
                            let schema = json!({
                                "columns": columns,
                                "column_count": columns.len(),
                            });
                            if let Ok(schema_str) = serde_json::to_string(&schema) {
                                if tx_pull
                                    .blocking_send(Ok(Event::default()
                                        .event("schema")
                                        .data(schema_str)))
                                    .is_err()
                                {
                                    stream_result_pull.cancel();
                                    return Ok(row_index);
                                }
                            }
                        }

                        for row in chunk.rows {
                            let obj: serde_json::Map<String, serde_json::Value> = row
                                .into_iter()
                                .enumerate()
                                .map(|(i, v)| {
                                    let col_name = columns.get(i).cloned().unwrap_or_default();
                                    (col_name, value_to_json(v))
                                })
                                .collect();
                            let item = StreamDataItem {
                                row: serde_json::Value::Object(obj),
                                index: row_index,
                            };
                            row_index += 1;

                            if let Ok(data) = serde_json::to_string(&item) {
                                if tx_pull
                                    .blocking_send(Ok(Event::default().data(data)))
                                    .is_err()
                                {
                                    // Client disconnected — cancel the query.
                                    stream_result_pull.cancel();
                                    return Ok(row_index);
                                }
                            }
                        }
                    }
                    Ok(None) => return Ok(row_index),
                    Err(e) => return Err(e),
                }
            }
        });

        // Wait for the pull task to finish.
        match pull_handle.await {
            Ok(Ok(total_rows)) => {
                // Send metadata summary AFTER all rows.
                let metadata = StreamMetadata {
                    rows_returned: total_rows,
                    execution_time_ms: start_time.elapsed().as_millis() as u64,
                    columns: Vec::new(), // schema was sent upfront
                };

                if let Ok(meta_str) = serde_json::to_string(&metadata) {
                    let _ = tx
                        .send(Ok(Event::default().event("metadata").data(meta_str)))
                        .await;
                }

                // Send Completion Event
                let _ = tx.send(Ok(Event::default().event("done").data("{}"))).await;
            }
            Ok(Err(e)) => {
                // Execution error — send error, then done.
                let error_msg = json!({
                    "error": true,
                    "message": e.to_string(),
                    "code": "QUERY_ERROR"
                });
                let _ = tx
                    .send(Ok(Event::default()
                        .event("error")
                        .data(error_msg.to_string())))
                    .await;
                let _ = tx.send(Ok(Event::default().event("done").data("{}"))).await;
            }
            Err(_) => {
                // Task panicked or cancelled — channel will be dropped.
            }
        }
    });

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(10))
            .text("keepalive"),
    ))
}
