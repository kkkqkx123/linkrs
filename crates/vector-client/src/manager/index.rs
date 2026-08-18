//! Index metadata
//!
//! The `IndexMetadata` type now lives in `vector-search::types` so the sync
//! layer can reference it without depending on the transport-specific crate.
//! This module only re-exports it for backward compatibility.

pub use vector_search::types::IndexMetadata;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_metadata_new() {
        let cfg = crate::types::CollectionConfig::new(384, crate::types::DistanceMetric::Cosine);
        let meta = IndexMetadata::new("test_idx".into(), cfg.clone());
        assert_eq!(meta.name, "test_idx");
        assert_eq!(meta.config.vector_size, 384);
        assert_eq!(meta.vector_count, 0);
    }

    #[test]
    fn test_index_metadata_serialize() {
        let cfg = crate::types::CollectionConfig::new(128, crate::types::DistanceMetric::Dot);
        let meta = IndexMetadata::new("serde_test".into(), cfg);
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("serde_test"));
        assert!(json.contains("\"vector_count\":0"));
    }
}
