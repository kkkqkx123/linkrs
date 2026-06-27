//! DataChunk: Basic unit of streaming execution
//!
//! A DataChunk represents a fixed-size batch of rows processed in streaming mode.
//! Typical size: 1024 rows (~4MB)

use std::sync::Arc;

/// A chunk of rows processed in streaming execution
#[derive(Debug, Clone)]
pub struct DataChunk {
    /// Row data
    pub rows: Vec<Vec<String>>,
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
    pub data_type: String,
}

impl Schema {
    pub fn new(columns: Vec<ColumnInfo>) -> Self {
        Self { columns }
    }

    pub fn empty() -> Self {
        Self {
            columns: vec![],
        }
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }
}

impl DataChunk {
    /// Create a new DataChunk with rows and schema
    pub fn new(rows: Vec<Vec<String>>, schema: Arc<Schema>) -> Self {
        Self { rows, schema }
    }

    /// Create a DataChunk from rows, inferring schema
    pub fn from_rows(rows: Vec<Vec<String>>) -> Self {
        let schema = if rows.is_empty() {
            Arc::new(Schema::empty())
        } else {
            // Simple schema inference - just use column count
            let col_count = rows[0].len();
            let columns = (0..col_count)
                .map(|i| ColumnInfo {
                    name: format!("col_{}", i),
                    data_type: "string".to_string(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_chunk_creation() {
        let rows = vec![vec!["a".to_string(), "b".to_string()]];
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
}
