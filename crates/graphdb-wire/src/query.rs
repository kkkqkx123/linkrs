//! Query contract DTOs (HTTP `/query` endpoints).
//!
//! Previously mirrored between `api/server/http/handlers/query_types.rs` and
//! `cli/client/{types,request_types,response_types}.rs`; this is the single
//! source of truth for both sides.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Query request
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryRequest {
    pub query: String,
    pub session_id: i64,
    /// Query parameters bound to `@name` references in the statement.
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
    /// Session variables bound to `$name` references in the statement.
    /// When omitted, the session-managed snapshot (set via `LET $name = expr`)
    /// is used.
    #[serde(default)]
    pub session_variables: HashMap<String, serde_json::Value>,
    /// Consistency level: None = eventual (default), Some("read_your_writes") enables RYW.
    #[serde(default)]
    pub consistency: Option<String>,
    #[serde(default)]
    pub consistency_timeout_ms: Option<u64>,
    #[serde(default)]
    pub minimum_lsn: Option<u64>,
}

/// Batch query request: multiple auto-commit DML statements executed inside a
/// single shared auto-commit batch window.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BatchQueryRequest {
    pub session_id: i64,
    pub statements: Vec<String>,
    /// Query parameters bound to `@name` references in every statement.
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
    /// Session variables bound to `$name` references in every statement.
    /// When omitted, the session-managed snapshot is used.
    #[serde(default)]
    pub session_variables: HashMap<String, serde_json::Value>,
}

/// Batch query response: one [`QueryResponse`] per input statement, in order.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BatchQueryResponse {
    pub results: Vec<QueryResponse>,
}

/// Query response (structured)
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryResponse {
    pub success: bool,
    #[serde(default)]
    pub data: Option<QueryData>,
    #[serde(default)]
    pub error: Option<QueryError>,
    #[serde(default)]
    pub metadata: QueryMetadata,
}

/// Query data
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryData {
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub rows: Vec<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub row_count: usize,
}

/// Query metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct QueryMetadata {
    #[serde(default)]
    pub execution_time_ms: u64,
    #[serde(default)]
    pub rows_scanned: u64,
    #[serde(default)]
    pub rows_returned: usize,
    #[serde(default)]
    pub space_id: Option<u64>,
    /// The result was cut at the configured row ceiling; `rows_returned`
    /// holds the rows actually delivered.
    #[serde(default)]
    pub truncated: bool,
}

/// Query error.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Option<String>,
}

/// Verify the response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ValidateRequest {
    pub query: String,
    pub session_id: i64,
    #[serde(default)]
    pub need_estimate: bool,
}

/// Verify the response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ValidateResponse {
    pub valid: bool,
    pub message: String,
    /// Planner root-operator row estimate for automatic routing.
    /// Absent when the statement is not stream-shaped or the plan
    /// carries no estimate; callers treat a missing value as "stream".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_rows: Option<u64>,
}

/// Streaming query request (SSE `/stream` endpoint).
///
/// Single-statement mode uses `query`. Batch-streaming mode uses
/// `statements` (with shared `parameters` / `session_variables`, mirroring
/// [`BatchQueryRequest`]) and emits one `statement_begin` /
/// `statement_end` event pair per statement. The two modes are mutually
/// exclusive: a non-empty `query` together with non-empty `statements` is
/// rejected by the server.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct StreamQueryRequest {
    pub query: String,
    pub session_id: i64,
    #[serde(default = "default_buffer_capacity")]
    pub event_buffer_capacity: usize,
    /// Batch-streaming statements. Empty means single-statement mode.
    #[serde(default)]
    pub statements: Vec<String>,
    /// Query parameters bound to `@name` references in every statement
    /// (batch-streaming mode only).
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
    /// Session variables bound to `$name` references in every statement
    /// (batch-streaming mode only). When omitted, the session-managed
    /// snapshot is used, same as [`BatchQueryRequest`].
    #[serde(default)]
    pub session_variables: HashMap<String, serde_json::Value>,
    /// Stop the batch at the first failing statement (default true).
    /// Already produced statements are kept either way.
    #[serde(default = "default_fail_fast")]
    pub fail_fast: bool,
}

fn default_fail_fast() -> bool {
    true
}

/// Open a forward-only cursor over a single statement's result.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OpenCursorRequest {
    pub session_id: i64,
    pub query: String,
}

/// Open cursor response: server-assigned id plus upfront column names.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OpenCursorResponse {
    pub cursor_id: u64,
    #[serde(default)]
    pub columns: Vec<String>,
}

/// Fetch one page from a cursor.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FetchCursorRequest {
    pub session_id: i64,
    pub cursor_id: u64,
    #[serde(default = "default_cursor_page_size")]
    pub page_size: usize,
}

/// One fetched page: rows in column order plus exhaustion flag.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FetchCursorResponse {
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub rows: Vec<HashMap<String, serde_json::Value>>,
    pub has_more: bool,
    pub returned: usize,
}

/// Release a cursor.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CloseCursorRequest {
    pub session_id: i64,
    pub cursor_id: u64,
}

/// Cursor release outcome: false when the id was already gone.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CloseCursorResponse {
    pub closed: bool,
}

fn default_cursor_page_size() -> usize {
    500
}

