//! DataChunk: Basic unit of streaming execution
//!
//! A DataChunk represents a fixed-size batch of rows processed in streaming mode.
//! Typical size: 1024 rows (~4MB)

use crate::core::Value;
use std::sync::Arc;

/// A chunk of rows processed in streaming execution
#[derive(Debug, Clone)]
pub struct DataChunk {
    /// Row data with Value types
    pub rows: Vec<Vec<Value>>,
    /// Schema information (column names and types)
    pub schema: Arc<Schema>,
}

/// Simple schema representation
#[derive(Debug, Clone)]
pub struct Schema {
    pub columns: Vec<ColumnInfo>,
}

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    /// Column data type (inferred from values if not specified)
    pub data_type: String,
}

impl Schema {
    pub fn new(columns: Vec<ColumnInfo>) -> Self {
        Self { columns }
    }

    pub fn empty() -> Self {
        Self { columns: vec![] }
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }
}

impl DataChunk {
    /// Create a new DataChunk with rows and schema
    pub fn new(rows: Vec<Vec<Value>>, schema: Arc<Schema>) -> Self {
        Self { rows, schema }
    }

    /// Create a DataChunk from rows, inferring schema and generating col_N names
    pub fn from_rows(rows: Vec<Vec<Value>>) -> Self {
        Self::from_rows_with_col_names(rows, None)
    }

    /// Create a DataChunk from rows, using provided column names if available
    ///
    /// When col_names is None, falls back to col_N inference (backward compat).
    /// When col_names is Some, uses those names directly.
    pub fn from_rows_with_col_names(rows: Vec<Vec<Value>>, col_names: Option<Vec<String>>) -> Self {
        let schema = if rows.is_empty() {
            if let Some(names) = col_names {
                Arc::new(Schema::new(
                    names
                        .into_iter()
                        .map(|name| ColumnInfo {
                            name,
                            data_type: "unknown".to_string(),
                        })
                        .collect(),
                ))
            } else {
                Arc::new(Schema::empty())
            }
        } else {
            let col_count = rows[0].len();
            let columns = (0..col_count)
                .map(|i| {
                    let name = col_names
                        .as_ref()
                        .and_then(|names| names.get(i).cloned())
                        .unwrap_or_else(|| format!("col_{}", i));

                    let data_type = if let Some(row) = rows.first() {
                        if let Some(val) = row.get(i) {
                            match val {
                                Value::BigInt(_) => "bigint",
                                Value::Int(_) => "int",
                                Value::Double(_) => "double",
                                Value::Float(_) => "float",
                                Value::String(_) => "string",
                                Value::Bool(_) => "bool",
                                Value::Null(_) => "null",
                                _ => "unknown",
                            }
                        } else {
                            "unknown"
                        }
                    } else {
                        "unknown"
                    };

                    ColumnInfo {
                        name,
                        data_type: data_type.to_string(),
                    }
                })
                .collect();
            Arc::new(Schema::new(columns))
        };
        Self { rows, schema }
    }

    /// Number of rows in this chunk
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether this chunk is empty
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Number of columns
    pub fn num_columns(&self) -> usize {
        self.schema.column_count()
    }

    /// Get column names from schema
    pub fn col_names(&self) -> Vec<String> {
        self.schema.columns.iter().map(|c| c.name.clone()).collect()
    }

    /// Get column name by index
    pub fn col_name(&self, index: usize) -> Option<String> {
        self.schema.columns.get(index).map(|c| c.name.clone())
    }

    /// Create column name to index mapping
    pub fn col_name_index(&self) -> std::collections::HashMap<String, usize> {
        self.schema
            .columns
            .iter()
            .enumerate()
            .map(|(i, col)| (col.name.clone(), i))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_chunk_creation() {
        let rows = vec![vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ]];
        let chunk = DataChunk::from_rows(rows);
        assert_eq!(chunk.len(), 1);
        assert_eq!(chunk.num_columns(), 2);
    }

    #[test]
    fn test_data_chunk_empty() {
        let chunk = DataChunk::from_rows(vec![]);
        assert!(chunk.is_empty());
        assert_eq!(chunk.num_columns(), 0);
    }

    #[test]
    fn test_data_chunk_type_inference() {
        let rows = vec![vec![Value::BigInt(42), Value::String("hello".to_string())]];
        let chunk = DataChunk::from_rows(rows);
        assert_eq!(chunk.schema.columns[0].data_type, "bigint");
        assert_eq!(chunk.schema.columns[1].data_type, "string");
    }
}
