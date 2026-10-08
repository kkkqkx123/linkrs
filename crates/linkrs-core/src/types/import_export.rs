//! Import/Export type definitions

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaExportConfig {
    pub space_id: Option<u64>,
    pub format: ExportFormat,
    pub include_comments: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExportFormat {
    JSON,
    YAML,
    Rust,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SchemaImportResult {
    pub success: bool,
    pub space_name: String,
    pub imported_items: i32,
    pub imported_tags: Vec<String>,
    pub imported_edge_types: Vec<String>,
    pub skipped_items: Vec<String>,
    pub errors: Vec<String>,
}

impl SchemaImportResult {
    pub fn new() -> Self {
        Self::default()
    }
}
