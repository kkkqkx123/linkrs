//! Sync Module Edge Synchronization Tests (TC-300 ~ TC-305)
//!
//! Tests for SyncManager edge insert/delete/update operations

use crate::common::sync_helpers::{create_test_vertex, SyncTestHarness};
use graphdb::core::types::{DataType, EdgeTypeInfo, PropertyDef, TransactionId, VertexId};
use graphdb::core::Value;
use graphdb::storage::{StorageReader, StorageSchemaOps, StorageWriter};
use graphdb::sync::{EdgeProps, EdgeRef};
use std::collections::HashMap;

/// TC-300: Edge insert sync via SyncManager
#[test]
fn test_edge_insert_sync_via_manager() {
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

    let edge_info = EdgeTypeInfo::new("KNOWS".to_string()).with_properties(vec![
        PropertyDef::new("since".to_string(), DataType::Int),
        PropertyDef::new("description".to_string(), DataType::String),
    ]);
    harness
        .storage
        .create_edge_type("test_space", &edge_info)
        .expect("Failed to create edge type");

    // Create fulltext index for edge description
    let space_id = harness.storage.get_space_id("test_space").unwrap();
    harness.rt.block_on(async {
        harness
            .sync_coordinator
            .fulltext_manager()
            .create_index(
                space_id,
                "KNOWS",
                "description",
                Some(graphdb::search::EngineType::Bm25),
            )
            .await
            .expect("Failed to create fulltext index for edge");
    });

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

    harness.wait_for_async(200);

    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    let txn_id = TransactionId(harness.current_txn_id.unwrap());

    let edge = graphdb::core::Edge::new(
        VertexId::from_int64(1),
        VertexId::from_int64(2),
        "KNOWS".to_string(),
        0,
        HashMap::new(),
    );

    // Insert edge via storage
    harness
        .storage
        .insert_edge("test_space", edge)
        .expect("Failed to insert edge");

    // Sync edge via manager with description
    harness
        .sync_manager
        .on_edge_insert(
            txn_id,
            space_id,
            &graphdb::core::Edge::new(
                VertexId::from_int64(1),
                VertexId::from_int64(2),
                "KNOWS".to_string(),
                0,
                HashMap::new(),
            ),
        )
        .expect("Failed to sync edge insert");

    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    harness.wait_for_async(200);

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
    assert!(edge_opt.is_some(), "Edge should exist in storage");
}

/// TC-301: Edge with description text gets fulltext indexed
#[test]
fn test_edge_with_fulltext_property_sync() {
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

    let edge_info = EdgeTypeInfo::new("KNOWS".to_string()).with_properties(vec![PropertyDef::new(
        "description".to_string(),
        DataType::String,
    )]);
    harness
        .storage
        .create_edge_type("test_space", &edge_info)
        .expect("Failed to create edge type");

    let space_id = harness.storage.get_space_id("test_space").unwrap();
    harness.rt.block_on(async {
        harness
            .sync_coordinator
            .fulltext_manager()
            .create_index(
                space_id,
                "KNOWS",
                "description",
                Some(graphdb::search::EngineType::Bm25),
            )
            .await
            .expect("Failed to create fulltext index for edge");
    });

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

    harness.begin_transaction().expect("Failed to begin txn");
    let txn_id = TransactionId(harness.current_txn_id.unwrap());

    let mut props = HashMap::new();
    props.insert(
        "description".to_string(),
        Value::String("Alice knows Bob since 2020".to_string()),
    );
    let edge = graphdb::core::Edge::new(
        VertexId::from_int64(1),
        VertexId::from_int64(2),
        "KNOWS".to_string(),
        0,
        props,
    );

    harness
        .storage
        .insert_edge("test_space", edge.clone())
        .expect("Failed to insert edge");

    harness
        .sync_manager
        .on_edge_insert(txn_id, space_id, &edge)
        .expect("Failed to sync edge with description");

    harness.commit_transaction().expect("Failed to commit");
    harness.wait_for_async(300);

    // Search for the description content in the fulltext index
    let results = harness
        .search_fulltext("test_space", "KNOWS", "description", "knows Bob", 10)
        .expect("Failed to search edge description");

    assert!(
        !results.is_empty(),
        "Edge description should be fulltext indexed"
    );
}

