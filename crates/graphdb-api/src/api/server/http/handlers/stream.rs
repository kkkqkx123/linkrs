//! Streaming Results HTTP Processor

use axum::{
    extract::{Json, State},
    response::{sse::Event, Sse},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_stream::wrappers::ReceiverStream;

use crate::api::server::http::{error::HttpError, state::AppState};
use crate::query::executor::ExecutionResult;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

/// Streaming Query Requests
#[derive(Debug, Clone, Deserialize)]
pub struct StreamQueryRequest {
    pub query: String,
    pub session_id: i64,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

fn default_batch_size() -> usize {
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
    let batch_size = request.batch_size.clamp(1, 1000);
    let server = state.server.clone();

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, HttpError>>(batch_size);

    tokio::spawn(async move {
        let start_time = std::time::Instant::now();
        let graph_service = server.get_graph_service();
        let request = request.clone();

        // perform a search
        let exec_result = match graph_service
            .execute(request.session_id, &request.query)
            .await
        {
            Ok(result) => result,
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

        // Convert execution results to streaming data
        let (rows, columns) = execution_result_to_stream_data(exec_result);
        let total_rows = rows.len();

        // Send data in batches
        for (index, row) in rows.into_iter().enumerate() {
            let item = StreamDataItem { row, index };

            if let Ok(data) = serde_json::to_string(&item) {
                if tx.send(Ok(Event::default().data(data))).await.is_err() {
                    // Client Disconnect
                    return;
                }
            }

            // Sleep briefly after each batch to avoid blocking
            if (index + 1) % batch_size == 0 {
                tokio::task::yield_now().await;
            }
        }

        // Send metadata
        let metadata = StreamMetadata {
            rows_returned: total_rows,
            execution_time_ms: start_time.elapsed().as_millis() as u64,
            columns,
        };

        if let Ok(meta_str) = serde_json::to_string(&metadata) {
            let _ = tx
                .send(Ok(Event::default().event("metadata").data(meta_str)))
                .await;
        }

        // Send Completion Event
        let _ = tx.send(Ok(Event::default().event("done").data("{}"))).await;
    });

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(10))
            .text("keepalive"),
    ))
}

/// Converting ExecutionResult to Streaming Data
fn execution_result_to_stream_data(
    result: ExecutionResult,
) -> (Vec<serde_json::Value>, Vec<String>) {
    match result {
        ExecutionResult::DataSet(dataset) => {
            let columns = dataset.col_names.clone();
            let rows: Vec<serde_json::Value> = dataset
                .rows
                .into_iter()
                .map(|row| {
                    let obj: serde_json::Map<String, serde_json::Value> = row
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| {
                            let col_name = columns.get(i).cloned().unwrap_or_default();
                            (col_name, value_to_json(v))
                        })
                        .collect();
                    serde_json::Value::Object(obj)
                })
                .collect();
            (rows, columns)
        }
        ExecutionResult::Empty | ExecutionResult::Success | ExecutionResult::SpaceSwitched(_) => {
            (vec![], vec![])
        }
        ExecutionResult::Error(msg) => (vec![json!({"error": msg})], vec!["error".to_string()]),
    }
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
            // Convert vector to JSON array of f64 values
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
            // Parse JSON text and convert to serde_json::Value
            serde_json::from_str(j.as_str()).unwrap_or(serde_json::Value::Null)
        }
        crate::core::Value::JsonB(j) => {
            // JSONB is already parsed, convert back to serde_json::Value
            j.as_value().clone()
        }
        crate::core::Value::Uuid(u) => serde_json::Value::String(u.to_hyphenated_string()),
        crate::core::Value::Interval(i) => serde_json::Value::String(i.to_postgresql()),
    }
}
