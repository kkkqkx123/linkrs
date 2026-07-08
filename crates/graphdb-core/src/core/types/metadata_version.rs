//! Metadata version management type

use crate::core::types::{EdgeTypeInfo, TagInfo};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetadataVersion {
    pub version: i32,
    pub timestamp: i64,
    pub description: String,
}

impl Default for MetadataVersion {
    fn default() -> Self {
        Self {
            version: 1,
            timestamp: chrono::Utc::now().timestamp_millis(),
            description: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SchemaVersion {
    pub version: i32,
    pub space_id: u64,
    pub tags: Vec<TagInfo>,
    pub edge_types: Vec<EdgeTypeInfo>,
    pub created_at: i64,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SchemaHistory {
    pub space_id: u64,
    pub versions: Vec<SchemaVersion>,
    pub current_version: i64,
    pub timestamp: i64,
}

impl Default for SchemaHistory {
    fn default() -> Self {
        Self {
            space_id: 0,
            versions: Vec::new(),
            current_version: 0,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}
