use axum::{
    extract::{Json, State},
    response::Json as JsonResponse,
};

use crate::api::server::http::handlers::query_types::*;
use crate::api::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

pub async fn execute<
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
    Json(request): Json<QueryRequest>,
) -> Result<JsonResponse<QueryResponse>, HttpError> {
    let graph_service = state.server.get_graph_service();

    let parameters = json_params_to_core(&request.parameters);
    let session_variables = json_params_to_core(&request.session_variables);

    // Executing Queries with GraphService
    let result = match graph_service
        .execute_with_params(
            request.session_id,
            &request.query,
            parameters,
            session_variables,
        )
        .await
    {
        Ok(exec_result) => {
            // Converting QueryResult to QueryResponse
            Ok::<_, HttpError>(query_result_to_response(exec_result))
        }
        Err(e) => Ok::<_, HttpError>(QueryResponse::error(
            "QUERY_ERROR".to_string(),
            e.to_string(),
            None,
        )),
    };

    Ok(JsonResponse(result?))
}

pub async fn execute_batch<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + crate::storage::AutoCommitBatchOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Json(request): Json<BatchQueryRequest>,
) -> Result<JsonResponse<BatchQueryResponse>, HttpError> {
    let graph_service = state.server.get_graph_service();

    let outcomes = graph_service
        .execute_batch(request.session_id, &request.statements)
        .await;

    let results = outcomes
        .into_iter()
        .map(|outcome| match outcome {
            Ok(exec_result) => query_result_to_response(exec_result),
            Err(e) => QueryResponse::error("QUERY_ERROR".to_string(), e, None),
        })
        .collect();

    Ok(JsonResponse(BatchQueryResponse { results }))
}

pub async fn validate<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
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

/// Converting core-layer QueryResult to QueryResponse.
///
/// The core result carries the engine's real execution metadata
/// (`execution_time_ms`, `rows_scanned`) as well as the result shape; the
/// response DTO is built from it directly (no intermediate
/// ExecutionResult round trip).
fn query_result_to_response(result: crate::api::core::QueryResult) -> QueryResponse {
    let space_id = extract_space_id(&result);
    let columns = result.columns.clone();
    let rows: Vec<std::collections::HashMap<String, serde_json::Value>> = result
        .rows
        .into_iter()
        .map(|row| {
            columns
                .iter()
                .filter_map(|col| {
                    row.get(col)
                        .cloned()
                        .map(|v| (col.clone(), value_to_json(v)))
                })
                .collect()
        })
        .collect();
    let row_count = rows.len();

    QueryResponse::success(
        QueryData::new(columns, rows),
        QueryMetadata {
            execution_time_ms: result.metadata.execution_time_ms,
            rows_scanned: result.metadata.rows_scanned,
            rows_returned: row_count,
            space_id,
        },
    )
}

/// Extract the space id from a USE-statement result row (the core converts
/// `SpaceSwitched` into a row with a `space_id` column).
fn extract_space_id(result: &crate::api::core::QueryResult) -> Option<u64> {
    let idx = result.columns.iter().position(|c| c == "space_id")?;
    let row = result.rows.first()?;
    match row.get(&result.columns[idx])? {
        crate::core::Value::BigInt(id) => Some(*id as u64),
        _ => None,
    }
}

/// Convert an HTTP request's JSON parameter map to core `Value` bindings.
/// Empty maps are passed through as `None` so the core sees no bindings.
fn json_params_to_core(
    params: &std::collections::HashMap<String, serde_json::Value>,
) -> Option<std::collections::HashMap<String, crate::core::Value>> {
    if params.is_empty() {
        return None;
    }
    Some(
        params
            .iter()
            .map(|(k, v)| (k.clone(), json_value_to_core(v)))
            .collect(),
    )
}

fn json_value_to_core(value: &serde_json::Value) -> crate::core::Value {
    match value {
        serde_json::Value::Null => crate::core::Value::Null(Default::default()),
        serde_json::Value::Bool(b) => crate::core::Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                crate::core::Value::BigInt(i)
            } else if let Some(u) = n.as_u64() {
                crate::core::Value::BigInt(u as i64)
            } else {
                crate::core::Value::Double(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => crate::core::Value::string(s.as_str()),
        serde_json::Value::Array(items) => crate::core::Value::list(crate::core::List::from_vec(
            items.iter().map(json_value_to_core).collect(),
        )),
        serde_json::Value::Object(map) => crate::core::Value::Map(Box::new(
            map.iter()
                .map(|(k, v)| (crate::core::Value::string(k.clone()), json_value_to_core(v)))
                .collect(),
        )),
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
        crate::core::Value::String(s) => serde_json::Value::String(s.to_string()),
        crate::core::Value::Date(d) => serde_json::Value::String(d.to_string()),
        crate::core::Value::DateTime(dt) => serde_json::Value::String(dt.to_string()),
        crate::core::Value::Time(t) => serde_json::Value::String(t.to_string()),
        crate::core::Value::List(list) => {
            serde_json::Value::Array(list.into_iter().map(value_to_json).collect())
        }
        crate::core::Value::Map(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(k, v)| (format!("{}", k), value_to_json(v)))
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
