//! Space Isolation and Index Naming Tests
//!
//! Test scope:
//! - Index naming consistency (vector and fulltext)
//! - Space isolation levels (Shared, Directory, Device)
//! - Space existence validation
//! - Tag existence validation
//! - Storage path configuration

#[cfg(test)]
mod tests {
    use crate::core::metadata::SchemaManager;
    use crate::core::types::{IsolationLevel, SpaceInfo, TagInfo};
    use crate::search::metadata::IndexKey;
    use crate::search::{EngineType, FulltextConfig, FulltextIndexManager, SearchError};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    const FULLTEXT_INDEX_PREFIX: &str = "space_ft";
    const VECTOR_COLLECTION_PREFIX: &str = "space";

    // ==================== Index Naming Tests ====================

    /// Test fulltext index ID format: space_ft_{space_id}_{tag}_{field}
    #[test]
    fn test_fulltext_index_naming_format() {
        let key = IndexKey::new(1, "Article", "content");
        let index_id = key.to_index_id();

        assert_eq!(index_id, "space_ft_1_Article_content");
        assert!(index_id.starts_with(FULLTEXT_INDEX_PREFIX));
    }

    /// Test vector index naming format: space_{space_id}
    #[cfg(feature = "qdrant")]
    #[test]
    fn test_vector_index_naming_format() {
        use graphdb_sync::sync::vector_sync::VectorIndexLocation;

        let location = VectorIndexLocation::new(1, "Article", "content");
        let collection_name = location.to_collection_name();

        assert_eq!(collection_name, "space_1");
        assert!(collection_name.starts_with(VECTOR_COLLECTION_PREFIX));
    }

    /// Test index naming with special characters in tag/field names
    #[test]
    fn test_index_naming_with_special_chars() {
        let key = IndexKey::new(1, "User_Profile", "email_address");
        let index_id = key.to_index_id();

        assert_eq!(index_id, "space_ft_1_User_Profile_email_address");
    }

    /// Test index naming consistency between vector and fulltext
    #[cfg(feature = "qdrant")]
    #[test]
    fn test_index_naming_consistency() {
        use graphdb_sync::sync::vector_sync::VectorIndexLocation;

        let space_id = 42;
        let tag = "Product";
        let field = "description";

        let ft_key = IndexKey::new(space_id, tag, field);
        let ft_index_id = ft_key.to_index_id();

        let vec_location = VectorIndexLocation::new(space_id, tag, field);
        let vec_collection = vec_location.to_collection_name();

        // Vector collection only contains space_id (one collection per space)
        assert!(vec_collection.contains(&space_id.to_string()));
        // Tag and field are in group_id payload, not collection name

        // Prefixes should be different
        assert!(ft_index_id.starts_with("space_ft_"));
        assert!(vec_collection.starts_with("space_"));
    }

    // ==================== SpaceInfo Isolation Level Tests ====================

    /// Test default isolation level is Shared
    #[test]
    fn test_default_isolation_level() {
        let space = SpaceInfo::new("test_space".to_string());

        assert_eq!(space.isolation_level, IsolationLevel::Shared);
        assert!(space.storage_path.is_none());
    }

    /// Test Directory isolation level
    #[test]
    fn test_directory_isolation_level() {
        let space = SpaceInfo::new("test_space".to_string())
            .with_isolation_level(IsolationLevel::Directory);

        assert_eq!(space.isolation_level, IsolationLevel::Directory);
    }

    /// Test Device isolation level with custom path
    #[test]
    fn test_device_isolation_level_with_path() {
        let custom_path = PathBuf::from("/custom/storage/path");
        let space =
            SpaceInfo::new("test_space".to_string()).with_storage_path(Some(custom_path.clone()));

        assert_eq!(space.isolation_level, IsolationLevel::Device);
        assert_eq!(space.storage_path, Some(custom_path));
    }

    /// Test setting isolation level explicitly
    #[test]
    fn test_explicit_isolation_level() {
        let space =
            SpaceInfo::new("test_space".to_string()).with_isolation_level(IsolationLevel::Device);

        assert_eq!(space.isolation_level, IsolationLevel::Device);
    }

    // ==================== Helper Functions ====================

    fn create_test_schema_manager_with_space(space: SpaceInfo) -> Arc<SchemaManager> {
        let manager = SchemaManager::new();
        let mut space = space;
        let _ = manager.create_space(&mut space);
        Arc::new(manager)
    }

    // ==================== Space Existence Validation Tests ====================

