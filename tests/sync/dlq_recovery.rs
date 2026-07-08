//! Sync Module Dead Letter Queue Recovery Tests (TC-310 ~ TC-315)
//!
//! Tests for DLQ recovery, retry, and the RecoveryResult struct

use crate::common::sync_helpers::SyncTestHarness;
use graphdb::core::types::DataType;
use graphdb::sync::dead_letter_queue::{DeadLetterEntry, DeadLetterQueue, DeadLetterQueueConfig};
use graphdb::sync::types::{ChangeType, IndexData, IndexOpKey, IndexOperation, IndexType};

/// TC-310: Dead letter queue recovery flow
///
/// Verifies that entries can be added to DLQ and the recover_dead_letter
/// method processes them correctly.
#[test]
fn test_dlq_recovery_flow() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Person",
            vec![("name", DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    let dlq = harness
        .sync_manager
        .sync_coordinator()
        .dead_letter_queue()
        .clone();

    // Add entries to DLQ directly (simulating failed sync operations)
    for i in 0..3 {
        let entry = DeadLetterEntry::new(
            IndexOperation {
                key: IndexOpKey::new(1, "Person".to_string(), "name".to_string()),
                index_type: IndexType::Fulltext,
                change_type: ChangeType::Insert,
                id: format!("doc_{}", i),
                data: Some(IndexData::Fulltext(format!("RecoveryTest{}", i))),
            },
            "Simulated failure".to_string(),
            3,
        );
        dlq.add(entry);
    }

    assert_eq!(dlq.len(), 3, "DLQ should have 3 entries");

    // Try recovery through the coordinator
    let rt = &harness.rt;
    let result = rt
        .block_on(async { harness.sync_coordinator.recover_dead_letter().await })
        .expect("Recovery should complete");

    // Recovery should have attempted all entries
    assert_eq!(
        result.total(),
        3,
        "Should have attempted recovery for 3 entries"
    );
}

/// TC-311: Dead letter queue retry flow
///
/// Verifies that retry_dead_letter processes unrecovered entries correctly.
#[test]
fn test_dlq_retry_flow() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Person",
            vec![("name", DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    let dlq = harness
        .sync_manager
        .sync_coordinator()
        .dead_letter_queue()
        .clone();

    // Add entries with mixed types
    dlq.add(DeadLetterEntry::new(
        IndexOperation {
            key: IndexOpKey::new(1, "Person", "name"),
            index_type: IndexType::Fulltext,
            change_type: ChangeType::Insert,
            id: "retry_1".to_string(),
            data: Some(IndexData::Fulltext("RetryTest1".to_string())),
        },
        "Insert failed".to_string(),
        2,
    ));
    dlq.add(DeadLetterEntry::new(
        IndexOperation {
            key: IndexOpKey::new(1, "Person", "name"),
            index_type: IndexType::Fulltext,
            change_type: ChangeType::Delete,
            id: "retry_2".to_string(),
            data: None,
        },
        "Delete failed".to_string(),
        2,
    ));

    assert_eq!(dlq.len(), 2, "DLQ should have 2 entries");

    // Mark one as recovered to test filtering
    dlq.mark_recovered(0);

    let rt = &harness.rt;
    let result = rt
        .block_on(async { harness.sync_coordinator.retry_dead_letter(None).await })
        .expect("Retry should complete");

    // Only 1 unrecovered entry should be retried
    assert_eq!(result.total(), 1, "Should retry only 1 unrecovered entry");
}

/// TC-312: DLQ recovery with max_entries limit
#[test]
fn test_dlq_recovery_with_limit() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Person",
            vec![("name", DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    let dlq = harness
        .sync_manager
        .sync_coordinator()
        .dead_letter_queue()
        .clone();

    for i in 0..5 {
        dlq.add(DeadLetterEntry::new(
            IndexOperation {
                key: IndexOpKey::new(1, "Person", "name"),
                index_type: IndexType::Fulltext,
                change_type: ChangeType::Insert,
                id: format!("limit_{}", i),
                data: Some(IndexData::Fulltext(format!("LimitTest{}", i))),
            },
            "Failure".to_string(),
            2,
        ));
    }

    let rt = &harness.rt;
    let result = rt
        .block_on(async { harness.sync_coordinator.retry_dead_letter(Some(2)).await })
        .expect("Retry with limit should complete");

    assert!(result.total() <= 2, "Should retry at most 2 entries");
}

/// TC-313: recover_operations_for_indexes with specific filter
#[test]
fn test_recover_operations_for_specific_index() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Person",
            vec![("name", DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    let dlq = harness
        .sync_manager
        .sync_coordinator()
        .dead_letter_queue()
        .clone();

    // Add entries for different indexes
    dlq.add(DeadLetterEntry::new(
        IndexOperation {
            key: IndexOpKey::new(1, "Person", "name"),
            index_type: IndexType::Fulltext,
            change_type: ChangeType::Insert,
            id: "p1".to_string(),
            data: Some(IndexData::Fulltext("Alice".to_string())),
        },
        "Fail".to_string(),
        2,
    ));
    dlq.add(DeadLetterEntry::new(
        IndexOperation {
            key: IndexOpKey::new(1, "Person", "email"),
            index_type: IndexType::Fulltext,
            change_type: ChangeType::Insert,
            id: "e1".to_string(),
            data: Some(IndexData::Fulltext("alice@test.com".to_string())),
        },
        "Fail".to_string(),
        2,
    ));

    let rt = &harness.rt;
    let result = rt
        .block_on(async {
            harness
                .sync_coordinator
                .recover_operations_for_indexes(1, "Person", "name")
                .await
        })
        .expect("Recovery for specific index should complete");

    assert_eq!(
        result.total(),
        1,
        "Should recover only 1 entry for Person.name"
    );
}

/// TC-314: RecoveryResult complete check
#[test]
fn test_recovery_result_complete_check() {
    use graphdb::sync::RecoveryResult;

    let result = RecoveryResult::default();
    assert!(result.is_complete(), "Default result should be complete");

    let failed = RecoveryResult::default();
    assert!(failed.is_complete());

    // Test the RecoveryResult accessors
    assert_eq!(result.total(), 0);
    assert_eq!(result.recovered(), 0);
    assert_eq!(result.failed(), 0);
}

/// TC-315: Standalone DLQ with stats verification
#[test]
fn test_dlq_stats_verification() {
    let config = DeadLetterQueueConfig::default();
    let dlq = DeadLetterQueue::new(config);

    for i in 0..5 {
        dlq.add(DeadLetterEntry::new(
            IndexOperation {
                key: IndexOpKey::new(1, "Test", "field"),
                index_type: IndexType::Fulltext,
                change_type: ChangeType::Insert,
                id: format!("doc_{}", i),
                data: Some(IndexData::Fulltext(format!("content_{}", i))),
            },
            "test error".to_string(),
            3,
        ));
    }

    dlq.mark_recovered(0);
    dlq.mark_recovered(2);

    let stats = dlq.get_stats();
    assert_eq!(stats.total_entries, 5);
    assert_eq!(stats.recovered_entries, 2);
    assert_eq!(stats.unrecovered_entries, 3);
    assert!(
        stats.oldest_entry_age > std::time::Duration::from_secs(0),
        "Oldest entry age should be positive"
    );
}
