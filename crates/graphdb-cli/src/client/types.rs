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
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<HashMap<String, serde_json::Value>>,
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
