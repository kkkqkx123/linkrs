//! Fulltext Integration Tests - Transaction Support
//!
//! Test scope:
//! - Transaction buffer operations
//! - Transaction commit with batch optimization
//! - Transaction rollback
//! - Prepare phase validation
//! - Multi-operation transactions
//!
//! Test cases: TC-FT-TXN-001 ~ TC-FT-TXN-010

use super::common::FulltextTestContext;
use graphdb_search::search::EngineType;
use graphdb_sync::sync::batch::BatchConfig;
use graphdb_sync::sync::coordinator::{ChangeContext, ChangeType, SyncCoordinator};
use graphdb_sync::sync::manager::SyncManager;
use std::sync::Arc;

#[allow(dead_code)]
struct TransactionTestContext {
    coordinator: Arc<SyncCoordinator>,
    sync_manager: Arc<SyncManager>,
    fulltext_ctx: FulltextTestContext,
}

impl TransactionTestContext {
    async fn new() -> Self {
        let fulltext_ctx = FulltextTestContext::new();
        let batch_config = BatchConfig::default();
        let coordinator = Arc::new(SyncCoordinator::new(
            fulltext_ctx.manager.clone(),
            batch_config,
        ));
        let sync_manager = Arc::new(SyncManager::new(coordinator.clone()));

        sync_manager
            .start()
            .await
            .expect("Failed to start sync manager");

        Self {
            coordinator,
            sync_manager,
            fulltext_ctx,
        }
    }

    #[allow(dead_code)]
    async fn shutdown(&self) {
        self.sync_manager.stop().await;
    }
}

impl Drop for TransactionTestContext {
    fn drop(&mut self) {
        // Note: This is a best-effort cleanup since we can't block in Drop
        // The sync_manager will be dropped automatically
    }
}

fn generate_txn_id(id: u64) -> u64 {
    id
}

/// TC-FT-TXN-001: Basic Transaction Buffer Operation
#[tokio::test]
async fn test_transaction_buffer_operation() {
    let ctx = TransactionTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let txn_id = generate_txn_id(1);

    let change_ctx = ChangeContext::new_fulltext(
        1,
        "Article",
        "content",
        ChangeType::Insert,
        "1",
        "Buffered content",
    );

    let result = ctx.coordinator.buffer_operation(txn_id, change_ctx);
    assert!(result.is_ok(), "Buffer operation should succeed");

    let count = ctx.coordinator.transaction_buffer_count(txn_id);
    assert_eq!(count, 1, "Should have 1 buffered operation");
}

/// TC-FT-TXN-002: Multiple Operations in Single Transaction
#[tokio::test]
async fn test_transaction_multiple_operations() {
    let ctx = TransactionTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let txn_id = generate_txn_id(2);

    for i in 1..=5 {
        let change_ctx = ChangeContext::new_fulltext(
            1,
            "Article",
            "content",
            ChangeType::Insert,
            format!("doc_{}", i),
            format!("Content for document {}", i),
        );
        ctx.coordinator
            .buffer_operation(txn_id, change_ctx)
            .expect("Buffer operation should succeed");
    }

    let count = ctx.coordinator.transaction_buffer_count(txn_id);
    assert_eq!(count, 5, "Should have 5 buffered operations");
}

/// TC-FT-TXN-003: Transaction Commit
#[tokio::test]
async fn test_transaction_commit() {
    let ctx = TransactionTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let txn_id = generate_txn_id(3);

    for i in 1..=3 {
        let change_ctx = ChangeContext::new_fulltext(
            1,
            "Article",
            "content",
            ChangeType::Insert,
            format!("doc_{}", i),
            format!("Transaction commit test {}", i),
        );
        ctx.coordinator
            .buffer_operation(txn_id, change_ctx)
            .expect("Buffer operation should succeed");
    }

    ctx.coordinator
        .prepare_transaction(txn_id)
        .await
        .expect("Prepare should succeed");

    ctx.coordinator
        .commit_transaction(txn_id)
        .await
        .expect("Commit should succeed");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit all");

    let results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "Transaction", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(results.len(), 3, "Should find all 3 committed documents");

    let count_after = ctx.coordinator.transaction_buffer_count(txn_id);
    assert_eq!(count_after, 0, "Buffer should be cleared after commit");
}

