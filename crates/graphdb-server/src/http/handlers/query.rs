use crate::graph_service::GraphServiceError;
use crate::value::{from_json as json_value_to_core, to_json as value_to_json};
use axum::{
    extract::{Json, State},
    response::Json as JsonResponse,
};
use graphdb_metrics::{ErrorInfo, ErrorType, QueryMetrics, QueryPhase, QueryProfile, StatsManager};
use graphdb_wire::query::{
    BatchQueryRequest, BatchQueryResponse, ExplainRequest, QueryData, QueryMetadata, QueryRequest,
    QueryResponse, QueryStageTimings, ValidateRequest, ValidateResponse,
};
use std::sync::Arc;
use std::time::Instant;

use crate::http::{error::HttpError, state::AppState};
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
            match crate::http::handlers::config::resolve_query_config_intent(
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
                        GraphServiceError::new(e),
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
                    match crate::http::handlers::config::resolve_query_config_intent(
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
                                GraphServiceError::new(e),
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
    let graph_service = state.server.get_graph_service();
    let parameters = json_params_to_core(&request.parameters);
    let session_variables = json_params_to_core(&request.session_variables);
    let row_limit = result_size_limit(&state);
    let trace_id = new_trace_id();
    let start = Instant::now();
    match graph_service
        .explain(
            request.session_id,
            &request.query,
            parameters,
            session_variables,
        )
        .await
    {
        Ok(exec_result) => {
            let elapsed_us = start.elapsed().as_micros() as u64;
            Ok(JsonResponse(query_result_to_response(
                exec_result,
                row_limit,
                &trace_id,
                elapsed_us,
            )))
        }
        Err(e) => {
            let elapsed_us = start.elapsed().as_micros() as u64;
            record_failure_profile(
                &state.server.get_stats_manager().clone(),
                request.session_id,
                &request.query,
                trace_id.clone(),
                elapsed_us,
                e.message(),
            );
            Ok(JsonResponse(error_with_trace(
                "QUERY_ERROR".to_string(),
                e,
                trace_id,
                elapsed_us,
            )))
        }
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

fn new_trace_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn stages_for_elapsed_us(elapsed_us: u64) -> QueryStageTimings {
    QueryStageTimings {
        parse_ms: 0.0,
        validate_ms: 0.0,
        plan_ms: 0.0,
        optimize_ms: 0.0,
        execute_ms: elapsed_us as f64 / 1000.0,
    }
}

fn error_with_trace(
    code: String,
    error: GraphServiceError,
    trace_id: String,
    elapsed_us: u64,
) -> QueryResponse {
    let position = error
        .position()
        .map(|p| graphdb_wire::query::ErrorPosition {
            line: p.line,
            column: p.column,
        });
    let mut response =
        QueryResponse::error_with_position(code, error.message().to_string(), None, position);
    response.metadata.trace_id = Some(trace_id);
    response.metadata.stages = Some(stages_for_elapsed_us(elapsed_us));
    response.metadata.execution_time_ms = elapsed_us / 1000;
    response
}

fn record_success_profile(
    stats: &Arc<StatsManager>,
    session_id: i64,
    query_text: &str,
    trace_id: String,
    elapsed_us: u64,
    rows_returned: usize,
) {
    let mut profile = QueryProfile::new(session_id, query_text.to_string());
    profile.trace_id = trace_id;
    profile.total_duration_us = elapsed_us;
    profile.stages.execute_us = elapsed_us;
    profile.result_count = rows_returned;
    let mut metrics = QueryMetrics::new();
    metrics.execute_time_us = elapsed_us;
    metrics.total_time_us = elapsed_us;
    metrics.result_row_count = rows_returned;
    stats.record_query_metrics(&metrics);
    stats.record_query_profile(profile);
}

fn record_failure_profile(
    stats: &Arc<StatsManager>,
    session_id: i64,
    query_text: &str,
    trace_id: String,
    elapsed_us: u64,
    message: &str,
) {
    let mut profile = QueryProfile::new(session_id, query_text.to_string());
    profile.trace_id = trace_id;
    profile.total_duration_us = elapsed_us;
    profile.stages.execute_us = elapsed_us;
    let mut metrics = QueryMetrics::new();
    metrics.execute_time_us = elapsed_us;
    metrics.total_time_us = elapsed_us;
    stats.record_query_metrics(&metrics);
    let error_info = ErrorInfo::new(
        ErrorType::ExecutionError,
        QueryPhase::Execute,
        message.to_string(),
    );
    stats.record_failed_query(profile, error_info);
}

/// Convert a core-layer [`QueryResult`] into the wire `QueryResponse`.
/// The core result carries the engine `ExecutionResult` unchanged; each
/// variant is rendered here into the JSON wire shape (rows stay in column
/// order, no intermediate map conversion). When `row_limit` is set, rows
/// past the ceiling are dropped and the response is marked truncated.
fn query_result_to_response(
    result: graphdb_api::api_core::QueryResult,
    row_limit: Option<usize>,
    trace_id: &str,
    elapsed_us: u64,
) -> QueryResponse {
    // `metadata.space_id` surfaces the switched-to space. The engine executes
    // USE as a DataSet with a `space_id` column (the `SpaceSwitched` variant
    // is never produced); `QueryResult::space_summary` recognizes both.
    let space_id = result.space_summary().map(|s| s.id);
    let (columns, mut rows): (
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
    let truncated = match row_limit {
        Some(limit) if rows.len() > limit => {
            rows.truncate(limit);
            true
        }
        _ => false,
    };
    let row_count = rows.len();
    let execution_time_ms = if result.metadata.execution_time_ms == 0 {
        elapsed_us / 1000
    } else {
        result.metadata.execution_time_ms
    };

    QueryResponse::success(
        QueryData::new(columns, rows),
        QueryMetadata {
            execution_time_ms,
            rows_scanned: result.metadata.rows_scanned,
            rows_returned: row_count,
            space_id,
            truncated,
            trace_id: Some(trace_id.to_string()),
            stages: Some(stages_for_elapsed_us(elapsed_us)),
            plan_node_count: None,
            result_row_count: Some(row_count),
        },
    )
}

/// Read the configured result row ceiling (`None` means unlimited).
/// Shared by the materialized, streaming, and export paths so one
/// configuration value guards every result shape.
pub(crate) fn result_size_limit<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    state: &AppState<S>,
) -> Option<usize> {
    let store = state.server.config_store();
    let guard = store.read();
    let resources = guard.query_resource();
    if resources.has_result_size_limit() {
        Some(resources.max_result_size)
    } else {
        None
    }
}

/// Convert an HTTP request's JSON parameter map to core `Value` bindings.
/// Empty maps are passed through as `None` so the core sees no bindings.
pub(crate) fn json_params_to_core(
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
