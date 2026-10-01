//! HTTP handler for data export operations

use axum::body::{Body, Bytes};
use axum::http::{header, StatusCode};
use axum::{
    extract::{Query, State},
    response::Response,
};
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;

use super::query::result_size_limit;
use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::value::to_json as value_to_json;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExportQuery {
    pub space: Option<String>,
    pub format: Option<String>,
    pub query: Option<String>,
    pub all: Option<String>,
    pub session_id: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/v1/export",
    tag = "Export",
    params(
        ("space" = Option<String>, Query, description = "Space to export (reserved, session space applies)"),
        ("format" = Option<String>, Query, description = "Export format: csv or jsonl"),
        ("query" = Option<String>, Query, description = "Query selecting rows to export"),
        ("all" = Option<String>, Query, description = "Reserved"),
        ("session_id" = Option<i64>, Query, description = "Session id executing the export query")
    ),
    responses(
        (status = 200, description = "Export file download"),
        (status = 400, description = "Invalid export request"),
        (status = 500, description = "Internal error")
    )
)]
/// Re-execute a single query and stream the result out as a download.
///
/// Rows are pulled chunk-at-a-time and encoded straight into the response
/// body, so peak memory stays flat regardless of result size. This is a
/// fresh execution, not a snapshot of what the browser buffered: data may
/// have changed between display and export. Only `csv` and `jsonl` are
/// supported; `jsonl` carries one object per line because a JSON array
/// cannot be appended to without backtracking.
pub async fn export_data<
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
    Query(params): Query<ExportQuery>,
) -> Result<Response, HttpError> {
    let format = params.format.as_deref().unwrap_or("csv");
    let (content_type, file_ext) = match format {
        "csv" => ("text/csv", "csv"),
        "jsonl" => ("application/x-ndjson", "jsonl"),
        other => {
            return Err(HttpError::bad_request(format!(
                "Unsupported export format '{other}'; use csv or jsonl"
            )));
        }
    };
    let query = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| HttpError::bad_request("Export requires a non-empty `query` parameter"))?;
    let session_id = params
        .session_id
        .ok_or_else(|| HttpError::bad_request("Export requires a `session_id` parameter"))?;

    let server = state.server.clone();
    let row_limit = result_size_limit(&state);

    // Set up execution before responding so setup failures surface as HTTP
    // errors instead of a truncated download.
    let stream_result = server
        .get_graph_service()
        .execute_stream(session_id, query)
        .await
        .map_err(|e| {
            if e.contains("Invalid session ID") {
                HttpError::bad_request(e)
            } else {
                HttpError::InternalError(e)
            }
        })?;

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(100);
    let as_csv = format == "csv";
    tokio::task::spawn_blocking(move || {
        let mut columns: Option<Vec<String>> = stream_result.column_names();
        let mut header_sent = false;
        let mut exported_rows: usize = 0;

        // Fail the download when the configured row ceiling is hit: the
        // client discards the partial file and reports the limit.
        let send_failure = |tx: &tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
                            message: String| {
            stream_result.cancel();
            let _ = tx.blocking_send(Err(std::io::Error::other(message)));
        };

        loop {
            match stream_result.next_chunk() {
                Ok(Some(chunk)) => {
                    let names = chunk.col_names();
                    if columns.is_none() {
                        columns = Some(names.clone());
                    }
                    if as_csv && !header_sent {
                        let header = columns
                            .as_ref()
                            .map(|cols| {
                                cols.iter()
                                    .map(|c| csv_escape(c))
                                    .collect::<Vec<_>>()
                                    .join(",")
                            })
                            .unwrap_or_default();
                        if tx
                            .blocking_send(Ok(Bytes::from(format!("{header}\n"))))
                            .is_err()
                        {
                            stream_result.cancel();
                            return;
                        }
                        header_sent = true;
                    }
                    for row in chunk.rows {
                        if let Some(limit) = row_limit {
                            if exported_rows >= limit {
                                send_failure(
                                    &tx,
                                    format!("Result size limit of {limit} rows exceeded"),
                                );
                                return;
                            }
                        }
                        let values: Vec<serde_json::Value> =
                            row.into_iter().map(value_to_json).collect();
                        let line = if as_csv {
                            encode_csv_line(&names, &values)
                        } else {
                            encode_jsonl_line(&names, &values)
                        };
                        exported_rows += 1;
                        if tx.blocking_send(Ok(Bytes::from(line))).is_err() {
                            stream_result.cancel();
                            return;
                        }
                    }
                }
                Ok(None) => {
                    // Empty result still yields a header-only CSV.
                    if as_csv && !header_sent {
                        let header = columns
                            .as_ref()
                            .map(|cols| {
                                cols.iter()
                                    .map(|c| csv_escape(c))
                                    .collect::<Vec<_>>()
                                    .join(",")
                            })
                            .unwrap_or_default();
                        let _ = tx.blocking_send(Ok(Bytes::from(format!("{header}\n"))));
                    }
                    return;
                }
                Err(e) => {
                    send_failure(&tx, format!("Export query failed: {e}"));
                    return;
                }
            }
        }
    });

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"export.{file_ext}\""),
        )
        .body(Body::from_stream(ReceiverStream::new(rx)))
        .map_err(|e| HttpError::InternalError(format!("Failed to build response: {}", e)))?;

    Ok(response)
}

/// Render one CSV line: cells in column order, objects as compact JSON.
fn encode_csv_line(columns: &[String], values: &[serde_json::Value]) -> String {
    let mut cells = Vec::with_capacity(columns.len());
    for (i, _col) in columns.iter().enumerate() {
        let cell = values.get(i).map(cell_text).unwrap_or_default();
        cells.push(csv_escape(&cell));
    }
    cells.join(",") + "\n"
}

/// Render one JSONL line: a column-name to value object.
fn encode_jsonl_line(columns: &[String], values: &[serde_json::Value]) -> String {
    let obj: serde_json::Map<String, serde_json::Value> = columns
        .iter()
        .enumerate()
        .map(|(i, col)| {
            (
                col.clone(),
                values.get(i).cloned().unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    serde_json::to_string(&serde_json::Value::Object(obj)).unwrap_or_else(|_| "{}".to_string())
        + "\n"
}

/// Plain-text rendering of a cell: nulls vanish, objects become JSON.
fn cell_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => value.to_string(),
    }
}

/// Quote a field when it carries a comma, quote, or line break.
fn csv_escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn csv_escapes_delimiters_quotes_and_line_breaks() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_escape("line1\nline2"), "\"line1\nline2\"");
        assert_eq!(csv_escape(""), "");
    }

    #[test]
    fn csv_line_orders_cells_and_stringifies_objects() {
        let columns = vec![
            "name".to_string(),
            "meta".to_string(),
            "missing".to_string(),
        ];
        let values = vec![json!("a,b"), json!({"k": 1})];
        assert_eq!(
            encode_csv_line(&columns, &values),
            "\"a,b\",\"{\"\"k\"\":1}\",\n"
        );
    }

    #[test]
    fn jsonl_line_maps_columns_to_values() {
        let columns = vec!["a".to_string(), "b".to_string()];
        let values = vec![json!(1), json!("x")];
        assert_eq!(
            encode_jsonl_line(&columns, &values),
            "{\"a\":1,\"b\":\"x\"}\n"
        );
    }
}
