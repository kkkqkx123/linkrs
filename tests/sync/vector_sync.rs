//! Vector Sync Basic Integration Tests (TC-200 ~ TC-210)
//!
//! Tests for basic vector synchronization at the sync layer.
//! These tests use VectorManager::disabled() to avoid needing a real qdrant instance.

use std::sync::Arc;

use graphdb::core::Value;
use graphdb::sync::{
    PendingVectorUpdate, VectorChangeContext, VectorChangeType, VectorIndexLocation,
    VectorPointData, VectorSyncCoordinator, VectorTransactionBuffer, VectorTransactionBufferConfig,
};
use graphdb::transaction::types::TransactionId;
use vector_client::{VectorClientConfig, VectorManager};

/// TC-200: Vector change type variants
#[tokio::test]
async fn test_vector_change_type_variants() {
    let vector_manager = Arc::new(
        VectorManager::new(VectorClientConfig::disabled())
            .await
            .unwrap(),
    );
    let handle = tokio::runtime::Handle::current();
    let coordinator = VectorSyncCoordinator::with_transaction_buffer(
        vector_manager,
        None,
        VectorTransactionBufferConfig::default(),
        handle,
    );

    let txn_id = TransactionId::from(1u64);

    // Test Insert change type
    let insert_ctx = VectorChangeContext::new(
        1,
        "test",
        "field",
        VectorChangeType::Insert,
        VectorPointData {
            id: "insert_id".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            payload: std::collections::HashMap::new(),
        },
    );
    assert!(matches!(insert_ctx.change_type, VectorChangeType::Insert));

    // Test Delete change type
    let delete_ctx = VectorChangeContext::new(
        1,
        "test",
        "field",
        VectorChangeType::Delete,
        VectorPointData {
            id: "delete_id".to_string(),
            vector: vec![],
            payload: std::collections::HashMap::new(),
        },
    );
    assert!(matches!(delete_ctx.change_type, VectorChangeType::Delete));

    // Buffer both types
    coordinator
        .buffer_vector_change(txn_id, insert_ctx)
        .unwrap();
    coordinator
        .buffer_vector_change(txn_id, delete_ctx)
        .unwrap();

    if let Some(buffer) = coordinator.transaction_buffer() {
        assert!(buffer.has_pending_updates(txn_id));
        let updates = buffer.take_updates(txn_id);
        assert_eq!(updates.len(), 2);
    }
}

/// TC-201: Pending update with change types
#[tokio::test]
async fn test_pending_update_with_change_types() {
    let buffer = VectorTransactionBuffer::new(VectorTransactionBufferConfig::default());
    let txn_id = TransactionId::from(1u64);

    let insert_ctx = VectorChangeContext::new(
        1,
        "docs",
        "embedding",
        VectorChangeType::Insert,
        VectorPointData {
            id: "doc_1".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            payload: std::collections::HashMap::new(),
        },
    );
    let insert_update = PendingVectorUpdate::new(txn_id, 0, insert_ctx);
    assert!(matches!(
        insert_update.context.change_type,
        VectorChangeType::Insert
    ));
    buffer.add_update(txn_id, insert_update).unwrap();

    let delete_ctx = VectorChangeContext::new(
        1,
        "docs",
        "embedding",
        VectorChangeType::Delete,
        VectorPointData {
            id: "doc_1".to_string(),
            vector: vec![],
            payload: std::collections::HashMap::new(),
        },
    );
    let delete_update = PendingVectorUpdate::new(txn_id, 0, delete_ctx);
    assert!(matches!(
        delete_update.context.change_type,
        VectorChangeType::Delete
    ));
    buffer.add_update(txn_id, delete_update).unwrap();

    assert!(buffer.has_pending_updates(txn_id));
    let updates = buffer.take_updates(txn_id);
    assert_eq!(updates.len(), 2);
}

