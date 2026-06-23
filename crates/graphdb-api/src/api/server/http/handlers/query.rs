use axum::{
    extract::{Json, State},
    response::Json as JsonResponse,
};

use crate::api::server::http::handlers::query_types::*;
use crate::api::server::http::{error::HttpError, state::AppState};
use crate::query::executor::ExecutionResult;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

pub async fn execute<
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
    Json(request): Json<QueryRequest>,
) -> Result<JsonResponse<QueryResponse>, HttpError> {
    let graph_service = state.server.get_graph_service();

    // Executing Queries with GraphService
    let result = match graph_service
        .execute(request.session_id, &request.query)
        .await
    {
        Ok(exec_result) => {
            // Converting ExecutionResult to QueryResponse
            Ok::<_, HttpError>(execution_result_to_response(exec_result))
        }
        Err(e) => Ok::<_, HttpError>(QueryResponse::error(
            "QUERY_ERROR".to_string(),
            e.to_string(),
            None,
        )),
    };

    Ok(JsonResponse(result?))
}

pub async fn validate<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(_state): State<AppState<S>>,
    Json(request): Json<QueryRequest>,
) -> Result<JsonResponse<ValidateResponse>, HttpError> {
    // Simple validation: check if query is not empty
    let valid = !request.query.trim().is_empty();
    let message = if valid {
        "Query is valid".to_string()
    } else {
        "Query cannot be empty".to_string()
    };

    Ok(JsonResponse(ValidateResponse { valid, message }))
}

/// Converting ExecutionResult to QueryResponse
fn execution_result_to_response(result: ExecutionResult) -> QueryResponse {
    match result {
        ExecutionResult::DataSet(dataset) => {
            let columns: Vec<String> = dataset.col_names.clone();
            let rows: Vec<std::collections::HashMap<String, serde_json::Value>> = dataset
                .rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .enumerate()
                        .map(|(i, v)| {
                            let col_name = columns.get(i).cloned().unwrap_or_default();
                            (col_name, value_to_json(v))
                        })
                        .collect()
                })
                .collect();
            let row_count = rows.len();

            QueryResponse::success(
                QueryData::new(columns, rows),
                QueryMetadata {
                    execution_time_ms: 0,
                    rows_scanned: 0,
                    rows_returned: row_count,
                    space_id: None,
                },
            )
        }
        ExecutionResult::Success => QueryResponse::success(
            QueryData::new(vec![], vec![]),
            QueryMetadata {
                execution_time_ms: 0,
                rows_scanned: 0,
                rows_returned: 0,
                space_id: None,
            },
        ),
        ExecutionResult::Empty => QueryResponse::success(
            QueryData::new(vec![], vec![]),
            QueryMetadata {
                execution_time_ms: 0,
                rows_scanned: 0,
                rows_returned: 0,
                space_id: None,
            },
        ),
        ExecutionResult::SpaceSwitched(summary) => QueryResponse::success(
            QueryData::new(
                vec![
                    "space_name".to_string(),
                    "space_id".to_string(),
                    "vid_type".to_string(),
                ],
                vec![std::collections::HashMap::from([
                    (
                        "space_name".to_string(),
                        serde_json::Value::String(summary.name.clone()),
                    ),
                    (
                        "space_id".to_string(),
                        serde_json::Value::Number(summary.id.into()),
                    ),
                    (
                        "vid_type".to_string(),
                        serde_json::Value::String(format!("{:?}", summary.vid_type)),
                    ),
                ])],
            ),
            QueryMetadata {
                execution_time_ms: 0,
                rows_scanned: 0,
                rows_returned: 1,
                space_id: Some(summary.id),
            },
        ),
        ExecutionResult::Error(msg) => {
            QueryResponse::error("EXECUTION_ERROR".to_string(), msg, None)
        }
    }
}

fn value_to_json(value: crate::core::Value) -> serde_json::Value {
    match value {
        crate::core::Value::Null(_) => serde_json::Value::Null,
        crate::core::Value::Bool(b) => serde_json::Value::Bool(b),
        crate::core::Value::Int(i) => serde_json::Value::Number(i.into()),
        crate::core::Value::BigInt(i) => serde_json::Value::Number(i.into()),
        crate::core::Value::Float(f) => serde_json::Value::Number(
            serde_json::Number::from_f64(f as f64).unwrap_or(serde_json::Number::from(0)),
        ),
        crate::core::Value::Double(d) => serde_json::Value::Number(
            serde_json::Number::from_f64(d).unwrap_or(serde_json::Number::from(0)),
        ),
        crate::core::Value::String(s) => serde_json::Value::String(s),
        crate::core::Value::Date(d) => serde_json::Value::String(d.to_string()),
        crate::core::Value::DateTime(dt) => serde_json::Value::String(dt.to_string()),
        crate::core::Value::Time(t) => serde_json::Value::String(t.to_string()),
        crate::core::Value::List(list) => {
            serde_json::Value::Array(list.into_iter().map(value_to_json).collect())
        }
        crate::core::Value::Map(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, value_to_json(v)))
                .collect(),
        ),
        crate::core::Value::Vertex(v) => serde_json::json!({
            "id": v.vid.to_string(),
            "tags": v.tags,
        }),
        crate::core::Value::Edge(e) => serde_json::json!({
            "src": e.src.to_string(),
            "dst": e.dst.to_string(),
            "edge_type": e.edge_type,
        }),
        crate::core::Value::Path(p) => serde_json::json!({
            "src": p.src.vid.to_string(),
            "steps": p.steps.len(),
        }),
        _ => serde_json::Value::String(format!("{:?}", value)),
    }
}
