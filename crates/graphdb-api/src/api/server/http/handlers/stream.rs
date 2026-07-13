use std::sync::Arc;

use axum::{
    extract::{Json, State},
    response::{sse::Event, Sse},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_stream::wrappers::ReceiverStream;

use crate::api::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

/// Streaming Query Requests
#[derive(Debug, Clone, Deserialize)]
pub struct StreamQueryRequest {
    pub query: String,
    pub session_id: i64,
    #[serde(default = "default_buffer_capacity")]
    pub event_buffer_capacity: usize,
}

fn default_buffer_capacity() -> usize {
    100
}

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
        + StorageTransactionContextOps
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

/// Convert Core Value to serde_json::Value
fn value_to_json(value: crate::core::Value) -> serde_json::Value {
    match value {
        crate::core::Value::Empty => serde_json::Value::Null,
        crate::core::Value::Null(_) => serde_json::Value::Null,
        crate::core::Value::Bool(b) => serde_json::Value::Bool(b),
        crate::core::Value::SmallInt(i) => serde_json::Value::Number(i.into()),
        crate::core::Value::Int(i) => serde_json::Value::Number(i.into()),
        crate::core::Value::BigInt(i) => serde_json::Value::Number(i.into()),
        crate::core::Value::Float(f) => serde_json::Value::Number(
            serde_json::Number::from_f64(f as f64).unwrap_or(serde_json::Number::from(0)),
        ),
        crate::core::Value::Double(f) => serde_json::Value::Number(
            serde_json::Number::from_f64(f).unwrap_or(serde_json::Number::from(0)),
        ),
        crate::core::Value::Decimal128(d) => serde_json::Value::String(d.to_string()),
        crate::core::Value::String(s) => serde_json::Value::String(s),
        crate::core::Value::FixedString { data, .. } => serde_json::Value::String(data),
        crate::core::Value::Blob(blob) => serde_json::Value::String(format!("{:?}", blob)),
        crate::core::Value::Date(d) => serde_json::Value::String(d.to_string()),
        crate::core::Value::Time(t) => serde_json::Value::String(t.to_string()),
        crate::core::Value::DateTime(dt) => serde_json::Value::String(dt.to_string()),
        crate::core::Value::Vertex(v) => serde_json::json!(v),
        crate::core::Value::Edge(e) => serde_json::json!(e),
        crate::core::Value::Path(p) => serde_json::json!(p),
        crate::core::Value::List(list) => {
            serde_json::Value::Array(list.into_iter().map(value_to_json).collect())
        }
        crate::core::Value::Map(map) => {
            let obj: serde_json::Map<String, serde_json::Value> = map
                .into_iter()
                .map(|(k, v)| (k, value_to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        crate::core::Value::Set(set) => {
            serde_json::Value::Array(set.into_iter().map(value_to_json).collect())
        }
        crate::core::Value::Geography(g) => serde_json::json!(g),
        crate::core::Value::Vector(v) => {
            let arr = v
                .to_dense()
                .iter()
                .map(|&f| {
                    serde_json::Number::from_f64(f as f64).unwrap_or(serde_json::Number::from(0))
                })
                .collect::<Vec<_>>();
            serde_json::Value::Array(arr.into_iter().map(serde_json::Value::Number).collect())
        }
        crate::core::Value::DataSet(ds) => serde_json::json!(ds),
        crate::core::Value::Json(j) => {
            serde_json::from_str(j.as_str()).unwrap_or(serde_json::Value::Null)
        }
        crate::core::Value::JsonB(j) => j.as_value().clone(),
        crate::core::Value::Uuid(u) => serde_json::Value::String(u.to_hyphenated_string()),
        crate::core::Value::Interval(i) => serde_json::Value::String(i.to_postgresql()),
    }
}
