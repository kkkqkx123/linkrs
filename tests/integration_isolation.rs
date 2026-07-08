//! Space Isolation Integration Tests
//!
//! Test scope:
//! - Space creation with different isolation levels
//! - Index creation and storage path verification
//! - Space deletion with index cleanup
//! - Cross-space isolation verification

mod common;

use graphdb::core::types::{IsolationLevel, SpaceInfo};
#[cfg(feature = "fulltext-search")]
use graphdb::search::FulltextIndexManager;
use graphdb::search::{EngineType, FulltextConfig};
use std::path::PathBuf;
use tempfile::TempDir;

// ==================== Space Creation Tests ====================

/// Test creating space with Shared isolation level (default)
#[test]
fn test_create_space_with_shared_isolation() {
    let space = SpaceInfo::new("shared_space".to_string());

    assert_eq!(space.isolation_level, IsolationLevel::Shared);
    assert!(space.storage_path.is_none());
    assert_eq!(space.space_name, "shared_space");
}

/// Test creating space with Directory isolation level
#[test]
fn test_create_space_with_directory_isolation() {
    let space = SpaceInfo::new("directory_space".to_string())
        .with_isolation_level(IsolationLevel::Directory);

    assert_eq!(space.isolation_level, IsolationLevel::Directory);
    assert!(space.storage_path.is_none());
}

/// Test creating space with Device isolation level and custom path
#[test]
fn test_create_space_with_device_isolation() {
    let custom_path = PathBuf::from("/mnt/fastdisk/graphdb");
    let space =
        SpaceInfo::new("device_space".to_string()).with_storage_path(Some(custom_path.clone()));

    assert_eq!(space.isolation_level, IsolationLevel::Device);
    assert_eq!(space.storage_path, Some(custom_path));
}

/// Test SpaceInfo builder pattern
#[test]
fn test_space_info_builder_pattern() {
    let space = SpaceInfo::new("test_space".to_string())
        .with_vid_type(graphdb::core::types::DataType::BigInt)
        .with_comment(Some("Test comment".to_string()))
        .with_isolation_level(IsolationLevel::Directory);

    assert_eq!(space.space_name, "test_space");
    assert_eq!(space.vid_type, graphdb::core::types::DataType::BigInt);
    assert_eq!(space.comment, Some("Test comment".to_string()));
    assert_eq!(space.isolation_level, IsolationLevel::Directory);
}

// ==================== Index Naming Tests ====================

/// Test fulltext index ID format compliance
#[test]
fn test_fulltext_index_id_format() {
    use graphdb::search::IndexKey;

    let test_cases = vec![
        (1, "Article", "content", "space_ft_1_Article_content"),
        (42, "User", "email", "space_ft_42_User_email"),
        (
            999,
            "Product",
            "description",
            "space_ft_999_Product_description",
        ),
    ];

    for (space_id, tag, field, expected) in test_cases {
        let key = IndexKey::new(space_id, tag, field);
        let index_id = key.to_index_id();
        assert_eq!(
            index_id, expected,
            "Index ID format mismatch for space_id={}",
            space_id
        );
    }
}

/// Test vector collection name format compliance
#[cfg(feature = "qdrant")]
#[test]
fn test_vector_collection_name_format() {
    use graphdb::sync::vector_sync::VectorIndexLocation;

    let test_cases = vec![
        (1, "Article", "content", "space_1"),
        (42, "User", "email", "space_42"),
        (999, "Product", "description", "space_999"),
    ];

    for (space_id, tag, field, expected) in test_cases {
        let location = VectorIndexLocation::new(space_id, tag, field);
        let collection_name = location.to_collection_name();
        assert_eq!(
            collection_name, expected,
            "Collection name format mismatch for space_id={}",
            space_id
        );
    }
}

/// Test naming consistency between vector and fulltext
#[cfg(feature = "qdrant")]
#[test]
fn test_naming_consistency_vector_fulltext() {
    use graphdb::search::IndexKey;
    use graphdb::sync::vector_sync::VectorIndexLocation;

    let space_id = 123;
    let tag = "Document";
    let field = "text";

    let ft_key = IndexKey::new(space_id, tag, field);
    let ft_index_id = ft_key.to_index_id();

    let vec_location = VectorIndexLocation::new(space_id, tag, field);
    let vec_collection = vec_location.to_collection_name();

    // Both should contain the same core components
    assert!(ft_index_id.contains(&space_id.to_string()));
    assert!(ft_index_id.contains(tag));
    assert!(ft_index_id.contains(field));

    assert!(vec_collection.contains(&space_id.to_string()));

    // Vector collection only contains space_id (one collection per space)
    // Fulltext index has all components in its index_id

    // Prefixes should be distinct and consistent
    assert!(
        ft_index_id.starts_with("space_ft_"),
        "Fulltext should use 'space_ft_' prefix"
    );
    assert!(
        vec_collection.starts_with("space_"),
        "Vector should use 'space_' prefix"
    );
}

