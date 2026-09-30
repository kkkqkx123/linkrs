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

#[utoipa::path(
    post,
    path = "/v1/query",
    tag = "Query",
    request_body = QueryRequest,
    responses(
        (status = 200, body = QueryResponse, description = "Query executed"),
        (status = 500, description = "Internal error")
    )
)]
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

    // Map consistency from wire to API consistency
    let consistency = match request.consistency.as_deref() {
        Some(s)
            if s.eq_ignore_ascii_case("read_your_writes")
                || s.eq_ignore_ascii_case("ryw")
                || s.eq_ignore_ascii_case("read-your-writes") =>
        {
            graphdb_api::api_core::types::ConsistencyLevel::ReadYourWrites(
                graphdb_core::types::ReadYourWritesConfig {
                    timeout_ms: request.consistency_timeout_ms.unwrap_or(2000),
                    minimum_lsn: request.minimum_lsn.map(graphdb_core::types::CommitLsn::new),
                },
            )
        }
        _ if request.consistency_timeout_ms.is_some() => {
            graphdb_api::api_core::types::ConsistencyLevel::ReadYourWrites(
                graphdb_core::types::ReadYourWritesConfig {
                    timeout_ms: request.consistency_timeout_ms.unwrap(),
                    minimum_lsn: request.minimum_lsn.map(graphdb_core::types::CommitLsn::new),
                },
            )
        }
        _ => graphdb_api::api_core::types::ConsistencyLevel::Eventual,
    };

    // Executing Queries with GraphService
    let result = match graph_service
        .execute_with_consistency(
            request.session_id,
            &request.query,
            parameters,
            session_variables,
            consistency,
        )
        .await
    {
        Ok(exec_result) => {
            // Configuration intents (`UPDATE CONFIGS` / `SHOW CONFIGS`)
            // resolve against the live store here; anything else passes
            // through to the standard response mapping.
            let store = state.server.config_store();
            let config_path = state.server.get_config_path();
            match crate::http::handlers::config::resolve_query_config_intent(
                exec_result,
                &store,
                config_path.as_deref(),
            ) {
                Ok(resolved) => Ok::<_, HttpError>(query_result_to_response(resolved)),
                Err(e) => Ok::<_, HttpError>(QueryResponse::error(
                    "CONFIG_ERROR".to_string(),
                    e,
                    None,
                )),
            }
        }
        Err(e) => Ok::<_, HttpError>(QueryResponse::error(
            "QUERY_ERROR".to_string(),
            e.to_string(),
            None,
        )),
    };

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    post,
    path = "/v1/query/batch",
    tag = "Query",
    request_body = BatchQueryRequest,
    responses(
        (status = 200, body = BatchQueryResponse, description = "Batch queries executed"),
        (status = 500, description = "Internal error")
    )
)]
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

    let store = state.server.config_store();
    let config_path = state.server.get_config_path();
    let results = outcomes
        .into_iter()
        .map(|outcome| match outcome {
            Ok(exec_result) => {
                match crate::http::handlers::config::resolve_query_config_intent(
                    exec_result,
                    &store,
                    config_path.as_deref(),
                ) {
                    Ok(resolved) => query_result_to_response(resolved),
                    Err(e) => QueryResponse::error("CONFIG_ERROR".to_string(), e, None),
                }
            }
            Err(e) => QueryResponse::error("QUERY_ERROR".to_string(), e, None),
        })
        .collect();

    Ok(JsonResponse(BatchQueryResponse { results }))
}

#[utoipa::path(
    post,
    path = "/v1/query/validate",
    tag = "Query",
    request_body = QueryRequest,
    responses(
        (status = 200, body = ValidateResponse, description = "Query validation result"),
        (status = 500, description = "Internal error")
    )
)]
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
    // Real validation: parse plus binder name resolution, without executing.
    match validate_gql(&request.query) {
        Ok(_) => Ok(JsonResponse(ValidateResponse {
            valid: true,
            message: "Query is valid".to_string(),
        })),
        Err(e) => Ok(JsonResponse(ValidateResponse {
            valid: false,
            message: e,
        })),
    }
}

/// Validate query text by parsing and binding it, without executing.
///
/// Returns the declared parameter names on success, or a human-readable
/// reason on failure. Binding runs without a schema manager, so checks
/// that need live schema (e.g. tag existence) are skipped while syntax,
/// name resolution, and function resolution still apply.
pub(crate) fn validate_gql(query: &str) -> Result<Vec<String>, String> {
    if query.trim().is_empty() {
        return Err("query must not be empty".to_string());
    }
    let mut parser = crate::query::parser::Parser::new(query);
    let result = parser.parse().map_err(|e| format!("syntax error: {e}"))?;
    if parser.has_errors() {
        return Err(format!("syntax error: {}", parser.take_errors()));
    }
    crate::query::binder::Binder::new()
        .bind(result.ast)
        .map_err(|e| format!("semantic error: {e}"))?;
    Ok(extract_parameter_names(query))
}

/// Collect `@name` parameter references from query text.
pub(crate) fn extract_parameter_names(query: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes = query.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > i + 1 {
                let name = &query[i + 1..j];
                if !names.iter().any(|n: &String| n == name) {
                    names.push(name.to_string());
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    names
}

/// Convert a core-layer [`QueryResult`] into the wire `QueryResponse`.
///
/// The core result carries the engine `ExecutionResult` unchanged; each
/// variant is rendered here into the JSON wire shape (rows stay in column
/// order, no intermediate map conversion).
fn query_result_to_response(result: graphdb_api::api_core::QueryResult) -> QueryResponse {
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
) -> Option<std::collections::HashMap<String, graphdb_core::Value>> {
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
