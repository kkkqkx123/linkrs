//! Query contract DTOs (HTTP `/query` endpoints).
//!
//! Previously mirrored between `api/server/http/handlers/query_types.rs` and
//! `cli/client/{types,request_types,response_types}.rs`; this is the single
//! source of truth for both sides.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Query request
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Batch query request: multiple auto-commit DML statements executed inside a
/// single shared auto-commit batch window (P4/P6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchQueryRequest {
    pub session_id: i64,
    pub statements: Vec<String>,
}

/// Batch query response: one [`QueryResponse`] per input statement, in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchQueryResponse {
    pub results: Vec<QueryResponse>,
}

/// Query response (structured)
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryData {
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub rows: Vec<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub row_count: usize,
}

/// Query metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryMetadata {
    #[serde(default)]
    pub execution_time_ms: u64,
    #[serde(default)]
    pub rows_scanned: u64,
    #[serde(default)]
    pub rows_returned: usize,
    #[serde(default)]
    pub space_id: Option<u64>,
}

/// Query error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Option<String>,
}

/// Verify the response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateResponse {
    pub valid: bool,
    pub message: String,
}

/// Streaming query request (SSE `/stream` endpoint).
#[derive(Debug, Clone, Deserialize)]
pub struct StreamQueryRequest {
    pub query: String,
    pub session_id: i64,
    #[serde(default = "default_buffer_capacity")]
    pub event_buffer_capacity: usize,
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
    fn request_roundtrip() {
        let request = QueryRequest {
            query: "MATCH (n) RETURN n".to_string(),
            session_id: 7,
            parameters: HashMap::from([("p".to_string(), serde_json::json!(42))]),
            session_variables: HashMap::new(),
        };
        let json = serde_json::to_string(&request).unwrap();
        let back: QueryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.query, request.query);
        assert_eq!(back.session_id, 7);
        assert_eq!(back.parameters.get("p"), Some(&serde_json::json!(42)));
    }
}
