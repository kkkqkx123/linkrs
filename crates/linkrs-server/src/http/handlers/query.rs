use axum::{
    extract::{Json, Path, State},
    response::Json as JsonResponse,
};
use linkrs_wire::query::{
    BatchQueryRequest, BatchQueryResponse, ExplainRequest, QueryRequest, QueryResponse,
    ValidateRequest, ValidateResponse,
};
use std::time::Instant;

use crate::http::error::HttpError;
use crate::http::state::AppState;
use crate::services::query::{
    error_with_trace, new_trace_id, query_result_to_response, record_failure_profile,
    record_success_profile, validate_gql,
};
pub(crate) use crate::services::query::{json_params_to_core, result_size_limit};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

#[utoipa::path(
    post,
    operation_id = "post_v1_query",
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
            linkrs_api::api_core::types::ConsistencyLevel::ReadYourWrites(
                linkrs_core::types::ReadYourWritesConfig {
                    timeout_ms: request.consistency_timeout_ms.unwrap_or(2000),
                    minimum_lsn: request.minimum_lsn.map(linkrs_core::types::CommitLsn::new),
                },
            )
        }
        _ if request.consistency_timeout_ms.is_some() => {
            linkrs_api::api_core::types::ConsistencyLevel::ReadYourWrites(
                linkrs_core::types::ReadYourWritesConfig {
                    timeout_ms: request.consistency_timeout_ms.unwrap(),
                    minimum_lsn: request.minimum_lsn.map(linkrs_core::types::CommitLsn::new),
                },
            )
        }
        _ => linkrs_api::api_core::types::ConsistencyLevel::Eventual,
    };

    // Executing Queries with GraphService
    let row_limit = result_size_limit(&state);
    let stats = state.server.get_stats_manager().clone();
    let trace_id = new_trace_id();
    let start = Instant::now();
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
            match crate::services::config::resolve_query_config_intent(
                exec_result,
                &store,
                config_path.as_deref(),
            ) {
                Ok(resolved) => {
                    let elapsed_us = start.elapsed().as_micros() as u64;
                    let response =
                        query_result_to_response(resolved, row_limit, &trace_id, elapsed_us);
                    record_success_profile(
                        &stats,
                        request.session_id,
                        &request.query,
                        trace_id.clone(),
                        elapsed_us,
                        response.metadata.rows_returned,
                    );
                    Ok::<_, HttpError>(response)
                }
                Err(e) => {
                    let elapsed_us = start.elapsed().as_micros() as u64;
                    record_failure_profile(
                        &stats,
                        request.session_id,
                        &request.query,
                        trace_id.clone(),
                        elapsed_us,
                        &e,
                    );
                    Ok::<_, HttpError>(error_with_trace(
                        "CONFIG_ERROR".to_string(),
                        crate::graph_service::GraphServiceError::new(e),
                        trace_id,
                        elapsed_us,
                    ))
                }
            }
        }
        Err(e) => {
            let elapsed_us = start.elapsed().as_micros() as u64;
            record_failure_profile(
                &stats,
                request.session_id,
                &request.query,
                trace_id.clone(),
                elapsed_us,
                e.message(),
            );
            Ok::<_, HttpError>(error_with_trace(
                "QUERY_ERROR".to_string(),
                e,
                trace_id,
                elapsed_us,
            ))
        }
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

    let parameters = json_params_to_core(&request.parameters);
    let session_variables = json_params_to_core(&request.session_variables);

    let outcomes = graph_service
        .execute_batch(
            request.session_id,
            &request.statements,
            parameters,
            session_variables,
        )
        .await;

    let store = state.server.config_store();
    let config_path = state.server.get_config_path();
    let row_limit = result_size_limit(&state);
    let stats = state.server.get_stats_manager().clone();
    let batch_start = Instant::now();
    let results = outcomes
        .into_iter()
        .enumerate()
        .map(|(index, outcome)| {
            let trace_id = new_trace_id();
            let elapsed_us = batch_start.elapsed().as_micros() as u64;
            let query_text = request.statements.get(index).cloned().unwrap_or_default();
            match outcome {
                Ok(exec_result) => {
                    match crate::services::config::resolve_query_config_intent(
                        exec_result,
                        &store,
                        config_path.as_deref(),
                    ) {
                        Ok(resolved) => {
                            let response = query_result_to_response(
                                resolved, row_limit, &trace_id, elapsed_us,
                            );
                            record_success_profile(
                                &stats,
                                request.session_id,
                                &query_text,
                                trace_id,
                                elapsed_us,
                                response.metadata.rows_returned,
                            );
                            response
                        }
                        Err(e) => {
                            record_failure_profile(
                                &stats,
                                request.session_id,
                                &query_text,
                                trace_id.clone(),
                                elapsed_us,
                                &e,
                            );
                            error_with_trace(
                                "CONFIG_ERROR".into(),
                                crate::graph_service::GraphServiceError::new(e),
                                trace_id,
                                elapsed_us,
                            )
                        }
                    }
                }
                Err(e) => {
                    record_failure_profile(
                        &stats,
                        request.session_id,
                        &query_text,
                        trace_id.clone(),
                        elapsed_us,
                        e.message(),
                    );
                    error_with_trace("QUERY_ERROR".into(), e, trace_id, elapsed_us)
                }
            }
        })
        .collect();

    Ok(JsonResponse(BatchQueryResponse { results }))
}

