use std::collections::HashMap;
use std::sync::Arc;

use crate::graph_service::GraphService;
use crate::value::to_json as value_to_json;
use axum::{
    extract::{Extension, Json, State},
    response::{sse::Event, Sse},
};
use linkrs_wire::query::StreamQueryRequest;
use serde::Serialize;
use serde_json::json;
use tokio::sync::mpsc::Sender;
use tokio_stream::wrappers::ReceiverStream;

use super::query::{json_params_to_core, result_size_limit};
use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

/// Streaming results data items
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct StreamDataItem {
    pub row: serde_json::Value,
    pub index: usize,
    pub stmt: usize,
}

/// Streaming results metadata
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct StreamMetadata {
    pub rows_returned: usize,
    pub execution_time_ms: u64,
    pub columns: Vec<String>,
    pub stmt: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stages: Option<linkrs_wire::query::QueryStageTimings>,
}

/// Batch-streaming boundary: a statement starts executing.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct StatementBegin {
    pub index: usize,
    pub query: String,
}

/// Batch-streaming boundary: a statement finished (successfully or not).
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct StatementEnd {
    pub index: usize,
    pub success: bool,
    pub rows_returned: usize,
    pub execution_time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

/// Failure detail carried by a statement outcome.
struct StreamFailure {
    code: String,
    message: String,
}

/// Outcome of streaming one statement's body (schema / rows / metadata or
/// error events are already on the channel when this returns).
struct StatementOutcome {
    rows_returned: usize,
    execution_time_ms: u64,
    /// The client disconnected mid-statement; the caller stops the loop.
    disconnected: bool,
    failure: Option<StreamFailure>,
    trace_id: String,
}

type EventSender = Sender<Result<Event, HttpError>>;

#[utoipa::path(
    post,
    path = "/v1/query/stream",
    tag = "Stream",
    request_body = StreamQueryRequest,
    responses(
        (status = 200, description = "Query result event stream", content_type = "text/event-stream"),
        (status = 500, description = "Internal error")
    )
)]
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
    Extension(caller_session_id): Extension<i64>,
    Json(request): Json<StreamQueryRequest>,
) -> Result<
    Sse<impl tokio_stream::Stream<Item = Result<Event, HttpError>> + Send + 'static>,
    HttpError,
> {
    super::authz::require_session_owner_or_admin(&state, caller_session_id, request.session_id)?;
    let batch_mode = !request.statements.is_empty();
    if batch_mode && !request.query.trim().is_empty() {
        return Err(HttpError::bad_request(
            "Stream request must use either `query` or `statements`, not both",
        ));
    }
    let buffer_capacity = request.event_buffer_capacity.clamp(1, 1000);
    let server = state.server.clone();
    let row_limit = result_size_limit(&state);

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, HttpError>>(buffer_capacity);

    tokio::spawn(async move {
        let graph_service = server.get_graph_service();
        let stats_manager = server.get_stats_manager().clone();
        if batch_mode {
            let parameters = json_params_to_core(&request.parameters);
            let session_variables = json_params_to_core(&request.session_variables);
            run_batch_stream(
                &tx,
                &graph_service,
                &stats_manager,
                request.session_id,
                &request.statements,
                parameters,
                session_variables,
                request.fail_fast,
                row_limit,
            )
            .await;
        } else {
            let trace_id = uuid::Uuid::new_v4().to_string();
            let outcome = stream_statement_body(
                &tx,
                &graph_service,
                &stats_manager,
                request.session_id,
                &request.query,
                None,
                row_limit,
                0,
                trace_id,
            )
            .await;
            if !outcome.disconnected {
                send_event(&tx, "done", "{}").await;
            }
        }
    });

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(10))
            .text("keepalive"),
    ))
}

/// Parameter bindings carried through one streamed statement:
/// `(query parameters, session variables)`.
type StatementBindings = (
    Option<HashMap<String, linkrs_core::Value>>,
    Option<HashMap<String, linkrs_core::Value>>,
);

