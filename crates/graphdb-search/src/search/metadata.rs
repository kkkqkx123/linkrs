use crate::search::engine::EngineType;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadata {
    pub index_id: String,
    pub index_name: String,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub engine_type: EngineType,
    pub storage_path: String,
    pub created_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub doc_count: usize,
    pub status: IndexStatus,
    pub engine_config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexStatus {
    Creating,
    Active,
    Rebuilding,
    Disabled,
    Error,
}

impl std::fmt::Display for IndexStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexStatus::Creating => write!(f, "CREATING"),
            IndexStatus::Active => write!(f, "ACTIVE"),
            IndexStatus::Rebuilding => write!(f, "REBUILDING"),
            IndexStatus::Disabled => write!(f, "DISABLED"),
            IndexStatus::Error => write!(f, "ERROR"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndexKey {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
}

const FULLTEXT_INDEX_PREFIX: &str = "space_ft";

impl IndexKey {
    pub fn new(space_id: u64, tag_name: &str, field_name: &str) -> Self {
        Self {
            space_id,
            tag_name: tag_name.to_string(),
            field_name: field_name.to_string(),
        }
    }

    pub fn to_index_id(&self) -> String {
        format!(
            "{}_{}_{}_{}",
            FULLTEXT_INDEX_PREFIX, self.space_id, self.tag_name, self.field_name
        )
    }
}
