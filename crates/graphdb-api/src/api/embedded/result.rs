//! Query Result Processing Module
//!
//! Provides comprehensive query result processing capabilities, extending the core layer of QueryResult and Row

use crate::api::core::{CoreError, CoreResult, QueryResult as CoreQueryResult, Row as CoreRow};
use crate::core::{Edge, Path, Value, Vertex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// Inquiry results
///
/// Encapsulate core layer query results to provide easier access methods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    columns: Vec<String>,
    rows: Vec<Row>,
    metadata: ResultMetadata,
}

/// result line
///
/// Encapsulates row data at the core level, providing access by column name and index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    values: HashMap<String, Value>,
    column_index: HashMap<String, usize>,
}

/// Results metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultMetadata {
    /// execution time
    pub execution_time: Duration,
    /// Returns the number of rows
    pub rows_returned: usize,
    /// scanning line
    pub rows_scanned: u64,
}

impl QueryResult {
    /// Created from core level query results
    pub fn from_core(result: CoreQueryResult) -> Self {
        let columns = result.columns.clone();
        let rows: Vec<Row> = result.rows.into_iter().map(Row::from_core).collect();
        let rows_returned = rows.len();

        Self {
            columns: columns.clone(),
            rows,
            metadata: ResultMetadata {
                execution_time: Duration::from_millis(result.metadata.execution_time_ms),
                rows_returned,
                rows_scanned: result.metadata.rows_scanned,
            },
        }
    }

    /// Get a list of column names
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Get rows
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Check if the result is null
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Get the specified row
    pub fn get(&self, index: usize) -> Option<&Row> {
        self.rows.get(index)
    }

    /// Get the first line
    pub fn first(&self) -> Option<&Row> {
        self.rows.first()
    }

    /// Get last line
    pub fn last(&self) -> Option<&Row> {
        self.rows.last()
    }

    /// Get row iterator
    pub fn iter(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter()
    }

    /// Getting Metadata
    pub fn metadata(&self) -> &ResultMetadata {
        &self.metadata
    }

    /// Get all rows
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Convert to JSON string
    pub fn to_json(&self) -> CoreResult<String> {
        serde_json::to_string_pretty(self)
            .map_err(|e| CoreError::Internal(format!("JSON serialization failed: {}", e)))
    }

    /// Convert to JSON string (compact format)
    pub fn to_json_compact(&self) -> CoreResult<String> {
        serde_json::to_string(self)
            .map_err(|e| CoreError::Internal(format!("JSON serialization failed: {}", e)))
    }

    /// Convert to JSON Value
    pub fn to_json_value(&self) -> CoreResult<serde_json::Value> {
        serde_json::to_value(self)
            .map_err(|e| CoreError::Internal(format!("JSON serialization failed: {}", e)))
    }
}

impl IntoIterator for QueryResult {
    type Item = Row;
    type IntoIter = std::vec::IntoIter<Row>;

    fn into_iter(self) -> Self::IntoIter {
        self.rows.into_iter()
    }
}

impl<'a> IntoIterator for &'a QueryResult {
    type Item = &'a Row;
    type IntoIter = std::slice::Iter<'a, Row>;

    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter()
    }
}

impl Row {
    /// Created from core layer row data
    pub fn from_core(row: CoreRow) -> Self {
        let mut column_index = HashMap::new();
        let values = row.values;

        for (idx, (key, _)) in values.iter().enumerate() {
            column_index.insert(key.clone(), idx);
        }

        Self {
            values,
            column_index,
        }
    }

    /// Getting values by column name
    pub fn get(&self, column: &str) -> Option<&Value> {
        self.values.get(column)
    }

    /// Getting values by index
    pub fn get_by_index(&self, index: usize) -> Option<&Value> {
        self.columns()
            .get(index)
            .and_then(|col| self.values.get(col.as_str()))
    }

    /// Get all column names
    pub fn columns(&self) -> Vec<&String> {
        self.values.keys().collect()
    }

    /// Get the number of columns
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Check for blank lines
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Checks if the specified column is included
    pub fn has_column(&self, column: &str) -> bool {
        self.values.contains_key(column)
    }

    // Typed Acquisition Methods

    /// Getting String Values
    pub fn get_string(&self, column: &str) -> Option<String> {
        self.get(column).and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
    }

    /// Get i64 integer value
    pub fn get_int(&self, column: &str) -> Option<i64> {
        self.get(column).and_then(|v| match v {
            Value::Int(i) => Some(*i as i64),
            _ => None,
        })
    }

    /// Get f64 floating point value
    pub fn get_float(&self, column: &str) -> Option<f64> {
        self.get(column).and_then(|v| match v {
            Value::Float(f) => Some(*f as f64),
            _ => None,
        })
    }

    /// Get Boolean
    pub fn get_bool(&self, column: &str) -> Option<bool> {
        self.get(column).and_then(|v| match v {
            Value::Bool(b) => Some(*b),
            _ => None,
        })
    }

    /// Get Vertex
    pub fn get_vertex(&self, column: &str) -> Option<&Vertex> {
        self.get(column).and_then(|v| match v {
            Value::Vertex(vertex) => Some(vertex.as_ref()),
            _ => None,
        })
    }

    /// Getting the edge
    pub fn get_edge(&self, column: &str) -> Option<&Edge> {
        self.get(column).and_then(|v| match v {
            Value::Edge(edge) => Some(edge.as_ref()),
            _ => None,
        })
    }

    /// Get Path
    pub fn get_path(&self, column: &str) -> Option<&Path> {
        self.get(column).and_then(|v| match v {
            Value::Path(path) => Some(path.as_ref()),
            _ => None,
        })
    }

    /// Get List
    pub fn get_list(&self, column: &str) -> Option<&crate::core::value::list::List> {
        self.get(column).and_then(|v| match v {
            Value::List(list) => Some(list.as_ref()),
            _ => None,
        })
    }

    /// Getting the mapping
    pub fn get_map(&self, column: &str) -> Option<&HashMap<String, Value>> {
        self.get(column).and_then(|v| match v {
            Value::Map(map) => Some(map.as_ref()),
            _ => None,
        })
    }

    /// Get all values
    pub fn values(&self) -> &HashMap<String, Value> {
        &self.values
    }

    /// Translate from "Convert to JSON string" to English: "Convert to JSON string"
    pub fn to_json(&self) -> CoreResult<String> {
        serde_json::to_string_pretty(self)
            .map_err(|e| CoreError::Internal(format!("JSON serialization failed: {}", e)))
    }
}

impl Default for ResultMetadata {
    fn default() -> Self {
        Self {
            execution_time: Duration::from_millis(0),
            rows_returned: 0,
            rows_scanned: 0,
        }
    }
}

/// Streaming Search Results
///
/// Used to process large datasets and avoid loading all data into memory at once
pub struct StreamingQueryResult {
    columns: Vec<String>,
    metadata: ResultMetadata,
}

impl StreamingQueryResult {
    /// Create streaming query results
    pub fn new(columns: Vec<String>, metadata: ResultMetadata) -> Self {
        Self { columns, metadata }
    }

    /// Get the list of column names
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Get metadata
    pub fn metadata(&self) -> &ResultMetadata {
        &self.metadata
    }

    /// Get the number of columns
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }
}
