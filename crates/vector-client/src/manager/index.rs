//! Index metadata

use serde::{Deserialize, Serialize};

use crate::types::CollectionConfig;

/// Index metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadata {
    pub name: String,
    pub config: CollectionConfig,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub vector_count: u64,
    pub index_name: Option<String>,
}

impl IndexMetadata {
    pub fn new(name: String, config: CollectionConfig) -> Self {
        Self {
            name,
            config,
            created_at: chrono::Utc::now(),
            vector_count: 0,
            index_name: None,
        }
    }

    pub fn with_index_name(name: String, config: CollectionConfig, index_name: String) -> Self {
        Self {
            name,
            config,
            created_at: chrono::Utc::now(),
            vector_count: 0,
            index_name: Some(index_name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_metadata_new() {
        let cfg = CollectionConfig::new(384, crate::types::DistanceMetric::Cosine);
        let meta = IndexMetadata::new("test_idx".into(), cfg.clone());
        assert_eq!(meta.name, "test_idx");
        assert_eq!(meta.config.vector_size, 384);
        assert_eq!(meta.vector_count, 0);
    }

    #[test]
    fn test_index_metadata_serialize() {
        let cfg = CollectionConfig::new(128, crate::types::DistanceMetric::Dot);
        let meta = IndexMetadata::new("serde_test".into(), cfg);
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("serde_test"));
        assert!(json.contains("\"vector_count\":0"));
    }
}
