//! Forward-only result cursor handlers: open, fetch, close.

use axum::{
    extract::{Json, State},
    response::Json as JsonResponse,
};
use graphdb_wire::query::{
    CloseCursorRequest, CloseCursorResponse, FetchCursorRequest, FetchCursorResponse,
    OpenCursorRequest, OpenCursorResponse,
};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::value::to_json as value_to_json;

/// Map cursor failures to HTTP status: client mistakes (bad session,
/// unknown cursor, unsupported statement) are 400, engine failures 500.
fn cursor_error(message: String) -> HttpError {
    if message.contains("Invalid session ID")
        || message.starts_with("Unknown cursor")
        || message.starts_with("Cursors support")
        || message.starts_with("Too many open cursors")
    {
        HttpError::bad_request(message)
    } else {
        HttpError::InternalError(message)
    }
}

#[utoipa::path(
    post,
    path = "/v1/query/cursor/open",
    tag = "Cursor",
    request_body = OpenCursorRequest,
    responses(
        (status = 200, body = OpenCursorResponse, description = "Cursor opened"),
        (status = 400, description = "Invalid cursor request"),
        (status = 500, description = "Internal error")
    )
)]
/// Open a forward-only cursor over a single statement's result.
pub async fn open_cursor<
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
    Json(request): Json<OpenCursorRequest>,
) -> Result<JsonResponse<OpenCursorResponse>, HttpError> {
    let (cursor_id, columns) = state
        .server
        .get_graph_service()
        .open_cursor(request.session_id, &request.query)
        .await
        .map_err(cursor_error)?;
    Ok(JsonResponse(OpenCursorResponse { cursor_id, columns }))
}

#[utoipa::path(
    post,
    path = "/v1/query/cursor/fetch",
    tag = "Cursor",
    request_body = FetchCursorRequest,
    responses(
        (status = 200, body = FetchCursorResponse, description = "Cursor page fetched"),
        (status = 400, description = "Invalid cursor request"),
        (status = 500, description = "Internal error")
    )
)]
/// Fetch one page from a cursor.
pub async fn fetch_cursor<
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
    Json(request): Json<FetchCursorRequest>,
) -> Result<JsonResponse<FetchCursorResponse>, HttpError> {
    let trace_id = uuid::Uuid::new_v4().to_string();
    let start = std::time::Instant::now();
    let stats_manager = state.server.get_stats_manager().clone();
    let page = state
        .server
        .get_graph_service()
        .fetch_cursor(request.session_id, request.cursor_id, request.page_size)
        .await
        .map_err(|message| {
            let elapsed_us = start.elapsed().as_micros() as u64;
            let mut profile = graphdb_metrics::QueryProfile::new(
                request.session_id,
                "(cursor fetch)".to_string(),
            );
            profile.trace_id = trace_id.clone();
            profile.total_duration_us = elapsed_us;
            profile.stages.execute_us = elapsed_us;
            let mut metrics = graphdb_metrics::QueryMetrics::new();
            metrics.execute_time_us = elapsed_us;
            metrics.total_time_us = elapsed_us;
            stats_manager.record_query_metrics(&metrics);
            let error_info = graphdb_metrics::ErrorInfo::new(
                graphdb_metrics::ErrorType::ExecutionError,
                graphdb_metrics::QueryPhase::Execute,
                message.clone(),
            );
            stats_manager.record_failed_query(profile, error_info);
            cursor_error(message)
        })?;
    let rows = page
        .rows
        .into_iter()
        .map(|row| {
            page.columns
                .iter()
                .zip(row.into_iter())
                .map(|(col, value)| (col.clone(), value_to_json(value)))
                .collect()
        })
        .collect::<Vec<_>>();
    let returned = rows.len();
    let elapsed_us = start.elapsed().as_micros() as u64;
    let mut profile =
        graphdb_metrics::QueryProfile::new(request.session_id, "(cursor fetch)".to_string());
    profile.trace_id = trace_id.clone();
    profile.total_duration_us = elapsed_us;
    profile.stages.execute_us = elapsed_us;
    profile.result_count = returned;
    let mut metrics = graphdb_metrics::QueryMetrics::new();
    metrics.execute_time_us = elapsed_us;
    metrics.total_time_us = elapsed_us;
    metrics.result_row_count = returned;
    stats_manager.record_query_metrics(&metrics);
    stats_manager.record_query_profile(profile);
    Ok(JsonResponse(FetchCursorResponse {
        columns: page.columns,
        rows,
        has_more: page.has_more,
        returned,
        trace_id: Some(trace_id),
        stages: Some(graphdb_wire::query::QueryStageTimings {
            parse_ms: 0.0,
            validate_ms: 0.0,
            plan_ms: 0.0,
            optimize_ms: 0.0,
            execute_ms: elapsed_us as f64 / 1000.0,
        }),
    }))
}

#[utoipa::path(
    post,
    path = "/v1/query/cursor/close",
    tag = "Cursor",
    request_body = CloseCursorRequest,
    responses(
        (status = 200, body = CloseCursorResponse, description = "Cursor closed"),
        (status = 400, description = "Invalid cursor request"),
        (status = 500, description = "Internal error")
    )
)]
/// Release a cursor.
pub async fn close_cursor<
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
    Json(request): Json<CloseCursorRequest>,
) -> Result<JsonResponse<CloseCursorResponse>, HttpError> {
    let closed = state
        .server
        .get_graph_service()
        .close_cursor(request.session_id, request.cursor_id)
        .await
        .map_err(cursor_error)?;
    Ok(JsonResponse(CloseCursorResponse { closed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_request_defaults_page_size() {
        let request: FetchCursorRequest =
            serde_json::from_str(r#"{"session_id": 1, "cursor_id": 7}"#)
                .expect("fetch request should parse");
        assert_eq!(request.page_size, 500);
    }

    #[test]
    fn cursor_responses_roundtrip() {
        let open = OpenCursorResponse {
            cursor_id: 9,
            columns: vec!["n".to_string()],
        };
        let json = serde_json::to_string(&open).expect("open response serializes");
        let back: OpenCursorResponse = serde_json::from_str(&json).expect("open response parses");
        assert_eq!(back.cursor_id, 9);
        assert_eq!(back.columns, vec!["n".to_string()]);

        let page = FetchCursorResponse {
            columns: vec!["n".to_string()],
            rows: vec![std::collections::HashMap::from([(
                "n".to_string(),
                serde_json::Value::from(1),
            )])],
            has_more: true,
            returned: 1,
            trace_id: None,
            stages: None,
        };
        let json = serde_json::to_string(&page).expect("fetch response serializes");
        let back: FetchCursorResponse = serde_json::from_str(&json).expect("fetch response parses");
        assert!(back.has_more);
        assert_eq!(back.returned, 1);
    }
}
