//! Batch Processor Boundary Tests (TC-240 ~ TC-245)
//!
//! Tests for batch processor edge cases

use crate::common::sync_helpers::SyncTestHarness;
use graphdb::core::Value;
use graphdb::storage::StorageWriter;

/// TC-240: Empty batch handling
#[test]
fn test_empty_batch_handling() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    // Force commit all with no operations
    let rt = &harness.rt;
    rt.block_on(async {
        harness
            .sync_coordinator
            .commit_all()
            .await
            .expect("Commit all should succeed even with no operations");
    });

    // No panic = success
}

/// TC-241: Batch flush on timeout
#[test]
fn test_batch_flush_on_timeout() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("name", graphdb::core::types::DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    // Insert single vertex (below batch size)
    let mut properties = std::collections::HashMap::new();
    properties.insert(
        "name".to_string(),
        Value::String("Timeout Test".to_string()),
    );
    let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
    let vertex =
        graphdb::core::Vertex::new(graphdb::core::types::VertexId::from_int64(1), vec![tag]);

    harness
        .insert_vertex("test_space", vertex)
        .expect("Failed to insert vertex");

    harness.wait_for_async(300);

    let rt = &harness.rt;
    rt.block_on(async {
        harness
            .sync_coordinator
            .commit_all()
            .await
            .expect("Commit all should succeed");
    });

    harness
        .assert_vertex_exists("test_space", &Value::Int(1))
        .expect("Vertex should exist");
}

/// TC-242: Batch flush on size trigger
#[test]
fn test_batch_flush_on_size_trigger() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("name", graphdb::core::types::DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    for i in 0..100 {
        let mut properties = std::collections::HashMap::new();
        properties.insert("name".to_string(), Value::String(format!("Doc{}", i)));
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

    let rt = &harness.rt;
    rt.block_on(async {
        harness
            .sync_coordinator
            .commit_all()
            .await
            .expect("Commit all should succeed");
    });

    let mut found_count = 0;
    for i in 0..100 {
        let result = harness
            .get_vertex("test_space", &Value::Int(i + 1))
            .expect("Failed to get vertex");

        if result.is_some() {
            found_count += 1;
        }
    }

    assert!(
        found_count >= 90,
        "At least 90 of 100 vertices should be processed, found {}",
        found_count
    );
}

/// TC-243: Large batch processing
#[test]
fn test_large_batch_processing() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("name", graphdb::core::types::DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    for i in 0..200 {
        let mut properties = std::collections::HashMap::new();
        properties.insert(
            "name".to_string(),
            Value::String(format!("LargeBatch{}", i)),
        );
        let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
        let vertex = graphdb::core::Vertex::new(
            graphdb::core::types::VertexId::from_int64(i + 1),
            vec![tag],
        );

        harness
            .insert_vertex("test_space", vertex)
            .expect("Failed to insert vertex");
    }

    harness.wait_for_async(800);

    let rt = &harness.rt;
    rt.block_on(async {
        harness
            .sync_coordinator
            .commit_all()
            .await
            .expect("Commit all should succeed");
    });

    let mut found_count = 0;
    for i in 0..200 {
        let result = harness
            .get_vertex("test_space", &Value::Int(i + 1))
            .expect("Failed to get vertex");

        if result.is_some() {
            found_count += 1;
        }
    }

    assert!(
        found_count >= 180,
        "At least 180 of 200 vertices should be processed, found {}",
        found_count
    );
}

/// TC-244: Mixed operation types in batch
#[test]
fn test_mixed_operation_types_in_batch() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("name", graphdb::core::types::DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    for i in 0..5 {
        let mut properties = std::collections::HashMap::new();
        properties.insert("name".to_string(), Value::String(format!("Initial{}", i)));
        let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
        let vertex = graphdb::core::Vertex::new(
            graphdb::core::types::VertexId::from_int64(i + 1),
            vec![tag],
        );

        harness
            .insert_vertex("test_space", vertex)
            .expect("Failed to insert vertex");
    }

    harness.wait_for_async(200);

    for i in 0..3 {
        let mut properties = std::collections::HashMap::new();
        properties.insert("name".to_string(), Value::String(format!("Updated{}", i)));
        let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
        let vertex = graphdb::core::Vertex::new(
            graphdb::core::types::VertexId::from_int64(i + 1),
            vec![tag],
        );

        harness
            .storage
            .update_vertex("test_space", vertex)
            .expect("Failed to update vertex");
    }

    harness.wait_for_async(300);

    for i in 0..3 {
        let result = harness
            .get_vertex("test_space", &Value::Int(i + 1))
            .expect("Failed to get vertex");

        assert!(result.is_some(), "Updated vertex {} should exist", i + 1);
    }
}

/// TC-245: Rapid successive commits
#[test]
fn test_rapid_successive_commits() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");

    harness
        .create_tag_with_fulltext(
            "test_space",
            "Document",
            vec![("name", graphdb::core::types::DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    for i in 0..10 {
        let mut properties = std::collections::HashMap::new();
        properties.insert("name".to_string(), Value::String(format!("Rapid{}", i)));
        let tag = graphdb::core::vertex_edge_path::Tag::new("Document".to_string(), properties);
        let vertex = graphdb::core::Vertex::new(
            graphdb::core::types::VertexId::from_int64(i + 1),
            vec![tag],
        );

        harness
            .insert_vertex("test_space", vertex)
            .expect("Failed to insert vertex");

        let rt = &harness.rt;
        rt.block_on(async {
            harness
                .sync_coordinator
                .commit_all()
                .await
                .expect("Commit all should succeed");
        });

        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    harness.wait_for_async(300);

    let mut found_count = 0;
    for i in 0..10 {
        let result = harness
            .get_vertex("test_space", &Value::Int(i + 1))
            .expect("Failed to get vertex");

        if result.is_some() {
            found_count += 1;
        }
    }

    assert!(
        found_count >= 8,
        "At least 8 of 10 vertices should exist, found {}",
        found_count
    );
}
