//! Core data types for client operations

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Space information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceInfo {
    pub id: u64,
    pub name: String,
    pub vid_type: String,
    #[serde(default)]
    pub comment: Option<String>,
}

/// Tag information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagInfo {
    pub name: String,
    pub fields: Vec<FieldInfo>,
}

/// Edge type information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeTypeInfo {
    pub name: String,
    pub fields: Vec<FieldInfo>,
}

/// Field information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    #[serde(default)]
    pub default_value: Option<String>,
}

/// Query execution result
///
/// Deserializes directly from the server's query response envelope. The
/// envelope groups the payload into `data`, `metadata` and `error` subobjects;
/// the wire groups are collapsed into these flat fields.
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<HashMap<String, serde_json::Value>>,
    pub row_count: usize,
    pub execution_time_ms: u64,
    pub rows_scanned: u64,
    pub error: Option<QueryErrorInfo>,
}

impl<'de> Deserialize<'de> for QueryResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            data: Option<WireData>,
            #[serde(default)]
            metadata: Option<WireMetadata>,
            #[serde(default)]
            error: Option<QueryErrorInfo>,
        }

        #[derive(Deserialize)]
        struct WireData {
            #[serde(default)]
            columns: Vec<String>,
            #[serde(default)]
            rows: Vec<HashMap<String, serde_json::Value>>,
            #[serde(default)]
            row_count: usize,
        }

        #[derive(Deserialize)]
        struct WireMetadata {
            #[serde(default)]
            execution_time_ms: u64,
            #[serde(default)]
            rows_scanned: u64,
        }

        let wire = Wire::deserialize(deserializer)?;
        let data = wire.data.unwrap_or(WireData {
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
        });
        let metadata = wire.metadata.unwrap_or(WireMetadata {
            execution_time_ms: 0,
            rows_scanned: 0,
        });
        Ok(Self {
            columns: data.columns,
            rows: data.rows,
            row_count: data.row_count,
            execution_time_ms: metadata.execution_time_ms,
            rows_scanned: metadata.rows_scanned,
            error: wire.error,
        })
    }
}

/// Query error information
#[derive(Debug, Clone, Deserialize)]
pub struct QueryErrorInfo {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> serde_json::Result<QueryResult> {
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

        assert_eq!(result.columns, vec!["name", "age"]);
        assert_eq!(result.row_count, 1);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.execution_time_ms, 12);
        assert_eq!(result.rows_scanned, 42);
        assert!(result.error.is_none());
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

        assert!(result.rows.is_empty());
        let error = result.error.expect("error should be present");
        assert_eq!(error.code, "QUERY_ERROR");
        assert_eq!(error.message, "syntax error");
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

        assert_eq!(result.row_count, 1);
        assert_eq!(
            result.rows[0].get("space_id"),
            Some(&serde_json::Value::from(1))
        );
    }
}