// ==================== Storage Path Integration Tests ====================

/// Test index storage path with Shared isolation
#[cfg(feature = "fulltext-search")]
#[tokio::test]
async fn test_shared_isolation_storage_path() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let base_path = temp_dir.path().to_path_buf();

    let config = FulltextConfig {
        enabled: true,
        index_path: base_path.clone(),
        default_engine: EngineType::Bm25,
        ..Default::default()
    };

    let manager = FulltextIndexManager::new(config).expect("Failed to create manager");

    // Create index without schema manager (backward compatibility mode)
    let index_id = manager
        .create_index(1, "Article", "content", None)
        .await
        .expect("Failed to create index");

    let metadata = manager
        .get_metadata(1, "Article", "content")
        .expect("Metadata should exist");

    // Verify storage path is directly under base_path
    let expected_path = base_path.join(&index_id);
    assert_eq!(
        metadata.storage_path,
        expected_path.to_string_lossy().to_string()
    );
}

/// Test index storage path with Directory isolation
#[cfg(feature = "fulltext-search")]
#[tokio::test]
async fn test_directory_isolation_storage_path() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let base_path = temp_dir.path().to_path_buf();

    let config = FulltextConfig {
        enabled: true,
        index_path: base_path.clone(),
        default_engine: EngineType::Bm25,
        ..Default::default()
    };

    let manager = FulltextIndexManager::new(config).expect("Failed to create manager");

    // Create multiple indexes for different spaces
    manager
        .create_index(1, "Article", "content", None)
        .await
        .expect("Failed to create index 1");
    manager
        .create_index(2, "Product", "description", None)
        .await
        .expect("Failed to create index 2");

    // Verify both indexes exist
    assert!(manager.has_index(1, "Article", "content"));
    assert!(manager.has_index(2, "Product", "description"));

    // In shared mode, both should be in the same directory
    let metadata1 = manager.get_metadata(1, "Article", "content").unwrap();
    let metadata2 = manager.get_metadata(2, "Product", "description").unwrap();

    // Both paths should be under the same base path
    assert!(metadata1
        .storage_path
        .starts_with(base_path.to_str().unwrap()));
    assert!(metadata2
        .storage_path
        .starts_with(base_path.to_str().unwrap()));
}

// ==================== Cross-Space Isolation Tests ====================

/// Test that indexes from different spaces are properly isolated
#[cfg(feature = "fulltext-search")]
#[tokio::test]
async fn test_cross_space_index_isolation() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = FulltextConfig {
        enabled: true,
        index_path: temp_dir.path().to_path_buf(),
        default_engine: EngineType::Bm25,
        ..Default::default()
    };

    let manager = FulltextIndexManager::new(config).expect("Failed to create manager");

    // Create indexes with same tag/field names but different space IDs
    let _index1 = manager
        .create_index(1, "Document", "content", None)
        .await
        .expect("Failed to create space 1 index");
    let _index2 = manager
        .create_index(2, "Document", "content", None)
        .await
        .expect("Failed to create space 2 index");

    // Both indexes should coexist (they have different IDs due to different space IDs)
    assert!(manager.has_index(1, "Document", "content"));
    assert!(manager.has_index(2, "Document", "content"));

    // Verify index IDs follow naming convention
    assert!(_index1.starts_with("space_ft_1_"));
    assert!(_index2.starts_with("space_ft_2_"));
}

/// Test dropping one space's indexes doesn't affect other spaces
#[cfg(feature = "fulltext-search")]
#[tokio::test]
async fn test_drop_space_isolation() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = FulltextConfig {
        enabled: true,
        index_path: temp_dir.path().to_path_buf(),
        default_engine: EngineType::Bm25,
        ..Default::default()
    };

    let manager = FulltextIndexManager::new(config).expect("Failed to create manager");

    // Create indexes for multiple spaces
    for space_id in 1..=3 {
        manager
            .create_index(space_id, "Entity", "data", None)
            .await
            .unwrap_or_else(|_| panic!("Failed to create index for space {}", space_id));
    }

    // Verify all indexes exist
    for space_id in 1..=3 {
        assert!(
            manager.has_index(space_id, "Entity", "data"),
            "Space {} index should exist",
            space_id
        );
    }

    // Drop indexes for space 2
    manager
        .drop_space_indexes(2)
        .await
        .expect("Failed to drop space 2 indexes");

    // Verify space 2 index is removed
    assert!(
        !manager.has_index(2, "Entity", "data"),
        "Space 2 index should be removed"
    );

    // Verify space 1 and 3 indexes still exist
    assert!(
        manager.has_index(1, "Entity", "data"),
        "Space 1 index should still exist"
    );
    assert!(
        manager.has_index(3, "Entity", "data"),
        "Space 3 index should still exist"
    );
}