fn default_buffer_capacity() -> usize {
    100
}

impl QueryResponse {
    /// A successful response has been created.
    pub fn success(data: QueryData, metadata: QueryMetadata) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            metadata,
        }
    }

    /// Creating an error response
    pub fn error(code: String, message: String, details: Option<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(QueryError {
                code,
                message,
                details,
            }),
            metadata: QueryMetadata::default(),
        }
    }
}

impl QueryData {
    /// Create empty query data.
    pub fn empty() -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
        }
    }

    /// Create query data from columns and rows.
    pub fn new(columns: Vec<String>, rows: Vec<HashMap<String, serde_json::Value>>) -> Self {
        let row_count = rows.len();
        Self {
            columns,
            rows,
            row_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> serde_json::Result<QueryResponse> {
        serde_json::from_str(json)
    }

    #[test]
    fn deserialize_success_envelope() {
        let result = parse(
            r#"{
                "success": true,
                "data": {
                    "columns": ["name", "age"],
                    "rows": [{"name": "Alice", "age": 30}],
                    "row_count": 1
                },
                "error": null,
                "metadata": {
                    "execution_time_ms": 12,
                    "rows_scanned": 42,
                    "rows_returned": 1,
                    "space_id": null
                }
            }"#,
        )
        .expect("success envelope should parse");

        let data = result.data.expect("data should be present");
        assert_eq!(data.columns, vec!["name", "age"]);
        assert_eq!(data.row_count, 1);
        assert_eq!(data.rows.len(), 1);
        assert_eq!(result.metadata.execution_time_ms, 12);
        assert_eq!(result.metadata.rows_scanned, 42);
        assert!(result.error.is_none());
        assert!(result.success);
    }

    #[test]
    fn deserialize_error_envelope() {
        let result = parse(
            r#"{
                "success": false,
                "data": null,
                "error": {
                    "code": "QUERY_ERROR",
                    "message": "syntax error",
                    "details": null
                },
                "metadata": {
                    "execution_time_ms": 0,
                    "rows_scanned": 0,
                    "rows_returned": 0,
                    "space_id": null
                }
            }"#,
        )
        .expect("error envelope should parse");

        assert!(result.data.is_none());
        let error = result.error.expect("error should be present");
        assert_eq!(error.code, "QUERY_ERROR");
        assert_eq!(error.message, "syntax error");
        assert!(!result.success);
    }

    #[test]
    fn deserialize_use_statement_space_id() {
        // USE results carry a space_id in the metadata group.
        let result = parse(
            r#"{
                "success": true,
                "data": {
                    "columns": ["space_name", "space_id", "vid_type"],
                    "rows": [
                        {
                            "space_name": "test_space",
                            "space_id": 1,
                            "vid_type": "INT64"
                        }
                    ],
                    "row_count": 1
                },
                "error": null,
                "metadata": {
                    "execution_time_ms": 1,
                    "rows_scanned": 0,
                    "rows_returned": 1,
                    "space_id": 1
                }
            }"#,
        )
        .expect("USE envelope should parse");

        let data = result.data.expect("data should be present");
        assert_eq!(data.row_count, 1);
        assert_eq!(
            data.rows[0].get("space_id"),
            Some(&serde_json::Value::from(1))
        );
        assert_eq!(result.metadata.space_id, Some(1));
    }

    #[test]
    fn stream_batch_request_defaults() {
        let request: StreamQueryRequest = serde_json::from_str(r#"{"query": "", "session_id": 3}"#)
            .expect("stream request should parse");
        assert!(request.statements.is_empty());
        assert!(request.parameters.is_empty());
        assert!(request.session_variables.is_empty());
        assert!(request.fail_fast);

        let batch: StreamQueryRequest = serde_json::from_str(
            r#"{
                "query": "",
                "session_id": 3,
                "statements": ["RETURN 1", "RETURN 2"],
                "parameters": {"p": 1},
                "fail_fast": false
            }"#,
        )
        .expect("batch stream request should parse");
        assert_eq!(batch.statements.len(), 2);
        assert_eq!(batch.parameters.get("p"), Some(&serde_json::Value::from(1)));
        assert!(!batch.fail_fast);
    }

    #[test]
    fn validate_request_estimate_defaults_off() {
        let request: ValidateRequest =
            serde_json::from_str(r#"{"query": "RETURN 1", "session_id": 3}"#)
                .expect("validate request should parse");
        assert!(!request.need_estimate);

        let with_estimate: ValidateRequest = serde_json::from_str(
            r#"{"query": "RETURN 1", "session_id": 3, "need_estimate": true}"#,
        )
        .expect("validate request with estimate should parse");
        assert!(with_estimate.need_estimate);
    }

    #[test]
    fn request_roundtrip() {
        let request = QueryRequest {
            query: "MATCH (n) RETURN n".to_string(),
            session_id: 7,
            parameters: HashMap::from([("p".to_string(), serde_json::json!(42))]),
            session_variables: HashMap::new(),
            consistency: None,
            consistency_timeout_ms: None,
            minimum_lsn: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        let back: QueryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.query, request.query);
        assert_eq!(back.session_id, 7);
        assert_eq!(back.parameters.get("p"), Some(&serde_json::json!(42)));
    }
}
