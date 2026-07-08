//! Vector Transaction Buffer Integration Tests

use std::sync::Arc;

use graphdb::sync::{
    PendingVectorUpdate, VectorChangeContext, VectorChangeType, VectorIndexLocation,
    VectorPointData, VectorSyncCoordinator, VectorTransactionBuffer, VectorTransactionBufferConfig,
};
use graphdb::transaction::types::TransactionId;
use vector_client::{VectorClientConfig, VectorManager};

#[tokio::test]
async fn test_vector_transaction_buffer_basic() {
    let config = VectorTransactionBufferConfig::default();
    let buffer = VectorTransactionBuffer::new(config);

    let txn_id = TransactionId::from(1u64);

    // Create test update
    let _location = VectorIndexLocation::new(1, "test", "vector_field");
    let context = VectorChangeContext::new(
        1,
        "test",
        "vector_field",
        VectorChangeType::Insert,
        VectorPointData {
            id: "test_id".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            payload: std::collections::HashMap::new(),
        },
    );

    let update = PendingVectorUpdate::new(txn_id, 0, context);

    // Add update to buffer
    buffer.add_update(txn_id, update).unwrap();

    // Verify update is buffered
    assert!(buffer.has_pending_updates(txn_id));

    // Take updates
    let updates = buffer.take_updates(txn_id);
    assert_eq!(updates.len(), 1);

    // Verify buffer is cleared
    assert!(!buffer.has_pending_updates(txn_id));
}

#[tokio::test]
async fn test_vector_transaction_buffer_cleanup() {
    let config = VectorTransactionBufferConfig::default();
    let buffer = VectorTransactionBuffer::new(config);

    let txn_id = TransactionId::from(1u64);

    // Add multiple updates
    for i in 0..3 {
        let context = VectorChangeContext::new(
            1,
            "test",
            "vector_field",
            VectorChangeType::Insert,
            VectorPointData {
                id: format!("test_id_{}", i),
                vector: vec![1.0, 2.0, 3.0],
                payload: std::collections::HashMap::new(),
            },
        );

        let update = PendingVectorUpdate::new(txn_id, 0, context);
        buffer.add_update(txn_id, update).unwrap();
    }

    assert!(buffer.has_pending_updates(txn_id));

    // Cleanup
    buffer.cleanup(txn_id);

    // Verify buffer is cleared
    assert!(!buffer.has_pending_updates(txn_id));
}

#[tokio::test]
async fn test_vector_sync_coordinator_with_buffer() {
    // Create a mock vector manager (using disabled config)
    let vector_manager = Arc::new(
        VectorManager::new(VectorClientConfig::disabled())
            .await
            .unwrap(),
    );

    let handle = tokio::runtime::Handle::current();
    // Create coordinator with transaction buffer
    let coordinator = VectorSyncCoordinator::with_transaction_buffer(
        vector_manager,
        None,
        VectorTransactionBufferConfig::default(),
        handle,
    );

    let txn_id = TransactionId::from(1u64);

    // Create test context
    let context = VectorChangeContext::new(
        1,
        "test",
        "vector_field",
        VectorChangeType::Insert,
        VectorPointData {
            id: "test_id".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            payload: std::collections::HashMap::new(),
        },
    );

    // Buffer the update
    coordinator
        .buffer_vector_change(txn_id, context.clone())
        .unwrap();

    // Verify buffer has pending updates
    if let Some(buffer) = coordinator.transaction_buffer() {
        assert!(buffer.has_pending_updates(txn_id));
    } else {
        panic!("Transaction buffer not initialized");
    }

    // Commit transaction — should fail with disabled engine
    let commit_result = coordinator.commit_transaction(txn_id).await;
    assert!(
        commit_result.is_err(),
        "commit_transaction should fail with disabled vector engine"
    );

    // Buffer should still have updates (peek-first preserves buffer on failure)
    if let Some(buffer) = coordinator.transaction_buffer() {
        assert!(
            buffer.has_pending_updates(txn_id),
            "Buffer should preserve updates when commit fails"
        );
    }

    // Rollback clears the buffer
    coordinator.rollback_transaction(txn_id).await;
    if let Some(buffer) = coordinator.transaction_buffer() {
        assert!(
            !buffer.has_pending_updates(txn_id),
            "Buffer should be cleared after rollback"
        );
    }
}

