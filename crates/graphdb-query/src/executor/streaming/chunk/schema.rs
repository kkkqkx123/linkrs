//! Schema definitions for DataChunk

use graphdb_core::DataType;

/// Simple schema representation
#[derive(Debug, Clone)]
pub struct Schema {
    pub columns: Vec<ColumnInfo>,
}

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    /// Column data type (inferred from values if not specified)
    ///
    /// Display-oriented lowercase string (e.g. `"int"`, `"unknown"`).
    /// Use [`ColumnInfo::typed_data_type`] for the typed view.
    pub data_type: String,
}

impl ColumnInfo {
    /// Typed view of [`ColumnInfo::data_type`].
    ///
    /// Unrecognized strings map to `DataType::Unknown` so display-only
    /// producers never break typed consumers.
    pub fn typed_data_type(&self) -> DataType {
        match self.data_type.to_lowercase().as_str() {
            "bool" => DataType::Bool,
            "int" | "integer" | "int64" => DataType::Int,
            "float" | "double" | "float64" => DataType::Float,
            "string" | "str" => DataType::String,
            _ => DataType::Unknown,
        }
    }
}

impl Schema {
    pub fn new(columns: Vec<ColumnInfo>) -> Self {
        Self { columns }
    }

    /// Build a schema from names plus resolved types.
    pub fn with_typed_data_types(names: &[String], types: &[DataType]) -> Self {
        let columns = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let data_type = types
                    .get(i)
                    .map(|dt| dt.to_string().to_lowercase())
                    .unwrap_or_else(|| "unknown".to_string());
                ColumnInfo {
                    name: name.clone(),
                    data_type,
                }
            })
            .collect();
        Self { columns }
    }

    pub fn empty() -> Self {
        Self { columns: vec![] }
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }
}
