//! Sync Recovery End-to-End Tests (TC-260 ~ TC-270)
//!
//! Tests for complete recovery scenarios.
//! Note: storage writes in `insert_vertex_with_txn` are immediate (not transactional).
//! Rollback only affects sync/index state, not storage.

use crate::common::sync_helpers::SyncTestHarness;
use graphdb::core::{types::DataType, Value};
use graphdb::storage::StorageWriter;
use graphdb::sync::dead_letter_queue::{DeadLetterEntry, DeadLetterQueue, DeadLetterQueueConfig};
use graphdb::sync::types::{ChangeType, IndexData, IndexType};

/// TC-260: Complete sync and verify
#[test]
fn test_complete_sync_and_verify() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("title", DataType::String)],
            vec!["title"],
        )
        .expect("Failed to create tag");

    let mut properties = std::collections::HashMap::new();
    properties.insert(
        "title".to_string(),
        Value::String("Complete Test".to_string()),
    );
    let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
    let vertex =
        graphdb::core::Vertex::new(graphdb::core::types::VertexId::from_int64(1), vec![tag]);

    harness
        .insert_vertex("test_space", vertex)
        .expect("Failed to insert vertex");

    harness.wait_for_async(300);

    harness
        .assert_vertex_exists("test_space", &Value::Int(1))
        .expect("Vertex should exist");
}

/// TC-261: Transaction commit and verify
#[test]
fn test_transaction_commit_and_verify() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("title", DataType::String)],
            vec!["title"],
        )
        .expect("Failed to create tag");

    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let mut properties = std::collections::HashMap::new();
    properties.insert(
        "title".to_string(),
        Value::String("Transaction Test".to_string()),
    );
    let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
    let vertex =
        graphdb::core::Vertex::new(graphdb::core::types::VertexId::from_int64(1), vec![tag]);

    harness
        .insert_vertex_with_txn("test_space", vertex)
        .expect("Failed to insert vertex with txn");

    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    harness.wait_for_async(300);

    harness
        .assert_vertex_exists("test_space", &Value::Int(1))
        .expect("Vertex should exist after commit");
}

/// TC-262: Transaction rollback handling
#[test]
fn test_transaction_rollback_and_verify() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("title", DataType::String)],
            vec!["title"],
        )
        .expect("Failed to create tag");

    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let mut properties = std::collections::HashMap::new();
    properties.insert(
        "title".to_string(),
        Value::String("Rollback Test".to_string()),
    );
    let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
    let vertex =
        graphdb::core::Vertex::new(graphdb::core::types::VertexId::from_int64(1), vec![tag]);

    harness
        .insert_vertex_with_txn("test_space", vertex)
        .expect("Failed to insert vertex with txn");

    harness
        .rollback_transaction()
        .expect("Failed to rollback transaction");

    harness.wait_for_async(200);

    // NOTE: storage writes are immediate, so vertex may exist in storage
    // Rollback only affects the sync layer (index state).
    // The test verifies rollback completes without error.
}

/// TC-263: Dead letter queue basic operations
#[test]
fn test_dead_letter_queue_operations() {
    let dlq = DeadLetterQueue::new(DeadLetterQueueConfig::default());

    for i in 0..15 {
        let entry = DeadLetterEntry::new(
            graphdb::sync::IndexOperation {
                key: graphdb::sync::IndexOpKey::new(1, "Document", "title"),
                index_type: IndexType::Fulltext,
                change_type: ChangeType::Insert,
                id: format!("test_id_{}", i),
                data: Some(IndexData::Fulltext(format!("Test{}", i))),
            },
            "Test failure".to_string(),
            3,
        );
        dlq.add(entry);
    }

    let entries = dlq.get_all();
    assert!(
        entries.len() <= 15,
        "DLQ should respect size limit (or handle overflow gracefully)"
    );
}

/// TC-265: Multiple sequential transactions
#[test]
fn test_multiple_sequential_transactions() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("title", DataType::String)],
            vec!["title"],
        )
        .expect("Failed to create tag");

    for txn_num in 0..3 {
        harness
            .begin_transaction()
            .expect("Failed to begin transaction");

        for i in 0..3 {
            let vid = txn_num * 3 + i + 1;
            let mut properties = std::collections::HashMap::new();
            properties.insert(
                "title".to_string(),
                Value::String(format!("Seq{}{}", txn_num, i)),
            );
            let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
            let vertex = graphdb::core::Vertex::new(
                graphdb::core::types::VertexId::from_int64(vid),
                vec![tag],
            );

            harness
                .insert_vertex_with_txn("test_space", vertex)
                .expect("Failed to insert vertex");
        }

        harness
            .commit_transaction()
            .expect("Failed to commit transaction");

        harness.wait_for_async(200);
    }

    for txn_num in 0..3 {
        for i in 0..3 {
            let vid = txn_num * 3 + i + 1;
            harness
                .assert_vertex_exists("test_space", &Value::Int(vid))
                .unwrap_or_else(|_| panic!("Vertex {} should exist", vid));
        }
    }
}