#[allow(
    clippy::too_many_arguments,
    reason = "streaming handler context; grouping would obscure call sites"
)]
/// Batch-streaming driver: statements run sequentially in request order,
/// each framed by `statement_begin` / `statement_end`. Every statement
/// shares the batch request's bindings. A client disconnect cancels the
/// running statement and skips the rest; produced statements are kept.
async fn run_batch_stream<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    tx: &EventSender,
    graph_service: &Arc<GraphService<S>>,
    stats_manager: &Arc<linkrs_metrics::StatsManager>,
    session_id: i64,
    statements: &[String],
    parameters: Option<HashMap<String, linkrs_core::Value>>,
    session_variables: Option<HashMap<String, linkrs_core::Value>>,
    fail_fast: bool,
    row_limit: Option<usize>,
) {
    for (index, statement) in statements.iter().enumerate() {
        let begin = StatementBegin {
            index,
            query: statement.clone(),
        };
        if !send_json(tx, "statement_begin", &begin).await {
            return;
        }
        let bindings = Some((parameters.clone(), session_variables.clone()));
        let trace_id = uuid::Uuid::new_v4().to_string();
        let outcome = stream_statement_body(
            tx,
            graph_service,
            stats_manager,
            session_id,
            statement,
            bindings,
            row_limit,
            index,
            trace_id,
        )
        .await;
        if outcome.disconnected {
            return;
        }
        let end = StatementEnd {
            index,
            success: outcome.failure.is_none(),
            rows_returned: outcome.rows_returned,
            execution_time_ms: outcome.execution_time_ms,
            code: outcome.failure.as_ref().map(|f| f.code.clone()),
            message: outcome.failure.as_ref().map(|f| f.message.clone()),
            trace_id: Some(outcome.trace_id.clone()),
        };
        if !send_json(tx, "statement_end", &end).await {
            return;
        }
        if outcome.failure.is_some() && fail_fast {
            break;
        }
    }
    send_event(tx, "done", "{}").await;
}