/// TC-FT-TXN-004: Transaction Rollback
#[tokio::test]
async fn test_transaction_rollback() {
    let ctx = TransactionTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let txn_id = generate_txn_id(4);

    for i in 1..=3 {
        let change_ctx = ChangeContext::new_fulltext(
            1,
            "Article",
            "content",
            ChangeType::Insert,
            format!("doc_{}", i),
            format!("Rollback test content {}", i),
        );
        ctx.coordinator
            .buffer_operation(txn_id, change_ctx)
            .expect("Buffer operation should succeed");
    }

    assert_eq!(
        ctx.coordinator.transaction_buffer_count(txn_id),
        3,
        "Should have 3 buffered operations before rollback"
    );

    ctx.coordinator
        .rollback_transaction(txn_id)
        .expect("Rollback should succeed");

    let count_after = ctx.coordinator.transaction_buffer_count(txn_id);
    assert_eq!(count_after, 0, "Buffer should be cleared after rollback");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit all");

    let results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "Rollback", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(results.len(), 0, "Should find no documents after rollback");
}

/// TC-FT-TXN-005: Prepare Phase Validation
#[tokio::test]
async fn test_transaction_prepare_phase() {
    let ctx = TransactionTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let txn_id = generate_txn_id(5);

    let change_ctx = ChangeContext::new_fulltext(
        1,
        "Article",
        "content",
        ChangeType::Insert,
        "doc_prepare",
        "Prepare phase test",
    );
    ctx.coordinator
        .buffer_operation(txn_id, change_ctx)
        .expect("Buffer operation should succeed");

    let prepare_result = ctx.coordinator.prepare_transaction(txn_id).await;
    assert!(prepare_result.is_ok(), "Prepare should succeed");

    ctx.coordinator
        .commit_transaction(txn_id)
        .await
        .expect("Commit should succeed");
}

/// TC-FT-TXN-006: Multiple Concurrent Transactions
#[tokio::test]
async fn test_concurrent_transactions() {
    let ctx = TransactionTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let txn1 = generate_txn_id(101);
    let txn2 = generate_txn_id(102);

    for i in 1..=3 {
        let change_ctx = ChangeContext::new_fulltext(
            1,
            "Article",
            "content",
            ChangeType::Insert,
            format!("txn1_doc_{}", i),
            format!("Transaction 1 content {}", i),
        );
        ctx.coordinator
            .buffer_operation(txn1, change_ctx)
            .expect("Buffer operation should succeed");
    }

    for i in 1..=2 {
        let change_ctx = ChangeContext::new_fulltext(
            1,
            "Article",
            "content",
            ChangeType::Insert,
            format!("txn2_doc_{}", i),
            format!("Transaction 2 content {}", i),
        );
        ctx.coordinator
            .buffer_operation(txn2, change_ctx)
            .expect("Buffer operation should succeed");
    }

    assert_eq!(ctx.coordinator.transaction_buffer_count(txn1), 3);
    assert_eq!(ctx.coordinator.transaction_buffer_count(txn2), 2);

    ctx.coordinator
        .commit_transaction(txn1)
        .await
        .expect("Commit txn1 should succeed");

    ctx.coordinator
        .rollback_transaction(txn2)
        .expect("Rollback txn2 should succeed");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    let results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "content", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(
        results.len(),
        3,
        "Should find 3 documents from committed txn1"
    );
}