/// TC-302: Edge delete sync via SyncManager
#[test]
fn test_edge_delete_sync_via_manager() {
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

    let space_id = harness.storage.get_space_id("test_space").unwrap();

    let edge_info = EdgeTypeInfo::new("KNOWS".to_string()).with_properties(vec![PropertyDef::new(
        "description".to_string(),
        DataType::String,
    )]);
    harness
        .storage
        .create_edge_type("test_space", &edge_info)
        .expect("Failed to create edge type");

    harness.rt.block_on(async {
        harness
            .sync_coordinator
            .fulltext_manager()
            .create_index(
                space_id,
                "KNOWS",
                "description",
                Some(graphdb::search::EngineType::Bm25),
            )
            .await
            .expect("Failed to create index");
    });

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

    harness.wait_for_async(200);

    // Insert an edge first (non-txn)
    let mut props = HashMap::new();
    props.insert(
        "description".to_string(),
        Value::String("Alice knows Bob".to_string()),
    );
    let edge = graphdb::core::Edge::new(
        VertexId::from_int64(1),
        VertexId::from_int64(2),
        "KNOWS".to_string(),
        0,
        props,
    );
    harness
        .storage
        .insert_edge("test_space", edge)
        .expect("Failed to insert initial edge");

    harness.begin_transaction().expect("Failed to begin txn");
    let txn_id = TransactionId(harness.current_txn_id.unwrap());

    // Delete via storage
    harness
        .storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .expect("Failed to delete edge");

    // Sync delete
    harness
        .sync_manager
        .on_edge_delete(txn_id, space_id, &Value::Int(1), &Value::Int(2), "KNOWS")
        .expect("Failed to sync edge delete");

    harness.commit_transaction().expect("Failed to commit");
    harness.wait_for_async(200);

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
    assert!(edge_opt.is_none(), "Edge should be deleted from storage");
}

/// TC-303: Edge update sync via SyncManager
#[test]
fn test_edge_update_sync_via_manager() {
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

    let space_id = harness.storage.get_space_id("test_space").unwrap();

    let edge_info = EdgeTypeInfo::new("KNOWS".to_string()).with_properties(vec![PropertyDef::new(
        "description".to_string(),
        DataType::String,
    )]);
    harness
        .storage
        .create_edge_type("test_space", &edge_info)
        .expect("Failed to create edge type");

    harness.rt.block_on(async {
        harness
            .sync_coordinator
            .fulltext_manager()
            .create_index(
                space_id,
                "KNOWS",
                "description",
                Some(graphdb::search::EngineType::Bm25),
            )
            .await
            .expect("Failed to create index");
    });

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

    harness.wait_for_async(200);

    // Insert initial edge
    let mut old_props = HashMap::new();
    old_props.insert(
        "description".to_string(),
        Value::String("old description".to_string()),
    );
    let old_edge = graphdb::core::Edge::new(
        VertexId::from_int64(1),
        VertexId::from_int64(2),
        "KNOWS".to_string(),
        0,
        old_props,
    );
    harness
        .storage
        .insert_edge("test_space", old_edge)
        .expect("Failed to insert old edge");

    harness.begin_transaction().expect("Failed to begin txn");
    let txn_id = TransactionId(harness.current_txn_id.unwrap());

    // Update edge
    let mut new_props = HashMap::new();
    new_props.insert(
        "description".to_string(),
        Value::String("new description".to_string()),
    );
    let new_edge = graphdb::core::Edge::new(
        VertexId::from_int64(1),
        VertexId::from_int64(2),
        "KNOWS".to_string(),
        0,
        new_props.clone(),
    );

    // Update via storage
    harness
        .storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .expect("Failed to delete old edge");
    harness
        .storage
        .insert_edge("test_space", new_edge)
        .expect("Failed to insert updated edge");

    // Sync update (delete old + insert new)
    harness
        .sync_manager
        .on_edge_update(
            txn_id,
            space_id,
            EdgeRef::new(&Value::Int(1), &Value::Int(2), "KNOWS"),
            EdgeProps::new(
                &[(
                    "description".to_string(),
                    Value::String("old description".to_string()),
                )],
                &[(
                    "description".to_string(),
                    Value::String("new description".to_string()),
                )],
            ),
        )
        .expect("Failed to sync edge update");

    harness.commit_transaction().expect("Failed to commit");
    harness.wait_for_async(300);

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
    assert!(edge_opt.is_some(), "Updated edge should exist");
    if let Some(edge) = edge_opt {
        assert_eq!(
            edge.props.get("description"),
            Some(&Value::String("new description".to_string()))
        );
    }
}

/// TC-304: Edge delete with non-existent index (graceful handling)
#[test]
fn test_edge_delete_no_index_graceful() {
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

    let space_id = harness.storage.get_space_id("test_space").unwrap();

    let edge_info = EdgeTypeInfo::new("KNOWS".to_string());
    harness
        .storage
        .create_edge_type("test_space", &edge_info)
        .expect("Failed to create edge type");

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

    harness.wait_for_async(200);

    // Insert edge
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

    harness.begin_transaction().expect("Failed to begin txn");
    let txn_id = TransactionId(harness.current_txn_id.unwrap());

    // Delete edge via storage
    harness
        .storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .expect("Failed to delete edge");

    // Sync delete (no index exists for KNOWS edge type)
    let result = harness.sync_manager.on_edge_delete(
        txn_id,
        space_id,
        &Value::Int(1),
        &Value::Int(2),
        "KNOWS",
    );
    assert!(
        result.is_ok(),
        "Edge delete should succeed even without index"
    );

    harness.commit_transaction().expect("Failed to commit");
}
