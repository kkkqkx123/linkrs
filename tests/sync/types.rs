//! External Index Client Tests (TC-220 ~ TC-230)
//!
//! Tests for external index client infrastructure

/// TC-220: Index key structure
#[test]
fn test_index_key_structure() {
    use graphdb::sync::IndexOpKey;

    let key = IndexOpKey::new(42, "Person", "name");

    assert_eq!(key.space_id, 42);
    assert_eq!(key.tag_name, "Person");
    assert_eq!(key.field_name, "name");

    let key2 = IndexOpKey::new(42, "Person", "name");
    assert_eq!(key, key2);

    let key3 = IndexOpKey::new(43, "Person", "name");
    assert_ne!(key, key3);
}

/// TC-221: Index operation types
#[test]
fn test_index_operation_types() {
    use graphdb::sync::types::{ChangeType, IndexData, IndexType};
    use graphdb::sync::{IndexOpKey, IndexOperation};

    let insert_op = IndexOperation {
        key: IndexOpKey::new(1, "tag", "field"),
        index_type: IndexType::Fulltext,
        change_type: ChangeType::Insert,
        id: "doc1".to_string(),
        data: Some(IndexData::Fulltext("test content".to_string())),
    };

    let key = insert_op.extract_index_key();
    assert_eq!(key, (1, "tag".to_string(), "field".to_string()));

    let delete_op = IndexOperation {
        key: IndexOpKey::new(2, "tag2", "field2"),
        index_type: IndexType::Fulltext,
        change_type: ChangeType::Delete,
        id: "doc2".to_string(),
        data: None,
    };
    let key = delete_op.extract_index_key();
    assert_eq!(key, (2, "tag2".to_string(), "field2".to_string()));

    let update_op = IndexOperation {
        key: IndexOpKey::new(3, "tag3", "field3"),
        index_type: IndexType::Fulltext,
        change_type: ChangeType::Update,
        id: "doc3".to_string(),
        data: Some(IndexData::Fulltext("updated content".to_string())),
    };
    let key = update_op.extract_index_key();
    assert_eq!(key, (3, "tag3".to_string(), "field3".to_string()));
}

/// TC-223: Fulltext error types
#[cfg(feature = "fulltext-search")]
#[test]
fn test_fulltext_error_types() {
    use graphdb::sync::coordinator::FulltextError;

    let not_found = FulltextError::IndexNotFound("my_index".to_string());
    assert!(matches!(not_found, FulltextError::IndexNotFound(_)));

    let timeout = FulltextError::Timeout;
    assert!(matches!(timeout, FulltextError::Timeout));

    let internal = FulltextError::Internal("something went wrong".to_string());
    assert!(matches!(internal, FulltextError::Internal(_)));
}

/// TC-225: Vector error types
#[test]
fn test_vector_error_types() {
    use graphdb::sync::vector_error::VectorError;

    let not_found = VectorError::IndexNotFound("index".to_string());
    assert!(matches!(not_found, VectorError::IndexNotFound(_)));

    let timeout = VectorError::Timeout;
    assert!(matches!(timeout, VectorError::Timeout));

    let dim_mismatch = VectorError::DimensionMismatch {
        expected: 128,
        actual: 64,
    };
    assert!(matches!(
        dim_mismatch,
        VectorError::DimensionMismatch { .. }
    ));

    let conn_failed = VectorError::ConnectionFailed("refused".to_string());
    assert!(matches!(conn_failed, VectorError::ConnectionFailed(_)));
}

/// TC-226: Coordinator error types
#[cfg(feature = "fulltext-search")]
#[test]
fn test_coordinator_error_types() {
    use graphdb::sync::coordinator::{CoordinatorError, FulltextError};
    use graphdb::sync::vector_error::{VectorCoordinatorError, VectorError};

    let fulltext_coord_err = CoordinatorError::Fulltext(FulltextError::Timeout);
    assert!(matches!(fulltext_coord_err, CoordinatorError::Fulltext(_)));

    let vec_coord_err = VectorCoordinatorError::Vector(VectorError::Timeout);
    assert!(matches!(vec_coord_err, VectorCoordinatorError::Vector(_)));
}

/// TC-227: Dead letter entry creation
#[test]
fn test_dead_letter_entry() {
    use graphdb::sync::dead_letter_queue::{
        DeadLetterEntry, DeadLetterQueue, DeadLetterQueueConfig,
    };
    use graphdb::sync::types::{ChangeType, IndexData, IndexType};
    use graphdb::sync::{IndexOpKey, IndexOperation};

    let dlq = DeadLetterQueue::new(DeadLetterQueueConfig::default());

    let entry = DeadLetterEntry::new(
        IndexOperation {
            key: IndexOpKey::new(1, "Document", "title"),
            index_type: IndexType::Fulltext,
            change_type: ChangeType::Insert,
            id: "test_id".to_string(),
            data: Some(IndexData::Fulltext("Test".to_string())),
        },
        "Test failure".to_string(),
        3,
    );

    dlq.add(entry);

    let entries = dlq.get_all();
    assert_eq!(entries.len(), 1);
}

/// TC-228: ChangeType conversion
#[cfg(feature = "qdrant")]
#[test]
fn test_change_type_conversion() {
    use graphdb::sync::coordinator::ChangeType;
    use graphdb::sync::VectorChangeType;

    let vt: VectorChangeType = ChangeType::Insert.into();
    assert!(matches!(vt, VectorChangeType::Insert));

    let vt: VectorChangeType = ChangeType::Update.into();
    assert!(matches!(vt, VectorChangeType::Insert));

    let vt: VectorChangeType = ChangeType::Delete.into();
    assert!(matches!(vt, VectorChangeType::Delete));
}

/// TC-229: Vector index location
#[cfg(feature = "qdrant")]
#[test]
fn test_vector_index_location_tc229() {
    use graphdb::sync::VectorIndexLocation;

    let loc = VectorIndexLocation::new(1, "tag", "field");
    assert_eq!(loc.to_collection_name(), format!("space_{}", 1));
    assert_eq!(loc.group_id(), format!("{}_{}", "tag", "field"));
}
