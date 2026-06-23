use crate::search::manager::FulltextIndexManager;
use std::sync::Arc;

/// Index warmer for preloading frequently accessed indexes
pub struct IndexWarmer {
    fulltext_manager: Arc<FulltextIndexManager>,
}

impl IndexWarmer {
    pub fn new(fulltext_manager: Arc<FulltextIndexManager>) -> Self {
        Self { fulltext_manager }
    }

    /// Warm up common queries
    pub async fn warm_common_queries(&self) {
        let common_queries = vec![
            (1, "Post", "content", "tutorial"),
            (1, "Article", "title", "Rust"),
            (1, "User", "name", "admin"),
        ];

        for (space_id, tag, field, query) in common_queries {
            // Execute search to load index into memory
            let _ = self
                .fulltext_manager
                .search(space_id, tag, field, query, 10)
                .await;
        }
    }

    /// Warm up specific index
    pub async fn warm_index(&self, space_id: u64, tag: &str, field: &str) {
        if let Some(engine) = self.fulltext_manager.get_engine(space_id, tag, field) {
            // Execute wildcard search to load index structure
            let _ = engine.search("*", 1).await;
        }
    }

    /// Warm up all indexes in a space
    pub async fn warm_space(&self, space_id: u64) {
        let indexes = self.fulltext_manager.list_indexes();
        for metadata in indexes {
            if metadata.space_id == space_id {
                self.warm_index(space_id, &metadata.tag_name, &metadata.field_name)
                    .await;
            }
        }
    }

    /// Warm up with custom query patterns
    pub async fn warm_with_patterns(
        &self,
        space_id: u64,
        tag: &str,
        field: &str,
        patterns: Vec<&str>,
    ) {
        for pattern in patterns {
            let _ = self
                .fulltext_manager
                .search(space_id, tag, field, pattern, 10)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::config::FulltextConfig;
    use crate::search::engine::EngineType;
    #[cfg(feature = "fulltext-search")]
    use crate::search::manager::FulltextIndexManager;
    use tempfile::TempDir;

    async fn setup_test_manager() -> (Arc<FulltextIndexManager>, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            sync: Default::default(),
            tantivy: Default::default(),
            cache_size: 100,
            max_result_cache: 1000,
            result_cache_ttl_secs: 60,
        };
        let manager =
            Arc::new(FulltextIndexManager::new(config).expect("Failed to create manager"));
        (manager, temp_dir)
    }

    #[tokio::test]
    async fn test_warm_index() {
        let (manager, _temp) = setup_test_manager().await;

        // Create index
        manager
            .create_index(1, "Article", "title", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");

        // Warm up index
        let warmer = IndexWarmer::new(manager.clone());
        warmer.warm_index(1, "Article", "title").await;

        // After warming, search should work (even if empty)
        let results = manager
            .search(1, "Article", "title", "Test", 10)
            .await
            .expect("Failed to search");
        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_warm_space() {
        let (manager, _temp) = setup_test_manager().await;

        // Create multiple indexes
        manager
            .create_index(1, "Article", "title", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");
        manager
            .create_index(1, "Article", "content", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");

        // Warm up entire space
        let warmer = IndexWarmer::new(manager.clone());
        warmer.warm_space(1).await;

        // Both indexes should be searchable (even if empty)
        let title_results = manager
            .search(1, "Article", "title", "Test", 10)
            .await
            .expect("Failed to search title");
        let content_results = manager
            .search(1, "Article", "content", "Test", 10)
            .await
            .expect("Failed to search content");

        assert_eq!(title_results.len(), 0);
        assert_eq!(content_results.len(), 0);
    }

    #[tokio::test]
    async fn test_warm_with_patterns() {
        let (manager, _temp) = setup_test_manager().await;

        // Create index
        manager
            .create_index(1, "Article", "title", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");

        // Warm up with specific patterns
        let warmer = IndexWarmer::new(manager.clone());
        warmer
            .warm_with_patterns(
                1,
                "Article",
                "title",
                vec!["Article 1", "Article 5", "Article 9"],
            )
            .await;

        // Search should work (even if empty)
        let results = manager
            .search(1, "Article", "title", "Article", 10)
            .await
            .expect("Failed to search");
        assert_eq!(results.len(), 0);
    }
}
