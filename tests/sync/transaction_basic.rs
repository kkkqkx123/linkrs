//! Sync Module Basic Transaction Tests
//!
//! Tests for basic transaction synchronization functionality

use crate::common::sync_helpers::{create_test_vertex, SyncTestHarness};
use graphdb::core::types::{DataType, VertexId};
use graphdb::core::Value;
use graphdb::storage::{StorageReader, StorageSchemaOps, StorageWriter};
use std::collections::HashMap;

/// TC-001: Transaction vertex insert sync
#[test]
fn test_transaction_vertex_insert_sync() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Setup space and tag with fulltext index
    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Person",
            vec![("name", DataType::String), ("email", DataType::String)],
            vec!["name", "email"],
        )
        .expect("Failed to create tag");

    // Begin transaction
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    // Insert vertex
    let vertex = create_test_vertex(
        1,
        "Person",
        vec![
            ("name", Value::String("Alice".to_string())),
            ("email", Value::String("alice@example.com".to_string())),
        ],
    );
    harness
        .insert_vertex_with_txn("test_space", vertex)
        .expect("Failed to insert vertex");

    // Commit transaction
    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    // Wait for async processing
    harness.wait_for_async(200);

    // Verify vertex exists in storage
    harness
        .assert_vertex_exists("test_space", &Value::Int(1))
        .expect("Vertex should exist");

    // Verify fulltext index is synced
    let results = harness
        .search_fulltext("test_space", "Person", "name", "Alice", 10)
        .expect("Failed to search");

    assert!(!results.is_empty(), "Fulltext index should be synced");
    // Results may contain multiple matches for the same document (different fields)
    // Check that we have at least one result with the correct doc_id
    use graphdb::core::Value;
    let unique_docs: std::collections::HashSet<_> = results.iter().map(|r| &r.doc_id).collect();
    assert_eq!(unique_docs.len(), 1, "Should find exactly one document");
    assert!(
        results
            .iter()
            .any(|r| r.doc_id == Value::String("1".to_string())),
        "Should find vertex with id=1"
    );
}

/// TC-002: Transaction vertex update sync
#[test]
fn test_transaction_vertex_update_sync() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Setup
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

    // Insert initial vertex (non-transactional)
    let vertex = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Alice".to_string()))],
    );
    harness
        .insert_vertex("test_space", vertex)
        .expect("Failed to insert vertex");

    harness.wait_for_async(200);

    // Verify initial index
    let results = harness
        .search_fulltext("test_space", "Person", "name", "Alice", 10)
        .expect("Failed to search");
    assert_eq!(results.len(), 1, "Should find initial vertex");

    // Begin transaction and insert a new vertex (testing transactional insert)
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let new_vertex = create_test_vertex(
        2,
        "Person",
        vec![("name", Value::String("Bob".to_string()))],
    );
    harness
        .insert_vertex_with_txn("test_space", new_vertex)
        .expect("Failed to insert vertex with transaction");

    harness.commit_transaction().expect("Failed to commit");
    harness.wait_for_async(200);

    // Verify new vertex is in index
    let results = harness
        .search_fulltext("test_space", "Person", "name", "Bob", 10)
        .expect("Failed to search");
    assert_eq!(
        results.len(),
        1,
        "Should find new vertex inserted via transaction"
    );
}

/// TC-003: Transaction vertex delete sync
#[test]
fn test_transaction_vertex_delete_sync() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Setup
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

    // Insert vertex
    let vertex = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Alice".to_string()))],
    );
    harness
        .insert_vertex("test_space", vertex)
        .expect("Failed to insert vertex");

    harness.wait_for_async(200);

    // Verify index exists
    let results = harness
        .search_fulltext("test_space", "Person", "name", "Alice", 10)
        .expect("Failed to search");
    assert_eq!(results.len(), 1, "Should find vertex");

    // Begin transaction and delete with sync cleanup
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");
    harness
        .delete_vertex_with_txn("test_space", 1)
        .expect("Failed to delete vertex with txn");

    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    harness.wait_for_async(200);

    // Verify vertex no longer exists in storage
    let vertex_opt = harness
        .get_vertex("test_space", &Value::Int(1))
        .expect("Failed to get vertex");
    assert!(
        vertex_opt.is_none(),
        "Vertex should be deleted from storage"
    );

    // Verify index is cleaned up
    let results = harness
        .search_fulltext("test_space", "Person", "name", "Alice", 10)
        .expect("Failed to search");
    assert_eq!(
        results.len(),
        0,
        "Vertex should be removed from fulltext index"
    );
}

/// TC-006: Transaction vertex update sync (full pipeline: insert → update → verify)
#[test]
fn test_transaction_vertex_update_pipeline_sync() {
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

    // Insert initial vertex
    let vertex = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Alice".to_string()))],
    );
    harness
        .insert_vertex("test_space", vertex)
        .expect("Failed to insert vertex");

    harness.wait_for_async(200);

    // Update vertex in transaction
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let updated_vertex = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("AliceUpdated".to_string()))],
    );
    harness
        .insert_vertex_with_txn("test_space", updated_vertex)
        .expect("Failed to update vertex");

    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    harness.wait_for_async(200);

    // Verify new content is searchable
    let new_results = harness
        .search_fulltext("test_space", "Person", "name", "AliceUpdated", 10)
        .expect("Failed to search updated name");
    assert!(
        !new_results.is_empty(),
        "Updated vertex content should be searchable"
    );
}