/// TC-202: Vector index location
#[tokio::test]
async fn test_vector_index_location() {
    let location = VectorIndexLocation::new(5, "Products", "image_embedding");
    assert_eq!(location.space_id, 5);
    assert_eq!(location.tag_name, "Products");
    assert_eq!(location.field_name, "image_embedding");

    let location2 = VectorIndexLocation::new(10, "Users", "face_embedding");
    assert_eq!(location2.space_id, 10);
    assert_eq!(location2.tag_name, "Users");
    assert_eq!(location2.field_name, "face_embedding");
}

/// TC-203: Point data with payload
#[tokio::test]
async fn test_point_data_with_payload() {
    let mut payload: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    payload.insert(
        "category".to_string(),
        Value::String("electronics".to_string()),
    );
    payload.insert("price".to_string(), Value::String("99.99".to_string()));

    let point = VectorPointData {
        id: "point_1".to_string(),
        vector: vec![1.5, 2.5, 3.5, 4.5],
        payload,
    };

    assert_eq!(point.id, "point_1");
    assert_eq!(point.vector.len(), 4);
    assert_eq!(
        point.payload.get("category").unwrap(),
        &Value::String("electronics".to_string())
    );
    assert_eq!(
        point.payload.get("price").unwrap(),
        &Value::String("99.99".to_string())
    );
}

/// TC-204: Coordinator commit with disabled engine returns expected error
#[tokio::test]
async fn test_coordinator_commit_disabled_engine() {
    let vector_manager = Arc::new(
        VectorManager::new(VectorClientConfig::disabled())
            .await
            .unwrap(),
    );
    let handle = tokio::runtime::Handle::current();
    let coordinator = VectorSyncCoordinator::with_transaction_buffer(
        vector_manager,
        None,
        VectorTransactionBufferConfig::default(),
        handle,
    );

    let txn_id = TransactionId::from(1u64);

    let ctx = VectorChangeContext::new(
        1,
        "docs",
        "embedding",
        VectorChangeType::Insert,
        VectorPointData {
            id: "doc_0".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            payload: std::collections::HashMap::new(),
        },
    );
    coordinator.buffer_vector_change(txn_id, ctx).unwrap();

    // Commit should fail because engine is disabled
    let result = coordinator.commit_transaction(txn_id).await;
    assert!(result.is_err());

    // Buffer should still have updates (peek-first preserves them on failure)
    if let Some(buffer) = coordinator.transaction_buffer() {
        assert!(buffer.has_pending_updates(txn_id));
    }

    // Rollback clears the buffer
    coordinator.rollback_transaction(txn_id).await;
    if let Some(buffer) = coordinator.transaction_buffer() {
        assert!(!buffer.has_pending_updates(txn_id));
    }
}

/// TC-205: Coordinator without transaction buffer
#[tokio::test]
async fn test_coordinator_without_buffer() {
    let vector_manager = Arc::new(
        VectorManager::new(VectorClientConfig::disabled())
            .await
            .unwrap(),
    );
    let handle = tokio::runtime::Handle::current();
    let coordinator = VectorSyncCoordinator::new(vector_manager, None, handle);

    assert!(coordinator.transaction_buffer().is_none());
}

/// TC-206: Different dimension vectors in buffer
#[tokio::test]
async fn test_different_dimensions_in_buffer() {
    let buffer = VectorTransactionBuffer::new(VectorTransactionBufferConfig::default());
    let txn_id = TransactionId::from(1u64);

    let ctx_3d = VectorChangeContext::new(
        1,
        "doc3d",
        "emb",
        VectorChangeType::Insert,
        VectorPointData {
            id: "doc_3d".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            payload: std::collections::HashMap::new(),
        },
    );
    buffer
        .add_update(txn_id, PendingVectorUpdate::new(txn_id, 0, ctx_3d))
        .unwrap();

    let ctx_128d = VectorChangeContext::new(
        2,
        "doc128d",
        "emb",
        VectorChangeType::Insert,
        VectorPointData {
            id: "doc_128d".to_string(),
            vector: vec![0.5; 128],
            payload: std::collections::HashMap::new(),
        },
    );
    buffer
        .add_update(txn_id, PendingVectorUpdate::new(txn_id, 0, ctx_128d))
        .unwrap();

    let updates = buffer.take_updates(txn_id);
    assert_eq!(updates.len(), 2);
}