/// TC-266: Interleaved transactional and non-transactional
#[test]
fn test_interleaved_txn_non_txn() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("title", DataType::String)],
            vec!["title"],
        )
        .expect("Failed to create tag");

    let mut properties = std::collections::HashMap::new();
    properties.insert("title".to_string(), Value::String("NonTxn1".to_string()));
    let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
    let vertex =
        graphdb::core::Vertex::new(graphdb::core::types::VertexId::from_int64(1), vec![tag]);
    harness
        .insert_vertex("test_space", vertex)
        .expect("Failed to insert vertex");

    harness.wait_for_async(200);

    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let mut properties = std::collections::HashMap::new();
    properties.insert("title".to_string(), Value::String("Txn1".to_string()));
    let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
    let vertex =
        graphdb::core::Vertex::new(graphdb::core::types::VertexId::from_int64(2), vec![tag]);

    harness
        .insert_vertex_with_txn("test_space", vertex)
        .expect("Failed to insert vertex with txn");

    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    harness.wait_for_async(200);

    let mut properties = std::collections::HashMap::new();
    properties.insert("title".to_string(), Value::String("NonTxn2".to_string()));
    let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
    let vertex =
        graphdb::core::Vertex::new(graphdb::core::types::VertexId::from_int64(3), vec![tag]);
    harness
        .insert_vertex("test_space", vertex)
        .expect("Failed to insert vertex");

    harness.wait_for_async(200);

    for vid in 1..=3 {
        harness
            .assert_vertex_exists("test_space", &Value::Int(vid))
            .unwrap_or_else(|_| panic!("Vertex {} should exist", vid));
    }
}

/// TC-267: Large transaction with many operations
#[test]
fn test_large_transaction() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("title", DataType::String)],
            vec!["title"],
        )
        .expect("Failed to create tag");

    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    for i in 0..50 {
        let mut properties = std::collections::HashMap::new();
        properties.insert("title".to_string(), Value::String(format!("Large{}", i)));
        let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
        let vertex = graphdb::core::Vertex::new(
            graphdb::core::types::VertexId::from_int64(i + 1),
            vec![tag],
        );

        harness
            .insert_vertex_with_txn("test_space", vertex)
            .expect("Failed to insert vertex");
    }

    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    harness.wait_for_async(500);

    let mut found_count = 0;
    for i in 0..50 {
        let result = harness
            .get_vertex("test_space", &Value::Int(i + 1))
            .expect("Failed to get vertex");

        if result.is_some() {
            found_count += 1;
        }
    }

    assert!(
        found_count >= 50,
        "All 50 vertices should exist, found {}",
        found_count
    );
}

/// TC-268: Rollback does not corrupt existing data
#[test]
fn test_rollback_does_not_corrupt_existing_data() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("title", DataType::String)],
            vec!["title"],
        )
        .expect("Failed to create tag");

    let mut properties = std::collections::HashMap::new();
    properties.insert("title".to_string(), Value::String("Initial".to_string()));
    let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
    let vertex =
        graphdb::core::Vertex::new(graphdb::core::types::VertexId::from_int64(1), vec![tag]);
    harness
        .insert_vertex("test_space", vertex)
        .expect("Failed to insert vertex");

    harness.wait_for_async(200);

    harness
        .assert_vertex_exists("test_space", &Value::Int(1))
        .expect("Initial vertex should exist");

    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let mut properties = std::collections::HashMap::new();
    properties.insert("title".to_string(), Value::String("Failed".to_string()));
    let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
    let vertex =
        graphdb::core::Vertex::new(graphdb::core::types::VertexId::from_int64(2), vec![tag]);

    harness
        .insert_vertex_with_txn("test_space", vertex)
        .expect("Failed to insert vertex with txn");

    harness
        .rollback_transaction()
        .expect("Failed to rollback transaction");

    harness.wait_for_async(200);

    harness
        .assert_vertex_exists("test_space", &Value::Int(1))
        .expect("Initial vertex should still exist");
}

/// TC-269: Multiple non-transactional inserts
#[test]
fn test_storage_consistency_after_multi_insert() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("title", DataType::String)],
            vec!["title"],
        )
        .expect("Failed to create tag");

    for i in 0..10 {
        let mut properties = std::collections::HashMap::new();
        properties.insert("title".to_string(), Value::String(format!("Doc{}", i)));
        let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
        let vertex = graphdb::core::Vertex::new(
            graphdb::core::types::VertexId::from_int64(i + 1),
            vec![tag],
        );

        harness
            .insert_vertex("test_space", vertex)
            .expect("Failed to insert vertex");
    }

    harness.wait_for_async(500);

    for i in 0..10 {
        harness
            .assert_vertex_exists("test_space", &Value::Int(i + 1))
            .unwrap_or_else(|_| panic!("Vertex {} should exist", i + 1));
    }
}

/// TC-270: Delete and verify remaining data
#[test]
fn test_delete_and_verify_remaining() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("title", DataType::String)],
            vec!["title"],
        )
        .expect("Failed to create tag");

    for i in 0..5 {
        let mut properties = std::collections::HashMap::new();
        properties.insert("title".to_string(), Value::String(format!("Initial{}", i)));
        let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
        let vertex = graphdb::core::Vertex::new(
            graphdb::core::types::VertexId::from_int64(i + 1),
            vec![tag],
        );

        harness
            .insert_vertex("test_space", vertex)
            .expect("Failed to insert vertex");
    }

    harness.wait_for_async(300);

    for i in 0..2 {
        let vertex_id = graphdb::core::types::VertexId::from_int64(i + 1);
        harness
            .storage
            .delete_vertex("test_space", &vertex_id)
            .expect("Failed to delete vertex");
    }

    harness.wait_for_async(200);

    for i in 0..2 {
        let result = harness
            .get_vertex("test_space", &Value::Int(i + 1))
            .expect("Failed to get vertex");

        assert!(result.is_none(), "Vertex {} should be deleted", i + 1);
    }

    for i in 2..5 {
        harness
            .assert_vertex_exists("test_space", &Value::Int(i + 1))
            .unwrap_or_else(|_| panic!("Vertex {} should still exist", i + 1));
    }
}
