//! Query Result Processing Module
//!
//! Provides comprehensive query result processing capabilities, extending the core layer of QueryResult and Row

use crate::api_core::{CoreError, CoreResult, QueryResult as CoreQueryResult};
use linkrs_core::{Edge, Path, Value, Vertex};
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
/// Encapsulates row data at the core level, providing access by column
/// name and index. Column order is preserved; repeated column names keep
/// their last occurrence for name-based lookups.
#[derive(Debug, Clone)]
pub struct Row {
    columns: Vec<String>,
    values: Vec<Value>,
    column_index: HashMap<String, usize>,
}

impl Serialize for Row {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.columns.len()))?;
        for (column, value) in self.columns.iter().zip(self.values.iter()) {
            map.serialize_entry(column, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Row {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RowVisitor;

        impl<'de> serde::de::Visitor<'de> for RowVisitor {
            type Value = Row;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map of column names to values")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut row = Row {
                    columns: Vec::new(),
                    values: Vec::new(),
                    column_index: HashMap::new(),
                };
                while let Some((column, value)) = map.next_entry::<String, Value>()? {
                    row.columns.push(column.clone());
                    row.values.push(value);
                    row.column_index.insert(column, row.columns.len() - 1);
                }
                Ok(row)
            }
        }

        deserializer.deserialize_map(RowVisitor)
    }
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
        let columns = result.columns().to_vec();
        let rows: Vec<Row> = result
            .rows()
            .iter()
            .map(|values| Row::from_columns(&columns, values))
            .collect();
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
    /// Created from core layer row data (column names + positional values).
    pub fn from_columns(columns: &[String], values: &[Value]) -> Self {
        let mut row = Self {
            columns: Vec::with_capacity(columns.len()),
            values: Vec::with_capacity(columns.len()),
            column_index: HashMap::with_capacity(columns.len()),
        };
        for (index, column) in columns.iter().enumerate() {
            let Some(value) = values.get(index) else {
                continue;
            };
            row.columns.push(column.clone());
            row.values.push(value.clone());
            row.column_index.insert(column.clone(), row.columns.len() - 1);
        }
        row
    }

    /// Getting values by column name
    pub fn get(&self, column: &str) -> Option<&Value> {
        self.column_index
            .get(column)
            .and_then(|index| self.values.get(*index))
    }

    /// Getting values by index
    pub fn get_by_index(&self, index: usize) -> Option<&Value> {
        self.values.get(index)
    }

    /// Get all column names in result order
    pub fn columns(&self) -> Vec<&String> {
        self.columns.iter().collect()
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
        self.column_index.contains_key(column)
    }

    // Typed Acquisition Methods

    /// Getting String Values
    pub fn get_string(&self, column: &str) -> Option<String> {
        self.get(column).and_then(|v| match v {
            Value::String(s) => Some(s.to_string()),
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
    pub fn get_list(&self, column: &str) -> Option<&linkrs_core::value::list::List> {
        self.get(column).and_then(|v| match v {
            Value::List(list) => Some(list.as_ref()),
            _ => None,
        })
    }

    /// Getting the mapping
    pub fn get_map(&self, column: &str) -> Option<&HashMap<Value, Value>> {
        self.get(column).and_then(|v| match v {
            Value::Map(map) => Some(map.as_ref()),
            _ => None,
        })
    }

    /// Get all (column, value) pairs in result order
    pub fn values(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.columns.iter().map(String::as_str).zip(self.values.iter())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_preserves_column_order_and_positional_access() {
        let columns = vec!["b".to_string(), "a".to_string()];
        let values = vec![Value::Int(2), Value::string("x")];
        let row = Row::from_columns(&columns, &values);

        assert_eq!(row.get_by_index(0), Some(&Value::Int(2)));
        assert_eq!(row.get_by_index(1), Some(&Value::string("x")));
        assert_eq!(row.columns(), vec![&"b".to_string(), &"a".to_string()]);
        assert_eq!(row.get("a"), Some(&Value::string("x")));
        assert_eq!(row.get("b"), Some(&Value::Int(2)));
        assert_eq!(row.len(), 2);
        assert!(!row.is_empty());

        let pairs: Vec<(&str, &Value)> = row.values().collect();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "b");
        assert_eq!(pairs[1].0, "a");
    }

    #[test]
    fn row_skips_columns_without_values() {
        // The engine row is shorter than the column list: only the
        // leading columns carry values.
        let columns = vec!["a".to_string(), "missing".to_string(), "c".to_string()];
        let values = vec![Value::Int(1)];
        let row = Row::from_columns(&columns, &values);

        assert_eq!(row.len(), 1);
        assert!(!row.has_column("missing"));
        assert!(!row.has_column("c"));
        assert_eq!(row.get("a"), Some(&Value::Int(1)));
    }

    #[test]
    fn row_serializes_as_ordered_json_object() {
        let columns = vec!["b".to_string(), "a".to_string()];
        let values = vec![Value::Int(2), Value::string("x")];
        let row = Row::from_columns(&columns, &values);

        let json = row.to_json().expect("serialization succeeds");
        let b_pos = json.find("\"b\"").expect("column b present");
        let a_pos = json.find("\"a\"").expect("column a present");
        assert!(b_pos < a_pos, "json keeps column order: {json}");

        let restored: Row =
            serde_json::from_str(&json).expect("deserialization succeeds");
        assert_eq!(restored.columns(), vec![&"b".to_string(), &"a".to_string()]);
        assert_eq!(restored.get("a"), Some(&Value::string("x")));
    }
}