    /// Test index creation fails when space does not exist
    #[tokio::test]
    async fn test_create_index_fails_when_space_not_exists() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            ..Default::default()
        };

        let mut space = SpaceInfo::new("existing_space".to_string());
        space.space_id = 1;
        let schema_manager = create_test_schema_manager_with_space(space);

        let manager = FulltextIndexManager::new(config)
            .expect("Failed to create manager")
            .with_schema_manager(schema_manager);

        let result = manager.create_index(999, "Article", "content", None).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SearchError::SpaceNotFound(999)
        ));
    }

    /// Test index creation succeeds when space exists
    #[tokio::test]
    async fn test_create_index_succeeds_when_space_exists() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            ..Default::default()
        };

        let mut space = SpaceInfo::new("test_space".to_string());
        space.space_id = 1;
        space.tags.push(TagInfo::new("Article".to_string()));
        let schema_manager = create_test_schema_manager_with_space(space);

        let manager = FulltextIndexManager::new(config)
            .expect("Failed to create manager")
            .with_schema_manager(schema_manager);

        let result = manager.create_index(1, "Article", "content", None).await;

        assert!(result.is_ok());
    }

    /// Test index creation fails when tag does not exist
    #[tokio::test]
    async fn test_create_index_fails_when_tag_not_exists() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            ..Default::default()
        };

        let mut space = SpaceInfo::new("test_space".to_string());
        space.space_id = 1;
        let schema_manager = create_test_schema_manager_with_space(space);

        let manager = FulltextIndexManager::new(config)
            .expect("Failed to create manager")
            .with_schema_manager(schema_manager);

        let result = manager
            .create_index(1, "NonExistentTag", "content", None)
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SearchError::TagNotFound(_)));
    }

    // ==================== Storage Path Tests ====================

    /// Test shared isolation level uses base path
    #[tokio::test]
    async fn test_shared_isolation_uses_base_path() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_path = temp_dir.path().to_path_buf();

        let config = FulltextConfig {
            enabled: true,
            index_path: base_path.clone(),
            default_engine: EngineType::Bm25,
            ..Default::default()
        };

        let mut space = SpaceInfo::new("test_space".to_string());
        space.space_id = 1;
        space.isolation_level = IsolationLevel::Shared;
        space.tags.push(TagInfo::new("Article".to_string()));
        let schema_manager = create_test_schema_manager_with_space(space);

        let manager = FulltextIndexManager::new(config)
            .expect("Failed to create manager")
            .with_schema_manager(schema_manager);

        let _index_id = manager
            .create_index(1, "Article", "content", None)
            .await
            .expect("Failed to create index");

        let metadata = manager
            .get_metadata(1, "Article", "content")
            .expect("Metadata should exist");

        assert!(metadata
            .storage_path
            .starts_with(base_path.to_string_lossy().as_ref()));
    }

    /// Test directory isolation level creates subdirectory
    #[tokio::test]
    async fn test_directory_isolation_creates_subdirectory() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_path = temp_dir.path().to_path_buf();

        let config = FulltextConfig {
            enabled: true,
            index_path: base_path.clone(),
            default_engine: EngineType::Bm25,
            ..Default::default()
        };

        let mut space = SpaceInfo::new("test_space".to_string());
        space.space_id = 1;
        space.isolation_level = IsolationLevel::Directory;
        space.tags.push(TagInfo::new("Article".to_string()));
        let schema_manager = create_test_schema_manager_with_space(space);

        let manager = FulltextIndexManager::new(config)
            .expect("Failed to create manager")
            .with_schema_manager(schema_manager);

        let _index_id = manager
            .create_index(1, "Article", "content", None)
            .await
            .expect("Failed to create index");

        let metadata = manager
            .get_metadata(1, "Article", "content")
            .expect("Metadata should exist");

        assert!(metadata.storage_path.contains("space_1"));
    }

    /// Test device isolation level uses custom path
    #[tokio::test]
    async fn test_device_isolation_uses_custom_path() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_path = temp_dir.path().to_path_buf();
        let custom_path = temp_dir.path().join("custom_device");

        let config = FulltextConfig {
            enabled: true,
            index_path: base_path,
            default_engine: EngineType::Bm25,
            ..Default::default()
        };

        let mut space = SpaceInfo::new("test_space".to_string());
        space.space_id = 1;
        space.isolation_level = IsolationLevel::Device;
        space.storage_path = Some(custom_path.clone());
        space.tags.push(TagInfo::new("Article".to_string()));
        let schema_manager = create_test_schema_manager_with_space(space);

        let manager = FulltextIndexManager::new(config)
            .expect("Failed to create manager")
            .with_schema_manager(schema_manager);

        let _index_id = manager
            .create_index(1, "Article", "content", None)
            .await
            .expect("Failed to create index");

        let metadata = manager
            .get_metadata(1, "Article", "content")
            .expect("Metadata should exist");

        assert!(metadata
            .storage_path
            .starts_with(custom_path.to_string_lossy().as_ref()));
    }

    // ==================== Drop Space Indexes Tests ====================

    /// Test drop_space_indexes removes all indexes for a space
    #[tokio::test]
    async fn test_drop_space_indexes_removes_all() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            ..Default::default()
        };

        let mut space = SpaceInfo::new("test_space".to_string());
        space.space_id = 1;
        space.tags.push(TagInfo::new("Article".to_string()));
        space.tags.push(TagInfo::new("Product".to_string()));
        let schema_manager = create_test_schema_manager_with_space(space);

        let manager = FulltextIndexManager::new(config)
            .expect("Failed to create manager")
            .with_schema_manager(schema_manager);

        manager
            .create_index(1, "Article", "title", None)
            .await
            .expect("Failed to create index 1");
        manager
            .create_index(1, "Article", "content", None)
            .await
            .expect("Failed to create index 2");
        manager
            .create_index(1, "Product", "description", None)
            .await
            .expect("Failed to create index 3");

        assert_eq!(manager.get_space_indexes(1).len(), 3);

        manager
            .drop_space_indexes(1)
            .await
            .expect("Failed to drop space indexes");

        assert_eq!(manager.get_space_indexes(1).len(), 0);
        assert!(!manager.has_index(1, "Article", "title"));
        assert!(!manager.has_index(1, "Article", "content"));
        assert!(!manager.has_index(1, "Product", "description"));
    }

    /// Test drop_space_indexes only affects specified space
    #[tokio::test]
    async fn test_drop_space_indexes_isolation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            ..Default::default()
        };

        let manager = SchemaManager::new();

        let mut space1 = SpaceInfo::new("space1".to_string());
        space1.space_id = 1;
        space1.tags.push(TagInfo::new("Article".to_string()));
        let _ = manager.create_space(&mut space1);

        let mut space2 = SpaceInfo::new("space2".to_string());
        space2.space_id = 2;
        space2.tags.push(TagInfo::new("Product".to_string()));
        let _ = manager.create_space(&mut space2);

        let schema_manager = Arc::new(manager);
        let manager = FulltextIndexManager::new(config)
            .expect("Failed to create manager")
            .with_schema_manager(schema_manager);

        manager
            .create_index(1, "Article", "content", None)
            .await
            .expect("Failed to create space 1 index");
        manager
            .create_index(2, "Product", "description", None)
            .await
            .expect("Failed to create space 2 index");

        manager
            .drop_space_indexes(1)
            .await
            .expect("Failed to drop space 1 indexes");

        assert!(!manager.has_index(1, "Article", "content"));

        assert!(manager.has_index(2, "Product", "description"));
    }

    // ==================== Edge Case Tests ====================

    /// Test index creation with space_id = 0
    #[test]
    fn test_index_naming_with_zero_space_id() {
        let key = IndexKey::new(0, "Tag", "field");
        let index_id = key.to_index_id();

        assert_eq!(index_id, "space_ft_0_Tag_field");
    }

    /// Test without schema manager (backward compatibility)
    #[tokio::test]
    async fn test_create_index_without_schema_manager() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            ..Default::default()
        };

        let manager = FulltextIndexManager::new(config).expect("Failed to create manager");

        let result = manager.create_index(1, "Article", "content", None).await;
        assert!(result.is_ok());
    }

    /// Test concurrent index creation for same space
    #[tokio::test]
    async fn test_concurrent_index_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            ..Default::default()
        };

        let schema_mgr = SchemaManager::new();
        let mut space = SpaceInfo::new("test_space".to_string());
        space.space_id = 1;
        for i in 0..5 {
            space.tags.push(TagInfo::new(format!("Tag{}", i)));
        }
        let _ = schema_mgr.create_space(&mut space);

        let manager = Arc::new(
            FulltextIndexManager::new(config)
                .expect("Failed to create manager")
                .with_schema_manager(Arc::new(schema_mgr)),
        );

        let mut handles = vec![];
        for i in 0..5 {
            let mgr = manager.clone();
            let handle = tokio::spawn(async move {
                mgr.create_index(1, &format!("Tag{}", i), "field", None)
                    .await
            });
            handles.push(handle);
        }

        for handle in handles {
            let result = handle.await.expect("Task failed");
            assert!(result.is_ok());
        }

        assert_eq!(manager.get_space_indexes(1).len(), 5);
    }
}
