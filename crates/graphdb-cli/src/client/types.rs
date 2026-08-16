//! Display-oriented client result types
//!
//! The wire contract lives in `graphdb-wire`; this module keeps only the
//! flattened display shape used by the output layer, produced from a wire
//! [`QueryResponse`] via [`From`].

use graphdb_wire::query::QueryResponse;

/// Query execution result (flattened wire response for display).
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<std::collections::HashMap<String, serde_json::Value>>,
    pub row_count: usize,
    pub execution_time_ms: u64,
    pub rows_scanned: u64,
    pub error: Option<QueryErrorInfo>,
}

/// Query error information
#[derive(Debug, Clone)]
pub struct QueryErrorInfo {
    pub code: String,
    pub message: String,
    pub details: Option<String>,
}

impl From<QueryResponse> for QueryResult {
    fn from(response: QueryResponse) -> Self {
        let data = response.data.unwrap_or_else(|| graphdb_wire::query::QueryData::empty());
        Self {
            columns: data.columns,
            rows: data.rows,
            row_count: data.row_count,
            execution_time_ms: response.metadata.execution_time_ms,
            rows_scanned: response.metadata.rows_scanned,
            error: response.error.map(|e| QueryErrorInfo {
                code: e.code,
                message: e.message,
                details: e.details,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_wire::query::{QueryData, QueryError, QueryMetadata, QueryResponse};

    fn parse(json: &str) -> serde_json::Result<QueryResponse> {
        serde_json::from_str(json)
    }

    fn to_result(json: &str) -> QueryResult {
        let response = parse(json).expect("envelope should parse");
        QueryResult::from(response)
    }

    #[test]
    fn deserialize_success_envelope() {
        let result = to_result(
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
        );

        assert_eq!(result.columns, vec!["name", "age"]);
        assert_eq!(result.row_count, 1);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.execution_time_ms, 12);
        assert_eq!(result.rows_scanned, 42);
        assert!(result.error.is_none());
    }

    #[test]
    fn deserialize_error_envelope() {
        let result = to_result(
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
        );

        assert!(result.rows.is_empty());
        let error = result.error.expect("error should be present");
        assert_eq!(error.code, "QUERY_ERROR");
        assert_eq!(error.message, "syntax error");
    }

    #[test]
    fn deserialize_use_statement_space_id() {
        let result = to_result(
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
        );

        assert_eq!(result.row_count, 1);
        assert_eq!(
            result.rows[0].get("space_id"),
            Some(&serde_json::Value::from(1))
        );
    }

    #[test]
    fn from_wire_response_direct() {
        let response = QueryResponse::success(
            QueryData::new(
                vec!["n".to_string()],
                vec![std::collections::HashMap::from([(
                    "n".to_string(),
                    serde_json::json!("a"),
                )])],
            ),
            QueryMetadata {
                execution_time_ms: 3,
                rows_scanned: 5,
                rows_returned: 1,
                space_id: None,
            },
        );
        let result = QueryResult::from(response);
        assert_eq!(result.execution_time_ms, 3);
        assert_eq!(result.rows_scanned, 5);
    }

    #[test]
    fn error_response_from_wire() {
        let response = QueryResponse {
            success: false,
            data: None,
            error: Some(QueryError {
                code: "QUERY_ERROR".to_string(),
                message: "boom".to_string(),
                details: None,
            }),
            metadata: QueryMetadata::default(),
        };
        let result = QueryResult::from(response);
        assert!(result.error.is_some());
        assert!(result.rows.is_empty());
    }
}
