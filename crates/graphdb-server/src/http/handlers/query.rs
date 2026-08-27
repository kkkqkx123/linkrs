use crate::value::{from_json as json_value_to_core, to_json as value_to_json};
use axum::{
    extract::{Json, State},
    response::Json as JsonResponse,
};
use graphdb_wire::query::{
    BatchQueryRequest, BatchQueryResponse, QueryData, QueryMetadata, QueryRequest, QueryResponse,
    ValidateResponse,
};

use crate::http::{error::HttpError, state::AppState};
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

/// Convert a core-layer [`QueryResult`] into the wire `QueryResponse`.
///
/// The core result carries the engine `ExecutionResult` unchanged; each
/// variant is rendered here into the JSON wire shape (rows stay in column
/// order, no intermediate map conversion).
fn query_result_to_response(result: graphdb_api::core::QueryResult) -> QueryResponse {
    // `metadata.space_id` surfaces the switched-to space. The engine executes
    // USE as a DataSet with a `space_id` column (the `SpaceSwitched` variant
    // is never produced); `QueryResult::space_summary` recognizes both.
    let space_id = result.space_summary().map(|s| s.id);
    let (columns, rows): (
        Vec<String>,
        Vec<std::collections::HashMap<String, serde_json::Value>>,
    ) = match result.execution {
        crate::query::executor::base::ExecutionResult::DataSet { data } => {
            let rows = data
                .rows
                .iter()
                .map(|row| {
                    data.col_names
                        .iter()
                        .zip(row.iter())
                        .map(|(col, value)| (col.clone(), value_to_json(value.clone())))
                        .collect()
                })
                .collect();
            (data.col_names, rows)
        }
        crate::query::executor::base::ExecutionResult::SpaceSwitched(summary) => {
            let row = std::collections::HashMap::from([
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
                    serde_json::Value::String(summary.vid_type.to_string()),
                ),
            ]);
            (
                vec![
                    "space_name".to_string(),
                    "space_id".to_string(),
                    "vid_type".to_string(),
                ],
                vec![row],
            )
        }
        _ => (vec![], vec![]),
    };
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