#[utoipa::path(
    post,
    path = "/v1/query/validate",
    tag = "Query",
    request_body = ValidateRequest,
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
    State(state): State<AppState<S>>,
    Json(request): Json<ValidateRequest>,
) -> Result<JsonResponse<ValidateResponse>, HttpError> {
    // Real validation: parse plus binder name resolution, without executing.
    match validate_gql(&request.query) {
        Ok(_) => {
            // Advisory row estimate for automatic routing. Command-like
            // statements never stream, so they carry no estimate.
            let estimated_rows = if !request.need_estimate
                || crate::graph_service::GraphService::<S>::is_command_like(&request.query)
            {
                None
            } else {
                state
                    .server
                    .get_graph_service()
                    .estimate_rows(request.session_id, &request.query)
                    .await
            };
            Ok(JsonResponse(ValidateResponse {
                valid: true,
                message: "Query is valid".to_string(),
                estimated_rows,
            }))
        }
        Err(e) => Ok(JsonResponse(ValidateResponse {
            valid: false,
            message: e,
            estimated_rows: None,
        })),
    }
}

#[utoipa::path(
    post,
    operation_id = "post_v1_query_explain",
    path = "/v1/query/explain",
    tag = "Query",
    request_body = ExplainRequest,
    responses(
        (status = 200, body = QueryResponse, description = "Query plan"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn explain<
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
    Json(request): Json<ExplainRequest>,
) -> Result<JsonResponse<QueryResponse>, HttpError> {
    Ok(JsonResponse(
        crate::services::query::explain(&state, request).await?,
    ))
}

/// List currently running queries (`SHOW QUERIES` over HTTP).
///
/// Returns the same `query_id / session_id / user_name / space_name /
/// status / duration_ms / query` rows as the Cypher `SHOW QUERIES`
/// statement so operators can discover `KILL QUERY` targets without
/// opening a query shell.
#[utoipa::path(
    get,
    operation_id = "get_v1_queries",
    path = "/v1/queries",
    tag = "Query",
    responses(
        (status = 200, body = serde_json::Value, description = "Running queries"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn list_queries<
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
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let result = graph_service
        .serve_show_queries()
        .map_err(|e| HttpError::internal(e.message().to_string()))?;
    let response = query_result_to_response(result, None, "list-queries", 0);
    Ok(JsonResponse(
        serde_json::to_value(&response).unwrap_or_default(),
    ))
}

/// Cancel a running query by `(session_id, query_id)`.
///
/// HTTP equivalent of the Cypher `KILL QUERY <session_id> <query_id>`
/// statement. Marks the tracked execution as killed and cancels the
/// shared execution token (plus a best-effort streaming cancel under the
/// same id). Returns `{session_id, query_id, status: "killed"}`.
#[utoipa::path(
    post,
    operation_id = "post_v1_sessions_queries_kill",
    path = "/v1/sessions/{id}/queries/{qid}/kill",
    tag = "Query",
    params(
        ("id" = i64, Path, description = "Session id"),
        ("qid" = i64, Path, description = "Query id from SHOW QUERIES")
    ),
    responses(
        (status = 200, body = serde_json::Value, description = "Query killed"),
        (status = 404, description = "Session or query not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn kill_query<
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
    Path((session_id, query_id)): Path<(i64, i64)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let result = graph_service
        .serve_kill_query(session_id, query_id)
        .map_err(|e| {
            let message = e.message().to_string();
            if message.contains("does not exist") {
                HttpError::not_found(message)
            } else {
                HttpError::transaction_message(message)
            }
        })?;
    let response = query_result_to_response(result, None, "kill-query", 0);
    Ok(JsonResponse(
        serde_json::to_value(&response).unwrap_or_default(),
    ))
}
