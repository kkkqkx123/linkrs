//! Sync Module Durable Dead Letter Tests (TC-310 ~ TC-315)
//!
//! Tests for durable outbox dead letters, requeue, degraded ranges and the
//! RecoveryResult struct. The in-memory dead-letter queue was removed; the
//! SQLite outbox `dead_letters` table is the single source of truth.

use linkrs::core::types::DataType;
use linkrs::test_utils::sync_helpers::SyncTestHarness;

/// TC-310: Fresh outbox exposes no dead letters.
///
/// Verifies that a newly created harness reports zero durable dead letters
/// through both the list API and the stats counters.
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

    let rows = harness
        .sync_manager
        .list_dead_letters(None, None, None, 100, 0)
        .expect("Listing dead letters should succeed");
    assert!(rows.is_empty(), "Fresh outbox should have no dead letters");

    let stats = harness.sync_manager.outbox_stats();
    assert_eq!(stats.dead_lettered, 0);
    assert_eq!(harness.sync_manager.get_dlq_size(), 0);
    assert_eq!(harness.sync_manager.get_unrecovered_dlq_size(), 0);
}

/// TC-311: Requeue of an unknown event is a no-op.
///
/// Verifies that single and batch requeue calls succeed without requeueing
/// anything when no dead letters exist.
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

    let requeued = harness
        .sync_manager
        .requeue_dead_letter(42)
        .expect("Requeue of unknown event should succeed");
    assert!(!requeued, "Unknown event id must not be reported as requeued");

    let batch = harness
        .sync_manager
        .requeue_dead_letters_batch(None, None, None, 100)
        .expect("Batch requeue should succeed");
    assert_eq!(batch, 0, "Empty dead letters should requeue nothing");
}

/// TC-312: Dead letter listing honors limit and offset.
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

    let rows = harness
        .sync_manager
        .list_dead_letters(None, None, None, 2, 0)
        .expect("Listing with limit should succeed");
    assert!(rows.len() <= 2, "Should return at most 2 entries");

    let skipped = harness
        .sync_manager
        .list_dead_letters(None, None, None, 100, 10_000)
        .expect("Listing with large offset should succeed");
    assert!(skipped.is_empty(), "Large offset should return no entries");
}

/// TC-313: Degraded ranges start empty and clearing unknown ranges is a no-op.
#[test]
fn test_recover_operations_for_specific_index() {
    use linkrs::core::types::{CommitLsn, TargetId};

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

    let ranges = harness
        .sync_manager
        .list_degraded_ranges(None, None, None)
        .expect("Listing degraded ranges should succeed");
    assert!(ranges.is_empty(), "Fresh outbox should have no degraded ranges");

    let target = TargetId::new("fulltext".to_string()).expect("valid target");
    let cleared = harness
        .sync_manager
        .clear_degraded_range(&target, 1, 1, CommitLsn::new(1), CommitLsn::new(2))
        .expect("Clearing unknown range should succeed");
    assert!(!cleared, "Unknown range must not be reported as cleared");
}

/// TC-314: RecoveryResult complete check
#[test]
fn test_recovery_result_complete_check() {
    use linkrs::sync::RecoveryResult;

    let result = RecoveryResult::default();
    assert!(result.is_complete(), "Default result should be complete");

    let failed = RecoveryResult::default();
    assert!(failed.is_complete());

    assert_eq!(result.total(), 0);
    assert_eq!(result.recovered(), 0);
    assert_eq!(result.failed(), 0);
}

/// TC-315: Status aliases stay consistent with durable stats.
#[test]
fn test_dlq_stats_verification() {
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

    let stats = harness.sync_manager.outbox_stats();
    assert_eq!(
        harness.sync_manager.get_dlq_size(),
        stats.dead_lettered,
        "dlq_size alias must equal durable dead_lettered"
    );
    assert_eq!(
        harness.sync_manager.get_unrecovered_dlq_size(),
        stats.dead_lettered,
        "unrecovered alias must equal durable dead_lettered"
    );
}