/// Stream one statement's schema / rows / metadata (or a single error
/// event on failure). The terminal `done` marker and, in batch mode, the
/// boundary events are the caller's responsibility.
#[allow(clippy::too_many_arguments)]
async fn stream_statement_body<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    tx: &EventSender,
    graph_service: &Arc<GraphService<S>>,
    stats_manager: &Arc<linkrs_metrics::StatsManager>,
    session_id: i64,
    stmt: &str,
    bindings: Option<StatementBindings>,
    row_limit: Option<usize>,
    stmt_index: usize,
    trace_id: String,
) -> StatementOutcome {
    let start_time = std::time::Instant::now();
    let stmt_text = stmt.to_string();
    let failed = |code: &str, message: String| StatementOutcome {
        rows_returned: 0,
        execution_time_ms: start_time.elapsed().as_millis() as u64,
        disconnected: false,
        failure: Some(StreamFailure {
            code: code.to_string(),
            message,
        }),
        trace_id: trace_id.clone(),
    };

    // Get a streaming result handle (chunk-at-a-time).
    let stream_result = match bindings {
        Some((parameters, session_variables)) => {
            graph_service
                .execute_stream_with_params(session_id, stmt, parameters, session_variables)
                .await
        }
        None => graph_service.execute_stream(session_id, stmt).await,
    };
    let stream_result = match stream_result {
        Ok(stream) => stream,
        Err(e) => {
            let error_msg = json!({
                "error": true,
                "message": e.message(),
                "code": "QUERY_ERROR",
                "stmt": stmt_index
            });
            send_event(tx, "error", &error_msg.to_string()).await;
            let outcome = failed("QUERY_ERROR", e.message().to_string());
            record_stream_profile(
                stats_manager,
                session_id,
                &stmt_text,
                &trace_id,
                start_time.elapsed().as_micros() as u64,
                0,
                Some(e.message().to_string()),
            );
            return outcome;
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
            "stmt": stmt_index,
        });
        if let Ok(schema_str) = serde_json::to_string(&schema) {
            send_event(tx, "schema", &schema_str).await;
        }
    }

    // Spawn a blocking task to pull chunks synchronously
    // and send rows through the channel.
    let tx_pull = tx.clone();
    let stream_result_pull = stream_result.clone();
    let schema_sent_pull = schema_sent.clone();
    let pull_stmt = stmt_index;
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
                            "stmt": pull_stmt,
                        });
                        if let Ok(schema_str) = serde_json::to_string(&schema) {
                            if tx_pull
                                .blocking_send(Ok(Event::default()
                                    .event("schema")
                                    .data(schema_str)))
                                .is_err()
                            {
                                stream_result_pull.cancel();
                                return PullOutcome::Disconnected(row_index);
                            }
                        }
                    }

                    for row in chunk.rows {
                        if let Some(limit) = row_limit {
                            if row_index >= limit {
                                stream_result_pull.cancel();
                                return PullOutcome::LimitReached(row_index, limit);
                            }
                        }
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
                            stmt: pull_stmt,
                        };
                        row_index += 1;

                        if let Ok(data) = serde_json::to_string(&item) {
                            if tx_pull
                                .blocking_send(Ok(Event::default().data(data)))
                                .is_err()
                            {
                                // Client disconnected — cancel the query.
                                stream_result_pull.cancel();
                                return PullOutcome::Disconnected(row_index);
                            }
                        }
                    }
                }
                Ok(None) => return PullOutcome::Completed(row_index),
                Err(e) => {
                    return PullOutcome::Failed(
                        row_index,
                        StreamFailure {
                            code: "QUERY_ERROR".to_string(),
                            message: e.to_string(),
                        },
                    )
                }
            }
        }
    });

    // Wait for the pull task to finish.
    match pull_handle.await {
        Ok(PullOutcome::Completed(total_rows)) => {
            let elapsed_us = start_time.elapsed().as_micros() as u64;
            // Send metadata summary AFTER all rows.
            let metadata = StreamMetadata {
                rows_returned: total_rows,
                execution_time_ms: (elapsed_us / 1000),
                columns: Vec::new(), // schema was sent upfront
                stmt: stmt_index,
                trace_id: Some(trace_id.clone()),
                stages: Some(linkrs_wire::query::QueryStageTimings {
                    parse_ms: 0.0,
                    validate_ms: 0.0,
                    plan_ms: 0.0,
                    optimize_ms: 0.0,
                    execute_ms: elapsed_us as f64 / 1000.0,
                }),
            };

            if let Ok(meta_str) = serde_json::to_string(&metadata) {
                send_event(tx, "metadata", &meta_str).await;
            }

            record_stream_profile(
                stats_manager,
                session_id,
                &stmt_text,
                &trace_id,
                elapsed_us,
                total_rows,
                None,
            );
            StatementOutcome {
                rows_returned: total_rows,
                execution_time_ms: (elapsed_us / 1000),
                disconnected: false,
                failure: None,
                trace_id: trace_id.clone(),
            }
        }
        Ok(PullOutcome::Failed(total_rows, failure)) => {
            // Execution error — send error (the caller frames it).
            let error_msg = json!({
                "error": true,
                "message": failure.message,
                "code": failure.code,
                "stmt": stmt_index,
            });
            send_event(tx, "error", &error_msg.to_string()).await;
            let elapsed_us = start_time.elapsed().as_micros() as u64;
            let message = failure.message.clone();
            record_stream_profile(
                stats_manager,
                session_id,
                &stmt_text,
                &trace_id,
                elapsed_us,
                total_rows,
                Some(message),
            );
            StatementOutcome {
                rows_returned: total_rows,
                execution_time_ms: start_time.elapsed().as_millis() as u64,
                disconnected: false,
                failure: Some(failure),
                trace_id: trace_id.clone(),
            }
        }
        Ok(PullOutcome::LimitReached(total_rows, limit)) => {
            // Row ceiling hit — send a truncation error (the caller frames
            // it); rows already delivered are kept.
            let failure = StreamFailure {
                code: "ROW_LIMIT_EXCEEDED".to_string(),
                message: format!(
                    "Result size limit of {limit} rows reached; narrow the query with LIMIT or use server export"
                ),
            };
            let error_msg = json!({
                "error": true,
                "message": failure.message,
                "code": failure.code,
                "stmt": stmt_index,
            });
            send_event(tx, "error", &error_msg.to_string()).await;
            let elapsed_us = start_time.elapsed().as_micros() as u64;
            let message = failure.message.clone();
            record_stream_profile(
                stats_manager,
                session_id,
                &stmt_text,
                &trace_id,
                elapsed_us,
                total_rows,
                Some(message),
            );
            StatementOutcome {
                rows_returned: total_rows,
                execution_time_ms: start_time.elapsed().as_millis() as u64,
                disconnected: false,
                failure: Some(failure),
                trace_id: trace_id.clone(),
            }
        }
        Ok(PullOutcome::Disconnected(total_rows)) => StatementOutcome {
            rows_returned: total_rows,
            execution_time_ms: start_time.elapsed().as_millis() as u64,
            disconnected: true,
            failure: None,
            trace_id: trace_id.clone(),
        },
        Err(_) => {
            // Task panicked or cancelled — channel will be dropped.
            StatementOutcome {
                rows_returned: 0,
                execution_time_ms: start_time.elapsed().as_millis() as u64,
                disconnected: true,
                failure: None,
                trace_id: trace_id.clone(),
            }
        }
    }
}

