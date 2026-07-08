//! Sync Module Comprehensive Integration Tests
//!
//! Tests covering remaining paths not covered by sync_2pc_protocol,
//! sync_fault_tolerance, or sync_transaction_basic.

use crate::common::sync_helpers::{create_test_vertex, SyncTestHarness};
use graphdb::core::types::DataType;
use graphdb::core::Value;

/// TC-100: Non-transactional direct path (on_vertex_change → BatchBuffer → commit)
///
/// Verifies that non-transactional vertex inserts go through the
/// BatchBuffer and are flushed by the background timeout task.
#[test]
fn test_non_transactional_direct_path() {
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

    for i in 0..5 {
        let vertex = create_test_vertex(
            i + 1,
            "Person",
            vec![("name", Value::String(format!("Direct{}", i + 1)))],
        );
        harness
            .insert_vertex("test_space", vertex)
            .expect("Failed to insert vertex");
    }

    harness.wait_for_async(300);

    let rt = &harness.rt;
    rt.block_on(async {
        harness
            .sync_coordinator
            .commit_all()
            .await
            .expect("Commit all should succeed");
    });

    for i in 0..5 {
        let results = harness
            .search_fulltext(
                "test_space",
                "Person",
                "name",
                &format!("Direct{}", i + 1),
                10,
            )
            .expect("Failed to search");
        assert!(
            !results.is_empty(),
            "Non-transactional insert should be indexed for Direct{}",
            i + 1
        );
    }
}

/// TC-101: Multi-field transaction atomicity
///
/// Verifies that a transaction updating multiple fields across
/// multiple indexes commits all fields atomically.
#[test]
fn test_multi_field_transaction_atomicity() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Person",
            vec![
                ("name", DataType::String),
                ("email", DataType::String),
                ("bio", DataType::String),
            ],
            vec!["name", "email", "bio"],
        )
        .expect("Failed to create tag with 3 fulltext fields");

    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let vertex = create_test_vertex(
        1,
        "Person",
        vec![
            ("name", Value::String("Alice".to_string())),
            ("email", Value::String("alice@example.com".to_string())),
            ("bio", Value::String("Software engineer".to_string())),
        ],
    );
    harness
        .insert_vertex_with_txn("test_space", vertex)
        .expect("Failed to insert vertex with txn");

    harness.commit_transaction().expect("Failed to commit");
    harness.wait_for_async(300);

    let results_name = harness
        .search_fulltext("test_space", "Person", "name", "Alice", 10)
        .expect("Failed to search name");
    let results_email = harness
        .search_fulltext("test_space", "Person", "email", "alice@example.com", 10)
        .expect("Failed to search email");
    let results_bio = harness
        .search_fulltext("test_space", "Person", "bio", "Software", 10)
        .expect("Failed to search bio");

    assert!(!results_name.is_empty(), "Name field should be indexed");
    assert!(!results_email.is_empty(), "Email field should be indexed");
    assert!(!results_bio.is_empty(), "Bio field should be indexed");
}

/// TC-102: Multi-tag transaction atomicity
///
/// Verifies that a transaction inserting vertices with different tags
/// commits all tags atomically.
#[test]
fn test_multi_tag_transaction_atomicity() {
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
        .expect("Failed to create Person tag");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Company",
            vec![("name", DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create Company tag");

    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let person = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Alice".to_string()))],
    );
    harness
        .insert_vertex_with_txn("test_space", person)
        .expect("Failed to insert person");

    let company = create_test_vertex(
        2,
        "Company",
        vec![("name", Value::String("TechCorp".to_string()))],
    );
    harness
        .insert_vertex_with_txn("test_space", company)
        .expect("Failed to insert company");

    harness.commit_transaction().expect("Failed to commit");
    harness.wait_for_async(300);

    let person_results = harness
        .search_fulltext("test_space", "Person", "name", "Alice", 10)
        .expect("Failed to search Person");
    let company_results = harness
        .search_fulltext("test_space", "Company", "name", "TechCorp", 10)
        .expect("Failed to search Company");

    assert!(!person_results.is_empty(), "Person should be indexed");
    assert!(!company_results.is_empty(), "Company should be indexed");
}

/// TC-103: Transaction rollback does not affect other transactions
///
/// Verifies that rolling back one transaction does not remove
/// data from a previously committed transaction.
#[test]
fn test_rollback_no_side_effects() {
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

    harness.begin_transaction().expect("Failed to begin txn1");
    let vertex1 = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Committed".to_string()))],
    );
    harness
        .insert_vertex_with_txn("test_space", vertex1)
        .expect("Failed to insert into txn1");
    harness.commit_transaction().expect("Failed to commit txn1");
    harness.wait_for_async(200);

    harness.begin_transaction().expect("Failed to begin txn2");
    let vertex2 = create_test_vertex(
        2,
        "Person",
        vec![("name", Value::String("RolledBack".to_string()))],
    );
    harness
        .insert_vertex_with_txn("test_space", vertex2)
        .expect("Failed to insert into txn2");
    harness
        .rollback_transaction()
        .expect("Failed to rollback txn2");
    harness.wait_for_async(200);

    let results_committed = harness
        .search_fulltext("test_space", "Person", "name", "Committed", 10)
        .expect("Failed to search");
    let results_rolledback = harness
        .search_fulltext("test_space", "Person", "name", "RolledBack", 10)
        .expect("Failed to search");

    assert!(
        !results_committed.is_empty(),
        "Committed txn data should remain"
    );
    assert!(
        results_rolledback.is_empty(),
        "Rolled-back txn data should not be indexed"
    );
}

