//! Schema definitions for DataChunk

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