/// Chunk-pull loop outcome from the blocking task.
enum PullOutcome {
    Completed(usize),
    Failed(usize, StreamFailure),
    LimitReached(usize, usize),
    Disconnected(usize),
}

/// Send a raw string payload; returns false when the client is gone.
async fn send_event(tx: &EventSender, event: &str, data: &str) -> bool {
    tx.send(Ok(Event::default().event(event).data(data)))
        .await
        .is_ok()
}

/// Serialize a payload and send it; returns false when the client is gone.
async fn send_json<T: Serialize>(tx: &EventSender, event: &str, payload: &T) -> bool {
    match serde_json::to_string(payload) {
        Ok(data) => send_event(tx, event, &data).await,
        Err(_) => false,
    }
}

fn record_stream_profile(
    stats_manager: &Arc<linkrs_metrics::StatsManager>,
    session_id: i64,
    query_text: &str,
    trace_id: &str,
    elapsed_us: u64,
    rows: usize,
    error: Option<String>,
) {
    if let Some(message) = error {
        let mut profile = linkrs_metrics::QueryProfile::new(session_id, query_text.to_string());
        profile.trace_id = trace_id.to_string();
        profile.total_duration_us = elapsed_us;
        profile.stages.execute_us = elapsed_us;
        profile.result_count = rows;
        let mut metrics = linkrs_metrics::QueryMetrics::new();
        metrics.execute_time_us = elapsed_us;
        metrics.total_time_us = elapsed_us;
        metrics.result_row_count = rows;
        stats_manager.record_query_metrics(&metrics);
        let error_info = linkrs_metrics::ErrorInfo::new(
            linkrs_metrics::ErrorType::ExecutionError,
            linkrs_metrics::QueryPhase::Execute,
            message,
        );
        stats_manager.record_failed_query(profile, error_info);
    } else {
        let mut profile = linkrs_metrics::QueryProfile::new(session_id, query_text.to_string());
        profile.trace_id = trace_id.to_string();
        profile.total_duration_us = elapsed_us;
        profile.stages.execute_us = elapsed_us;
        profile.result_count = rows;
        let mut metrics = linkrs_metrics::QueryMetrics::new();
        metrics.execute_time_us = elapsed_us;
        metrics.total_time_us = elapsed_us;
        metrics.result_row_count = rows;
        stats_manager.record_query_metrics(&metrics);
        stats_manager.record_query_profile(profile);
    }
}