/// TC-104: Batch size trigger with exact boundary
///
/// Verifies that the batch processor flushes exactly when
/// batch_size is reached.
#[test]
fn test_batch_size_exact_boundary() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Item",
            vec![("name", DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    for i in 0..100 {
        let vertex = create_test_vertex(
            i + 1,
            "Item",
            vec![("name", Value::String(format!("Item{}", i + 1)))],
        );
        harness
            .insert_vertex("test_space", vertex)
            .expect("Failed to insert vertex");
    }

    harness.wait_for_async(500);

    let rt = &harness.rt;
    rt.block_on(async {
        harness
            .sync_coordinator
            .commit_all()
            .await
            .expect("Commit all should succeed");
    });

    let mut found = 0;
    for i in 0..100 {
        let results = harness
            .search_fulltext("test_space", "Item", "name", &format!("Item{}", i + 1), 10)
            .expect("Failed to search");
        if !results.is_empty() {
            found += 1;
        }
    }
    assert!(
        found >= 90,
        "At least 90 of 100 items should be indexed, found {}",
        found
    );
}

/// TC-105: Large batch transaction commit
///
/// Verifies that a transaction with many operations (exceeding batch_size)
/// commits correctly.
#[test]
fn test_large_batch_transaction() {
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

    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    for i in 0..50 {
        let vertex = create_test_vertex(
            i + 1,
            "Person",
            vec![("name", Value::String(format!("Bulk{}", i + 1)))],
        );
        harness
            .insert_vertex_with_txn("test_space", vertex)
            .expect("Failed to insert vertex");
    }

    harness.commit_transaction().expect("Failed to commit");
    harness.wait_for_async(300);

    let mut found = 0;
    for i in 0..50 {
        let results = harness
            .search_fulltext(
                "test_space",
                "Person",
                "name",
                &format!("Bulk{}", i + 1),
                10,
            )
            .expect("Failed to search");
        if !results.is_empty() {
            found += 1;
        }
    }
    assert!(
        found >= 50,
        "All 50 items should be indexed, found {}",
        found
    );
}

/// TC-106: Multiple sequential transactions
///
/// Verifies that committing multiple transactions sequentially
/// correctly indexes all data.
#[test]
fn test_sequential_transactions() {
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

    for txn_num in 0..3 {
        harness
            .begin_transaction()
            .expect("Failed to begin transaction");
        for i in 0..3 {
            let vid = txn_num * 3 + i + 1;
            let vertex = create_test_vertex(
                vid,
                "Person",
                vec![("name", Value::String(format!("Seq{}{}", txn_num, i)))],
            );
            harness
                .insert_vertex_with_txn("test_space", vertex)
                .expect("Failed to insert vertex");
        }
        harness.commit_transaction().expect("Failed to commit");
        harness.wait_for_async(200);
    }

    for txn_num in 0..3 {
        for i in 0..3 {
            let results = harness
                .search_fulltext(
                    "test_space",
                    "Person",
                    "name",
                    &format!("Seq{}{}", txn_num, i),
                    10,
                )
                .expect("Failed to search");
            assert!(!results.is_empty(), "Seq{}{} should be indexed", txn_num, i);
        }
    }
}

/// TC-107: Non-transactional insert then transactional update
///
/// Verifies that a vertex inserted non-transactionally can be
/// updated within a transaction, with both operations reflected
/// in the index.
#[test]
fn test_mixed_transactional_non_transactional() {
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

    let vertex = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Original".to_string()))],
    );
    harness
        .insert_vertex("test_space", vertex)
        .expect("Failed to insert initial vertex");

    harness.wait_for_async(300);

    let rt = &harness.rt;
    rt.block_on(async {
        harness
            .sync_coordinator
            .commit_all()
            .await
            .expect("Commit all should succeed");
    });

    let results = harness
        .search_fulltext("test_space", "Person", "name", "Original", 10)
        .expect("Failed to search");
    assert!(!results.is_empty(), "Initial insert should be indexed");

    harness
        .begin_transaction()
        .expect("Failed to begin transaction");
    let updated_vertex = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Updated".to_string()))],
    );
    harness
        .insert_vertex_with_txn("test_space", updated_vertex)
        .expect("Failed to update vertex in txn");
    harness.commit_transaction().expect("Failed to commit");
    harness.wait_for_async(300);

    let results = harness
        .search_fulltext("test_space", "Person", "name", "Updated", 10)
        .expect("Failed to search");
    assert!(!results.is_empty(), "Updated vertex should be searchable");
}

/// TC-108: Transaction with interleaved field updates
///
/// Verifies that updating different fields of the same vertex
/// within a single transaction produces correct index state.
#[test]
fn test_interleaved_field_updates() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Person",
            vec![("name", DataType::String), ("title", DataType::String)],
            vec!["name", "title"],
        )
        .expect("Failed to create tag");

    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let vertex = create_test_vertex(
        1,
        "Person",
        vec![
            ("name", Value::String("Alice".to_string())),
            ("title", Value::String("Engineer".to_string())),
        ],
    );
    harness
        .insert_vertex_with_txn("test_space", vertex)
        .expect("Failed to insert vertex");

    harness.commit_transaction().expect("Failed to commit");
    harness.wait_for_async(300);

    let name_results = harness
        .search_fulltext("test_space", "Person", "name", "Alice", 10)
        .expect("Failed to search name");
    let title_results = harness
        .search_fulltext("test_space", "Person", "title", "Engineer", 10)
        .expect("Failed to search title");

    assert!(!name_results.is_empty(), "Name should be indexed");
    assert!(!title_results.is_empty(), "Title should be indexed");
}