#[tokio::test]
async fn test_vector_sync_coordinator_rollback() {
    // Create a mock vector manager
    let vector_manager = Arc::new(
        VectorManager::new(VectorClientConfig::disabled())
            .await
            .unwrap(),
    );

    let handle = tokio::runtime::Handle::current();
    // Create coordinator with transaction buffer
    let coordinator = VectorSyncCoordinator::with_transaction_buffer(
        vector_manager,
        None,
        VectorTransactionBufferConfig::default(),
        handle,
    );

    let txn_id = TransactionId::from(1u64);

    // Buffer multiple updates
    for i in 0..3 {
        let context = VectorChangeContext::new(
            1,
            "test",
            "vector_field",
            VectorChangeType::Insert,
            VectorPointData {
                id: format!("test_id_{}", i),
                vector: vec![1.0, 2.0, 3.0],
                payload: std::collections::HashMap::new(),
            },
        );

        coordinator.buffer_vector_change(txn_id, context).unwrap();
    }

    // Verify updates are buffered
    if let Some(buffer) = coordinator.transaction_buffer() {
        assert!(buffer.has_pending_updates(txn_id));
        assert_eq!(buffer.take_updates(txn_id).len(), 3);
    }

    // Rollback transaction
    coordinator.rollback_transaction(txn_id).await;

    // Verify buffer is cleared
    if let Some(buffer) = coordinator.transaction_buffer() {
        assert!(!buffer.has_pending_updates(txn_id));
    }
}

#[tokio::test]
async fn test_vector_transaction_buffer_size_limit() {
    let config = VectorTransactionBufferConfig {
        max_buffer_size: 2,
        ..Default::default()
    };
    let buffer = VectorTransactionBuffer::new(config);

    let txn_id = TransactionId::from(1u64);

    // Add 2 updates (at limit)
    for i in 0..2 {
        let context = VectorChangeContext::new(
            1,
            "test",
            "vector_field",
            VectorChangeType::Insert,
            VectorPointData {
                id: format!("test_id_{}", i),
                vector: vec![1.0, 2.0, 3.0],
                payload: std::collections::HashMap::new(),
            },
        );

        let update = PendingVectorUpdate::new(txn_id, 0, context);
        buffer.add_update(txn_id, update).unwrap();
    }

    // Third update should fail
    let context = VectorChangeContext::new(
        1,
        "test",
        "vector_field",
        VectorChangeType::Insert,
        VectorPointData {
            id: "test_id_3".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            payload: std::collections::HashMap::new(),
        },
    );

    let update = PendingVectorUpdate::new(txn_id, 0, context);
    let result = buffer.add_update(txn_id, update);

    assert!(result.is_err());
}

#[tokio::test]
async fn test_vector_transaction_multiple_transactions() {
    let config = VectorTransactionBufferConfig::default();
    let buffer = VectorTransactionBuffer::new(config);

    // Create updates for multiple transactions
    for txn_num in 0..3 {
        let txn_id = TransactionId::from(txn_num as u64);

        let context = VectorChangeContext::new(
            1,
            "test",
            "vector_field",
            VectorChangeType::Insert,
            VectorPointData {
                id: format!("test_id_{}", txn_num),
                vector: vec![1.0, 2.0, 3.0],
                payload: std::collections::HashMap::new(),
            },
        );

        let update = PendingVectorUpdate::new(txn_id, 0, context);
        buffer.add_update(txn_id, update).unwrap();
    }

    // Verify all transactions have pending updates
    for txn_num in 0..3 {
        let txn_id = TransactionId::from(txn_num as u64);
        assert!(buffer.has_pending_updates(txn_id));
    }

    // Take updates for specific transaction
    let updates = buffer.take_updates(TransactionId::from(1u64));
    assert_eq!(updates.len(), 1);

    // Verify transaction 1 no longer has pending updates
    assert!(!buffer.has_pending_updates(TransactionId::from(1u64)));
    // Verify transactions 0 and 2 still have pending updates
    assert!(buffer.has_pending_updates(TransactionId::from(0u64)));
    assert!(buffer.has_pending_updates(TransactionId::from(2u64)));
}

#[tokio::test]
async fn test_vector_buffer_cleanup() {
    let config = VectorTransactionBufferConfig::default();
    let buffer = VectorTransactionBuffer::new(config);

    // Add updates for multiple transactions
    for txn_num in 0..2 {
        let txn_id = TransactionId::from(txn_num as u64);

        for i in 0..3 {
            let context = VectorChangeContext::new(
                1,
                "test",
                "vector_field",
                VectorChangeType::Insert,
                VectorPointData {
                    id: format!("test_id_{}_{}", txn_num, i),
                    vector: vec![1.0, 2.0, 3.0],
                    payload: std::collections::HashMap::new(),
                },
            );

            let update = PendingVectorUpdate::new(txn_id, 0, context);
            buffer.add_update(txn_id, update).unwrap();
        }
    }

    // Verify transactions have pending updates
    assert!(buffer.has_pending_updates(TransactionId::from(0u64)));
    assert!(buffer.has_pending_updates(TransactionId::from(1u64)));

    // Cleanup transaction 0
    buffer.cleanup(TransactionId::from(0u64));
    assert!(!buffer.has_pending_updates(TransactionId::from(0u64)));
    assert!(buffer.has_pending_updates(TransactionId::from(1u64)));

    // Take updates for transaction 1
    let updates = buffer.take_updates(TransactionId::from(1u64));
    assert_eq!(updates.len(), 3);
    assert!(!buffer.has_pending_updates(TransactionId::from(1u64)));
}