/// TC-207: Rollback clears buffer
#[tokio::test]
async fn test_rollback_clears_buffer() {
    let vector_manager = Arc::new(
        VectorManager::new(VectorClientConfig::disabled())
            .await
            .unwrap(),
    );
    let handle = tokio::runtime::Handle::current();
    let coordinator = VectorSyncCoordinator::with_transaction_buffer(
        vector_manager,
        None,
        VectorTransactionBufferConfig::default(),
        handle,
    );

    // buffer + rollback
    let txn_1 = TransactionId::from(1u64);
    let ctx_1 = VectorChangeContext::new(
        1,
        "docs",
        "emb",
        VectorChangeType::Insert,
        VectorPointData {
            id: "doc_1".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            payload: std::collections::HashMap::new(),
        },
    );
    coordinator.buffer_vector_change(txn_1, ctx_1).unwrap();
    coordinator.rollback_transaction(txn_1).await;
    if let Some(buffer) = coordinator.transaction_buffer() {
        assert!(!buffer.has_pending_updates(txn_1));
    }

    // buffer + rollback again
    let txn_2 = TransactionId::from(2u64);
    let ctx_2 = VectorChangeContext::new(
        1,
        "docs",
        "emb",
        VectorChangeType::Insert,
        VectorPointData {
            id: "doc_2".to_string(),
            vector: vec![4.0, 5.0, 6.0],
            payload: std::collections::HashMap::new(),
        },
    );
    coordinator.buffer_vector_change(txn_2, ctx_2).unwrap();
    coordinator.rollback_transaction(txn_2).await;
    if let Some(buffer) = coordinator.transaction_buffer() {
        assert!(!buffer.has_pending_updates(txn_2));
    }
}

/// TC-208: Invalid transaction handling
#[tokio::test]
async fn test_invalid_transaction_handling() {
    let buffer = VectorTransactionBuffer::new(VectorTransactionBufferConfig::default());

    let non_existent = TransactionId::from(999u64);
    let updates = buffer.take_updates(non_existent);
    assert!(
        updates.is_empty(),
        "Should return empty for non-existent txn"
    );

    assert!(!buffer.has_pending_updates(non_existent));
}

/// TC-209: Empty buffer operations
#[tokio::test]
async fn test_empty_buffer_operations() {
    let buffer = VectorTransactionBuffer::new(VectorTransactionBufferConfig::default());

    let txn_id = TransactionId::from(1u64);
    buffer.cleanup(txn_id);

    assert!(!buffer.has_pending_updates(txn_id));
}

/// TC-210: Multiple locations in single transaction
#[tokio::test]
async fn test_multiple_locations_in_transaction() {
    let handle = tokio::runtime::Handle::current();
    let coordinator = VectorSyncCoordinator::with_transaction_buffer(
        Arc::new(
            VectorManager::new(VectorClientConfig::disabled())
                .await
                .unwrap(),
        ),
        None,
        VectorTransactionBufferConfig::default(),
        handle,
    );

    let txn_id = TransactionId::from(1u64);

    let locations: Vec<(u64, &str, &str)> = vec![
        (1, "users", "face_embedding"),
        (1, "products", "image_embedding"),
        (2, "articles", "text_embedding"),
    ];

    for (i, (space, tag, field)) in locations.iter().enumerate() {
        let ctx = VectorChangeContext::new(
            *space,
            *tag,
            *field,
            VectorChangeType::Insert,
            VectorPointData {
                id: format!("item_{}", i),
                vector: vec![i as f32; 4],
                payload: std::collections::HashMap::new(),
            },
        );
        coordinator.buffer_vector_change(txn_id, ctx).unwrap();
    }

    if let Some(buffer) = coordinator.transaction_buffer() {
        assert!(buffer.has_pending_updates(txn_id));
        let updates = buffer.take_updates(txn_id);
        assert_eq!(updates.len(), 3);
    }
}