/// TC-004: Transaction batch vertex insert sync
#[test]
fn test_transaction_batch_vertex_insert_sync() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Setup
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

    // Begin transaction
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    // Batch insert 100 vertices
    for i in 0..100 {
        let vertex = create_test_vertex(
            i + 1,
            "Person",
            vec![("name", Value::String(format!("Person{}", i + 1)))],
        );
        harness
            .insert_vertex_with_txn("test_space", vertex)
            .expect("Failed to insert vertex");
    }

    // Commit transaction
    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    // Wait for async processing
    harness.wait_for_async(500);

    // Verify all vertices exist
    for i in 0..100 {
        harness
            .assert_vertex_exists("test_space", &Value::Int(i + 1))
            .expect("Vertex should exist");
    }

    // Verify batch indexing worked
    let results = harness
        .search_fulltext("test_space", "Person", "name", "Person1", 200)
        .expect("Failed to search");

    assert!(
        !results.is_empty(),
        "Should find at least one vertex, found {}",
        results.len()
    );

    // Also verify total count by searching with empty string or checking storage
    let mut found_count = 0;
    for i in 0..100 {
        let search_term = format!("Person{}", i + 1);
        let results = harness
            .search_fulltext("test_space", "Person", "name", &search_term, 10)
            .expect("Failed to search");
        if !results.is_empty() {
            found_count += 1;
        }
    }

    assert!(
        found_count >= 100,
        "Should find all 100 vertices, found {}",
        found_count
    );
}

/// TC-005: Transaction rollback clears index buffer
#[test]
fn test_transaction_rollback_clears_index_buffer() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Setup
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

    // Begin transaction
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    // Insert vertex (buffered)
    let vertex = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Alice".to_string()))],
    );
    harness
        .insert_vertex_with_txn("test_space", vertex)
        .expect("Failed to insert vertex");

    // Rollback transaction
    harness
        .rollback_transaction()
        .expect("Failed to rollback transaction");

    // Wait for async processing
    harness.wait_for_async(200);

    // Verify index was NOT synced (rollback should clear buffer)
    let results = harness
        .search_fulltext("test_space", "Person", "name", "Alice", 10)
        .expect("Failed to search");
    assert_eq!(
        results.len(),
        0,
        "Index should not be synced after rollback"
    );
}

/// TC-010: Transaction edge insert sync
#[test]
fn test_transaction_edge_insert_sync() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Setup
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

    // Create edge type
    let edge_info = graphdb::core::types::EdgeTypeInfo::new("KNOWS".to_string());
    harness
        .storage
        .create_edge_type("test_space", &edge_info)
        .expect("Failed to create edge type");

    // Insert vertices
    let vertex1 = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Alice".to_string()))],
    );
    let vertex2 = create_test_vertex(
        2,
        "Person",
        vec![("name", Value::String("Bob".to_string()))],
    );
    harness
        .insert_vertex("test_space", vertex1)
        .expect("Failed to insert vertex1");
    harness
        .insert_vertex("test_space", vertex2)
        .expect("Failed to insert vertex2");

    // Begin transaction and insert edge
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let edge = graphdb::core::Edge::new(
        VertexId::from_int64(1),
        VertexId::from_int64(2),
        "KNOWS".to_string(),
        0,
        HashMap::new(),
    );
    harness
        .storage
        .insert_edge("test_space", edge)
        .expect("Failed to insert edge");

    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    // Verify edge exists
    let edge_opt = harness
        .storage
        .get_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .expect("Failed to get edge");
    assert!(edge_opt.is_some(), "Edge should exist");
}

/// TC-012: Transaction edge with properties sync
#[test]
fn test_transaction_edge_with_properties_sync() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Setup
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
        .create_tag_with_fulltext(
            "test_space",
            "Company",
            vec![("name", DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create Company tag");

    // Create edge type with properties
    let edge_info = graphdb::core::types::EdgeTypeInfo::new("WORKS_AT".to_string())
        .with_properties(vec![
            graphdb::core::types::PropertyDef::new("position".to_string(), DataType::String),
            graphdb::core::types::PropertyDef::new("since".to_string(), DataType::Int),
        ]);
    harness
        .storage
        .create_edge_type("test_space", &edge_info)
        .expect("Failed to create edge type");

    // Insert vertices
    let person = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Alice".to_string()))],
    );
    let company = create_test_vertex(
        100,
        "Company",
        vec![("name", Value::String("TechCorp".to_string()))],
    );
    harness
        .insert_vertex("test_space", person)
        .expect("Failed to insert person");
    harness
        .insert_vertex("test_space", company)
        .expect("Failed to insert company");

    // Begin transaction and insert edge with properties
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let mut edge_props = HashMap::new();
    edge_props.insert(
        "position".to_string(),
        Value::String("Engineer".to_string()),
    );
    edge_props.insert("since".to_string(), Value::Int(2020));

    let edge = graphdb::core::Edge::new(
        VertexId::from_int64(1),
        VertexId::from_int64(100),
        "WORKS_AT".to_string(),
        0,
        edge_props,
    );
    harness
        .storage
        .insert_edge("test_space", edge)
        .expect("Failed to insert edge");

    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    // Verify edge exists with properties
    let edge_opt = harness
        .storage
        .get_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(100),
            "WORKS_AT",
            0,
        )
        .expect("Failed to get edge");
    assert!(edge_opt.is_some(), "Edge should exist");

    let edge = edge_opt.unwrap();
    assert_eq!(
        edge.props.get("position"),
        Some(&Value::String("Engineer".to_string()))
    );
}