/// TC-FT-TXN-007: Transaction with Update Operations
#[tokio::test]
async fn test_transaction_update_operations() {
    let ctx = TransactionTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let setup_txn = generate_txn_id(70);

    let insert_ctx = ChangeContext::new_fulltext(
        1,
        "Article",
        "content",
        ChangeType::Insert,
        "doc_update",
        "Original content",
    );
    ctx.coordinator
        .buffer_operation(setup_txn, insert_ctx)
        .expect("Buffer operation should succeed");

    ctx.coordinator
        .commit_transaction(setup_txn)
        .await
        .expect("Commit should succeed");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    let txn_id = generate_txn_id(7);

    let delete_ctx = ChangeContext::new_fulltext(
        1,
        "Article",
        "content",
        ChangeType::Delete,
        "doc_update",
        "Original content",
    );
    ctx.coordinator
        .buffer_operation(txn_id, delete_ctx)
        .expect("Buffer delete should succeed");

    let insert_ctx = ChangeContext::new_fulltext(
        1,
        "Article",
        "content",
        ChangeType::Insert,
        "doc_update",
        "Updated content in transaction",
    );
    ctx.coordinator
        .buffer_operation(txn_id, insert_ctx)
        .expect("Buffer insert should succeed");

    ctx.coordinator
        .commit_transaction(txn_id)
        .await
        .expect("Commit should succeed");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    let old_results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "Original", 10)
        .await
        .expect("Search should succeed");
    assert_eq!(old_results.len(), 0, "Should not find old content");

    let new_results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "Updated", 10)
        .await
        .expect("Search should succeed");
    assert_eq!(new_results.len(), 1, "Should find updated content");
}

/// TC-FT-TXN-008: Transaction with Mixed Change Types
#[tokio::test]
async fn test_transaction_mixed_change_types() {
    let ctx = TransactionTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let setup_txn = generate_txn_id(80);

    for i in 1..=3 {
        let change_ctx = ChangeContext::new_fulltext(
            1,
            "Article",
            "content",
            ChangeType::Insert,
            format!("initial_doc_{}", i),
            format!("Initial content {}", i),
        );
        ctx.coordinator
            .buffer_operation(setup_txn, change_ctx)
            .expect("Buffer operation should succeed");
    }

    ctx.coordinator
        .commit_transaction(setup_txn)
        .await
        .expect("Commit should succeed");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    let txn_id = generate_txn_id(8);

    let insert_ctx = ChangeContext::new_fulltext(
        1,
        "Article",
        "content",
        ChangeType::Insert,
        "new_doc",
        "New document in transaction",
    );
    ctx.coordinator
        .buffer_operation(txn_id, insert_ctx)
        .expect("Buffer insert should succeed");

    let delete_ctx = ChangeContext::new_fulltext(
        1,
        "Article",
        "content",
        ChangeType::Delete,
        "initial_doc_1",
        "Initial content 1",
    );
    ctx.coordinator
        .buffer_operation(txn_id, delete_ctx)
        .expect("Buffer delete should succeed");

    let update_ctx = ChangeContext::new_fulltext(
        1,
        "Article",
        "content",
        ChangeType::Update,
        "initial_doc_2",
        "Updated content for doc 2",
    );
    ctx.coordinator
        .buffer_operation(txn_id, update_ctx)
        .expect("Buffer update should succeed");

    ctx.coordinator
        .commit_transaction(txn_id)
        .await
        .expect("Commit should succeed");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    let all_results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "content", 10)
        .await
        .expect("Search should succeed");

    assert!(
        all_results.len() >= 2,
        "Should have at least 2 documents after mixed operations"
    );
}

/// TC-FT-TXN-010: Empty Transaction Commit
#[tokio::test]
async fn test_empty_transaction_commit() {
    let ctx = TransactionTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let txn_id = generate_txn_id(10);

    let prepare_result = ctx.coordinator.prepare_transaction(txn_id).await;
    assert!(
        prepare_result.is_ok(),
        "Prepare empty transaction should succeed"
    );

    let commit_result = ctx.coordinator.commit_transaction(txn_id).await;
    assert!(
        commit_result.is_ok(),
        "Commit empty transaction should succeed"
    );
}