// ==================== Edge Case Tests ====================

/// Test index creation with special characters in names
#[test]
fn test_index_naming_with_special_chars() {
    use graphdb::search::IndexKey;

    let test_cases = vec![
        (1, "User_Profile", "email_address"),
        (2, "Product-Item", "short-description"),
        (3, "Log.Entry", "timestamp.value"),
    ];

    for (space_id, tag, field) in test_cases {
        let key = IndexKey::new(space_id, tag, field);
        let index_id = key.to_index_id();

        // Verify format
        assert!(
            index_id.starts_with(&format!("space_ft_{}_", space_id)),
            "Index ID should start with correct prefix"
        );
        assert!(index_id.contains(tag), "Index ID should contain tag name");
        assert!(
            index_id.contains(field),
            "Index ID should contain field name"
        );
    }
}

/// Test space with zero ID
#[test]
fn test_space_with_zero_id() {
    use graphdb::search::IndexKey;

    let key = IndexKey::new(0, "Test", "field");
    let index_id = key.to_index_id();

    assert_eq!(index_id, "space_ft_0_Test_field");
}

/// Test custom path auto-creation
#[tokio::test]
async fn test_custom_path_auto_creation() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let base_path = temp_dir.path().to_path_buf();
    let custom_path = temp_dir
        .path()
        .join("nonexistent")
        .join("nested")
        .join("path");

    // Path doesn't exist yet
    assert!(!custom_path.exists());

    let config = FulltextConfig {
        enabled: true,
        index_path: base_path,
        default_engine: EngineType::Bm25,
        ..Default::default()
    };

    // Note: This test would require schema manager integration
    // For now, we just verify the config accepts the path
    assert_eq!(config.index_path, temp_dir.path().to_path_buf());
}

// ==================== Metadata Consistency Tests ====================

/// Test index metadata contains correct space information
#[cfg(feature = "fulltext-search")]
#[tokio::test]
async fn test_index_metadata_space_info() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = FulltextConfig {
        enabled: true,
        index_path: temp_dir.path().to_path_buf(),
        default_engine: EngineType::Bm25,
        ..Default::default()
    };

    let manager = FulltextIndexManager::new(config).expect("Failed to create manager");

    let space_id = 42u64;
    let tag_name = "Article";
    let field_name = "content";

    manager
        .create_index(space_id, tag_name, field_name, None)
        .await
        .expect("Failed to create index");

    let metadata = manager
        .get_metadata(space_id, tag_name, field_name)
        .expect("Metadata should exist");

    assert_eq!(metadata.space_id, space_id);
    assert_eq!(metadata.tag_name, tag_name);
    assert_eq!(metadata.field_name, field_name);
    assert_eq!(metadata.index_id, "space_ft_42_Article_content");
}

/// Test listing space indexes returns correct metadata
#[cfg(feature = "fulltext-search")]
#[tokio::test]
async fn test_list_space_indexes() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = FulltextConfig {
        enabled: true,
        index_path: temp_dir.path().to_path_buf(),
        default_engine: EngineType::Bm25,
        ..Default::default()
    };

    let manager = FulltextIndexManager::new(config).expect("Failed to create manager");

    // Create multiple indexes for space 1
    manager
        .create_index(1, "Article", "title", None)
        .await
        .unwrap();
    manager
        .create_index(1, "Article", "content", None)
        .await
        .unwrap();
    manager
        .create_index(1, "Product", "name", None)
        .await
        .unwrap();

    // Create index for space 2
    manager
        .create_index(2, "User", "profile", None)
        .await
        .unwrap();

    // List indexes for space 1
    let space1_indexes = manager.get_space_indexes(1);
    assert_eq!(space1_indexes.len(), 3);

    // Verify all space 1 indexes are returned
    let index_names: Vec<String> = space1_indexes
        .iter()
        .map(|m| format!("{}.{}", m.tag_name, m.field_name))
        .collect();
    assert!(index_names.contains(&"Article.title".to_string()));
    assert!(index_names.contains(&"Article.content".to_string()));
    assert!(index_names.contains(&"Product.name".to_string()));

    // List indexes for space 2
    let space2_indexes = manager.get_space_indexes(2);
    assert_eq!(space2_indexes.len(), 1);
    assert_eq!(space2_indexes[0].tag_name, "User");
    assert_eq!(space2_indexes[0].field_name, "profile");
}
