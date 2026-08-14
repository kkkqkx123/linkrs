//! Definition of query result type

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Query request
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
pub struct BatchQueryRequest {
    pub session_id: i64,
    pub statements: Vec<String>,
}

/// Batch query response: one [`QueryResponse`] per input statement, in order.
#[derive(Debug, Serialize)]
pub struct BatchQueryResponse {
    pub results: Vec<QueryResponse>,
}

/// Query response (structured)
#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub success: bool,
    pub data: Option<QueryData>,
    pub error: Option<QueryError>,
    pub metadata: QueryMetadata,
}

/// Query data
#[derive(Debug, Serialize)]
pub struct QueryData {
    pub columns: Vec<String>,
    pub rows: Vec<HashMap<String, serde_json::Value>>,
    pub row_count: usize,
}

/// Query metadata
#[derive(Debug, Serialize)]
pub struct QueryMetadata {
    pub execution_time_ms: u64,
    pub rows_scanned: u64,
    pub rows_returned: usize,
    pub space_id: Option<u64>,
}

/// Query error.
#[derive(Debug, Serialize)]
pub struct QueryError {
    pub code: String,
    pub message: String,
    pub details: Option<String>,
}

/// Verify the response.
#[derive(Debug, Serialize)]
pub struct ValidateResponse {
    pub valid: bool,
    pub message: String,
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
            metadata: QueryMetadata {
                execution_time_ms: 0,
                rows_scanned: 0,
                rows_returned: 0,
                space_id: None,
            },
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
