//! Business Scenario Integration Tests
//!
//! Tests complex real-world scenarios that exercise multiple modules:
//! - Multi-tag vertices: read back all tag data
//! - Dangling edge lifecycle: create → detect → repair → verify
//! - Schema alteration with existing data
//! - Edge traversal with multiple edge types
//! - Error handling for invalid operations

mod common;

use graphdb_storage::core::types::{PropertyDef, VertexId};
use graphdb_storage::core::vertex_edge_path::Tag;
use graphdb_storage::core::DataType;
use graphdb_storage::core::{Edge, EdgeDirection, Value, Vertex};
use graphdb_storage::storage::{
    StorageAdmin, StoragePersistenceOps, StorageReader, StorageSchemaOps, StorageWriter,
};

// ── Scenario 1: Multi-Tag Vertex Operations ──

/// Business scenario: A vertex has multiple roles (e.g., Person + Employee).
/// The user inserts a vertex with two tags and expects to read all data back.
/// In addition, scan_vertices should not return duplicates.
#[test]
fn test_multi_tag_vertex_get_and_scan() {
    let mut storage = common::create_in_memory_storage();
    common::setup_multi_tag_schema(&mut storage);

    // Insert a vertex with both Person and Employee tags
    let vertex = common::create_multi_tag_vertex(1, "Alice", 30, "AcmeCorp", 100000);
    storage.insert_vertex("test_space", vertex).unwrap();

    // get_vertex should return data from ALL tags
    let retrieved = storage
        .get_vertex("test_space", &VertexId::from_int64(1))
        .unwrap()
        .expect("Vertex should exist");

    // Person properties
    assert_eq!(
        retrieved.properties.get("name"),
        Some(&Value::String("Alice".to_string()))
    );
    assert_eq!(retrieved.properties.get("age"), Some(&Value::BigInt(30)));

    // Employee properties
    assert_eq!(
        retrieved.properties.get("company"),
        Some(&Value::String("AcmeCorp".to_string()))
    );
    assert_eq!(
        retrieved.properties.get("salary"),
        Some(&Value::BigInt(100000))
    );

    // Both tags should be present
    let tag_names: Vec<&str> = retrieved.tags.iter().map(|t| t.name.as_str()).collect();
    assert!(tag_names.contains(&"Person"), "Should have Person tag");
    assert!(tag_names.contains(&"Employee"), "Should have Employee tag");

    // scan_vertices should NOT return duplicates (vertex with 2 tags = 1 result)
    let all_vertices = storage.scan_vertices("test_space").unwrap();
    assert_eq!(
        all_vertices.len(),
        1,
        "scan_vertices should not duplicate multi-tag vertices"
    );
}

/// Business scenario: User queries a vertex by a single tag.
/// This should return the correct tag-specific data.
#[test]
fn test_multi_tag_vertex_query_by_tag() {
    let mut storage = common::create_in_memory_storage();
    common::setup_multi_tag_schema(&mut storage);

    let vertex = common::create_multi_tag_vertex(1, "Bob", 28, "BetaInc", 80000);
    storage.insert_vertex("test_space", vertex).unwrap();

    // Query by Person tag
    let persons = storage
        .scan_vertices_by_tag("test_space", "Person")
        .unwrap();
    assert_eq!(persons.len(), 1);
    assert_eq!(
        persons[0].properties.get("name"),
        Some(&Value::String("Bob".to_string()))
    );

    // Query by Employee tag
    let employees = storage
        .scan_vertices_by_tag("test_space", "Employee")
        .unwrap();
    assert_eq!(employees.len(), 1);
    assert_eq!(
        employees[0].properties.get("company"),
        Some(&Value::String("BetaInc".to_string()))
    );
}

/// Business scenario: User deletes a specific tag from a vertex
/// (e.g., remove Employee role but keep Person).
#[test]
fn test_delete_tag_from_vertex() {
    let mut storage = common::create_in_memory_storage();
    common::setup_multi_tag_schema(&mut storage);

    let vertex = common::create_multi_tag_vertex(1, "Charlie", 35, "GammaCorp", 120000);
    storage.insert_vertex("test_space", vertex).unwrap();

    // Delete the Employee tag from vertex
    let deleted = storage
        .delete_tags(
            "test_space",
            &VertexId::from_int64(1),
            &["Employee".to_string()],
        )
        .unwrap();
    assert_eq!(deleted, 1, "Should have deleted 1 tag");

    // Vertex still exists with Person tag
    let retrieved = storage
        .get_vertex("test_space", &VertexId::from_int64(1))
        .unwrap()
        .expect("Vertex should still exist");
    let tag_names: Vec<&str> = retrieved.tags.iter().map(|t| t.name.as_str()).collect();
    assert!(tag_names.contains(&"Person"), "Person tag should remain");
    assert!(
        !tag_names.contains(&"Employee"),
        "Employee tag should be deleted"
    );

    // Person properties still accessible
    assert_eq!(
        retrieved.properties.get("name"),
        Some(&Value::String("Charlie".to_string()))
    );
}

// ── Scenario 2: Dangling Edge Lifecycle ──

/// Business scenario: A vertex is deleted but its edges are not cleaned up,
/// creating dangling edges. The admin detects and repairs them.
#[test]
fn test_dangling_edge_detection_and_repair() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);

    // Insert two vertices with an edge
    let alice = common::create_person_vertex(1, "Alice", 30);
    let bob = common::create_person_vertex(2, "Bob", 25);
    storage.insert_vertex("test_space", alice).unwrap();
    storage.insert_vertex("test_space", bob).unwrap();

    let edge = common::create_knows_edge(1, 2, 2020);
    storage.insert_edge("test_space", edge).unwrap();

    // Delete vertex 2 (Bob) WITHOUT cascade — leaves dangling edge
    storage
        .delete_vertex("test_space", &VertexId::from_int64(2))
        .unwrap();

    // Detect dangling edges
    let dangling = storage
        .find_dangling_edges("test_space")
        .expect("find_dangling_edges should succeed");
    assert!(
        !dangling.is_empty(),
        "Should find at least one dangling edge"
    );
    let dangling_edge = &dangling[0];
    assert_eq!(dangling_edge.edge_type, "KNOWS");
    assert_eq!(dangling_edge.src, VertexId::from_int64(1));
    assert_eq!(dangling_edge.dst, VertexId::from_int64(2));

    // Repair dangling edges
    let repaired_count = storage
        .repair_dangling_edges("test_space")
        .expect("repair_dangling_edges should succeed");
    assert!(repaired_count >= 1, "Should have repaired at least 1 edge");

    // Verify no dangling edges remain
    let remaining = storage
        .find_dangling_edges("test_space")
        .expect("find_dangling_edges should succeed");
    assert!(
        remaining.is_empty(),
        "All dangling edges should be repaired"
    );
}

/// Business scenario: When no dangling edges exist, find/repair should
/// return empty/success.
#[test]
fn test_no_dangling_edges_on_clean_state() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);
    common::insert_test_data(&mut storage, "test_space");

    // No dangling edges expected
    let dangling = storage
        .find_dangling_edges("test_space")
        .expect("find_dangling_edges should succeed");
    assert!(dangling.is_empty(), "No dangling edges expected");

    // Repair on clean state should succeed
    let repaired = storage
        .repair_dangling_edges("test_space")
        .expect("repair_dangling_edges should succeed");
    assert_eq!(repaired, 0, "No edges should need repair");
}

// ── Scenario 3: Schema Alteration with Existing Data ──

/// Business scenario: User alters a tag to add new properties and
/// remove old ones. Existing data should remain readable for
/// unchanged properties.
#[test]
fn test_alter_tag_properties_with_existing_data() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);
    common::insert_test_data(&mut storage, "test_space");

    // Note: the original schema has name(String) + age(BigInt)
    // Let's add 'email' and remove 'age'
    storage
        .alter_tag(
            "test_space",
            "Person",
            vec![PropertyDef::new("email".to_string(), DataType::String)],
            vec!["age".to_string()],
        )
        .expect("alter_tag should succeed");

    // Existing data should still be readable for unchanged property
    let alice = storage
        .get_vertex("test_space", &VertexId::from_int64(1))
        .unwrap()
        .expect("Alice should exist after alter");
    assert_eq!(
        alice.properties.get("name"),
        Some(&Value::String("Alice".to_string())),
        "name property should survive alteration"
    );

    // Insert new data with new schema
    let new_person = Vertex::new(
        VertexId::from_int64(3),
        vec![Tag::new(
            "Person".to_string(),
            vec![
                ("name".to_string(), Value::String("Diana".to_string())),
                (
                    "email".to_string(),
                    Value::String("diana@test.com".to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        )],
    );
    storage.insert_vertex("test_space", new_person).unwrap();

    let diana = storage
        .get_vertex("test_space", &VertexId::from_int64(3))
        .unwrap()
        .expect("Diana should exist");
    assert_eq!(
        diana.properties.get("email"),
        Some(&Value::String("diana@test.com".to_string()))
    );
}

/// Business scenario: User alters an edge type, adding and removing
/// properties, while existing edges remain valid.
#[test]
fn test_alter_edge_type_properties() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);
    common::insert_test_data(&mut storage, "test_space");

    // Add 'weight' property, remove 'since'
    storage
        .alter_edge_type(
            "test_space",
            "KNOWS",
            vec![PropertyDef::new("weight".to_string(), DataType::Double)],
            vec!["since".to_string()],
        )
        .expect("alter_edge_type should succeed");

    // Existing edge still readable (without the removed property)
    let edge = storage
        .get_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .unwrap()
        .expect("Edge should still exist after alter");
    assert_eq!(edge.src, VertexId::from_int64(1));
    assert_eq!(edge.dst, VertexId::from_int64(2));

    // Insert a new edge with new properties
    let new_edge = Edge::new(
        VertexId::from_int64(1),
        VertexId::from_int64(2),
        "KNOWS".to_string(),
        1,
        vec![("weight".to_string(), Value::Double(0.5))]
            .into_iter()
            .collect(),
    );
    storage.insert_edge("test_space", new_edge).unwrap();

    let retrieved = storage
        .get_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            1,
        )
        .unwrap()
        .expect("New edge with rank 1 should exist");
    assert_eq!(retrieved.props.get("weight"), Some(&Value::Double(0.5)));
}

// ── Scenario 4: Edge Traversal with Multiple Edge Types ──

/// Business scenario: A graph has multiple edge types between vertices.
/// Queries with direction must correctly filter by edge type.
#[test]
fn test_multi_edge_type_traversal() {
    let mut storage = common::create_in_memory_storage();
    common::setup_multi_tag_schema(&mut storage);

    // Insert Persons: Alice(1), Bob(2), Charlie(3)
    let alice = common::create_person_vertex(1, "Alice", 30);
    let bob = common::create_person_vertex(2, "Bob", 25);
    let charlie = common::create_person_vertex(3, "Charlie", 35);
    storage.insert_vertex("test_space", alice).unwrap();
    storage.insert_vertex("test_space", bob).unwrap();
    storage.insert_vertex("test_space", charlie).unwrap();

    // Insert Employee vertex for Charlie
    let charlie_emp = Vertex::new(
        VertexId::from_int64(3),
        vec![Tag::new(
            "Employee".to_string(),
            vec![
                ("company".to_string(), Value::String("AcmeCorp".to_string())),
                ("salary".to_string(), Value::BigInt(90000)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    storage.insert_vertex("test_space", charlie_emp).unwrap();

    // KNOWS edges between persons
    let knows_ab = common::create_knows_edge(1, 2, 2020); // Alice -> Bob
    let knows_bc = common::create_knows_edge(2, 3, 2021); // Bob -> Charlie
    let knows_ca = common::create_knows_edge(3, 1, 2022); // Charlie -> Alice
    storage.insert_edge("test_space", knows_ab).unwrap();
    storage.insert_edge("test_space", knows_bc).unwrap();
    storage.insert_edge("test_space", knows_ca).unwrap();

    // WORKS_AT edge: Alice -> Charlie (works at Charlie's company)
    let works_at = Edge::new(
        VertexId::from_int64(1),
        VertexId::from_int64(3),
        "WORKS_AT".to_string(),
        0,
        vec![("role".to_string(), Value::String("Engineer".to_string()))]
            .into_iter()
            .collect(),
    );
    storage.insert_edge("test_space", works_at).unwrap();

    // Alice's outgoing: KNOWS->Bob + WORKS_AT->Charlie = 2
    let alice_out = storage
        .get_node_edges("test_space", &VertexId::from_int64(1), EdgeDirection::Out)
        .unwrap();
    assert_eq!(alice_out.len(), 2, "Alice should have 2 outgoing edges");

    // Alice's incoming: KNOWS from Charlie (Charlie -> Alice)
    let alice_in = storage
        .get_node_edges("test_space", &VertexId::from_int64(1), EdgeDirection::In)
        .unwrap();
    assert_eq!(
        alice_in.len(),
        1,
        "Alice should have 1 incoming edge (KNOWS from Charlie)"
    );

    // Charlie's incoming: KNOWS from Bob + WORKS_AT from Alice = 2
    let charlie_in = storage
        .get_node_edges("test_space", &VertexId::from_int64(3), EdgeDirection::In)
        .unwrap();
    assert_eq!(charlie_in.len(), 2, "Charlie should have 2 incoming edges");

    // Charlie's outgoing: KNOWS -> Alice, no WORKS_AT from Charlie
    let charlie_out = storage
        .get_node_edges("test_space", &VertexId::from_int64(3), EdgeDirection::Out)
        .unwrap();
    assert_eq!(
        charlie_out.len(),
        1,
        "Charlie should have 1 outgoing edge (KNOWS -> Alice)"
    );
}

// ── Scenario 5: Storage Stats Integrity ──

/// Business scenario: Storage statistics should reflect the actual
/// state of the data after various operations.
#[test]
fn test_storage_stats_reflect_data_state() {
    let mut storage = common::create_in_memory_storage();

    // Empty stats
    let stats = storage.get_storage_stats();
    assert_eq!(stats.total_spaces, 0);
    assert_eq!(stats.total_vertices, 0);

    // After schema creation
    common::setup_basic_schema(&mut storage);
    let stats = storage.get_storage_stats();
    assert_eq!(stats.total_spaces, 1);
    assert_eq!(stats.total_tags, 1);
    assert_eq!(stats.total_edge_types, 1);

    // After data insertion
    common::insert_test_data(&mut storage, "test_space");
    let stats = storage.get_storage_stats();
    assert_eq!(stats.total_vertices, 2);
    assert_eq!(stats.total_edges, 1);

    // After deleting a vertex (without cascade), edge remains as dangling
    // Note: delete_vertex does NOT cascade-delete edges. The edge still
    // exists as a dangling edge. Use delete_vertex_with_edges for cascade.
    storage
        .delete_vertex("test_space", &VertexId::from_int64(1))
        .unwrap();
    let stats = storage.get_storage_stats();
    assert_eq!(stats.total_edges, 1, "Edge still exists as dangling edge");
}

// ── Scenario 6: Error Recovery Consistency ──

/// Business scenario: After a failed batch insert, the system should
/// be in a consistent state with no partial data.
#[test]
fn test_batch_insert_failure_rollback_consistency() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);

    // Insert one valid vertex first
    let alice = common::create_person_vertex(1, "Alice", 30);
    storage.insert_vertex("test_space", alice).unwrap();

    // Batch insert with a duplicate ID — should fail and rollback
    let vertices = vec![
        Vertex::new(
            VertexId::from_int64(2),
            vec![Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::String("Bob".to_string()))]
                    .into_iter()
                    .collect(),
            )],
        ),
        // Duplicate ID — intentionally cause failure
        Vertex::new(
            VertexId::from_int64(1),
            vec![Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::String("Duplicate".to_string()))]
                    .into_iter()
                    .collect(),
            )],
        ),
    ];

    let result = storage.batch_insert_vertices("test_space", vertices);
    assert!(result.is_err(), "Batch insert should fail on duplicate");

    // Alice should still exist (rollback should not affect her)
    let alice = storage
        .get_vertex("test_space", &VertexId::from_int64(1))
        .unwrap()
        .expect("Alice should still exist after failed batch");
    assert_eq!(
        alice.properties.get("name"),
        Some(&Value::String("Alice".to_string()))
    );

    // Bob (vertex 2) should NOT have been inserted
    let bob = storage
        .get_vertex("test_space", &VertexId::from_int64(2))
        .unwrap();
    assert!(bob.is_none(), "Bob should not exist after rollback");
}

// ── Scenario 7: Multiple Flush/Load Cycles ──

/// Business scenario: User repeatedly persists and reloads data,
/// adding more data each time.
///
#[test]
fn test_multi_cycle_flush_and_load() {
    let dir = std::env::temp_dir()
        .join("graphdb_storage_int_test")
        .join("multi_cycle_flush");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Cycle 1: Initial setup and save to disk (includes schema + data)
    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");
        StorageAdmin::save_to_disk(&storage).unwrap();
        storage.create_checkpoint().unwrap();
    }

    // Cycle 2: Load, add more data, save again
    {
        let mut storage = common::open_persistent_storage(&dir);
        common::verify_test_data(&storage, "test_space");

        let charlie = common::create_person_vertex(3, "Charlie", 35);
        storage.insert_vertex("test_space", charlie).unwrap();

        let edge = common::create_knows_edge(1, 3, 2022);
        storage.insert_edge("test_space", edge).unwrap();
        StorageAdmin::save_to_disk(&storage).unwrap();
        storage.create_checkpoint().unwrap();
    }

    // Cycle 3: Load, add more data, save again
    {
        let mut storage = common::open_persistent_storage(&dir);
        common::verify_test_data(&storage, "test_space");

        let charlie = storage
            .get_vertex("test_space", &VertexId::from_int64(3))
            .unwrap()
            .expect("Charlie should survive reload");
        assert_eq!(
            charlie.properties.get("name"),
            Some(&Value::String("Charlie".to_string()))
        );

        let dave = common::create_person_vertex(4, "Dave", 40);
        storage.insert_vertex("test_space", dave).unwrap();

        let edge = common::create_knows_edge(1, 4, 2023);
        storage.insert_edge("test_space", edge).unwrap();
        StorageAdmin::save_to_disk(&storage).unwrap();
        storage.create_checkpoint().unwrap();
    }

    // Cycle 4: Final load, verify everything survived
    {
        let storage = common::open_persistent_storage(&dir);
        common::verify_test_data(&storage, "test_space");

        // Charlie from cycle 2 survived
        let charlie = storage
            .get_vertex("test_space", &VertexId::from_int64(3))
            .unwrap()
            .expect("Charlie should survive");
        assert_eq!(
            charlie.properties.get("name"),
            Some(&Value::String("Charlie".to_string()))
        );

        // Dave from cycle 3 survived
        let dave = storage
            .get_vertex("test_space", &VertexId::from_int64(4))
            .unwrap()
            .expect("Dave should survive");
        assert_eq!(
            dave.properties.get("name"),
            Some(&Value::String("Dave".to_string()))
        );

        // Edge from cycle 2 survived
        let edge_13 = storage
            .get_edge(
                "test_space",
                &VertexId::from_int64(1),
                &VertexId::from_int64(3),
                "KNOWS",
                0,
            )
            .unwrap()
            .expect("Edge Alice->Charlie should survive");
        assert_eq!(edge_13.ranking, 0);

        // Edge from cycle 3 survived
        let edge_14 = storage
            .get_edge(
                "test_space",
                &VertexId::from_int64(1),
                &VertexId::from_int64(4),
                "KNOWS",
                0,
            )
            .unwrap()
            .expect("Edge Alice->Dave should survive");
        assert_eq!(edge_14.ranking, 0);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

// ── Scenario 8: Clear Space ──

/// Business scenario: User clears all data from a space while preserving
/// the space and its schema.
#[test]
fn test_clear_space_preserves_schema() {
    let mut storage = common::create_in_memory_storage();
    let _space_id = common::setup_basic_schema(&mut storage);
    common::insert_test_data(&mut storage, "test_space");

    // Clear the space
    storage.clear_space("test_space").unwrap();

    // Space still exists
    assert!(storage.space_exists("test_space"));

    // Data is gone
    assert_eq!(storage.scan_vertices("test_space").unwrap().len(), 0);
    assert_eq!(storage.scan_all_edges("test_space").unwrap().len(), 0);

    // Schema is cleared; must recreate tags/edge types before inserting new data
    // (clear_space drops all vertex types and edge types in the space)
    common::create_person_tag(&mut storage, "test_space");
    common::create_knows_edge_type(&mut storage, "test_space");

    // Can insert new data after clear
    let alice = common::create_person_vertex(1, "Alice", 30);
    storage.insert_vertex("test_space", alice).unwrap();
    assert_eq!(storage.scan_vertices("test_space").unwrap().len(), 1);
}

// ── Scenario 9: Delete Vertex With Edges (Cascade) ──

/// Business scenario: User deletes a vertex and all its edges should
/// also be deleted (cascade).
#[test]
fn test_delete_vertex_with_edges_cascade() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);

    let alice = common::create_person_vertex(1, "Alice", 30);
    let bob = common::create_person_vertex(2, "Bob", 25);
    storage.insert_vertex("test_space", alice).unwrap();
    storage.insert_vertex("test_space", bob).unwrap();

    let edge = common::create_knows_edge(1, 2, 2020);
    storage.insert_edge("test_space", edge).unwrap();

    // Delete Alice with edges
    storage
        .delete_vertex_with_edges("test_space", &VertexId::from_int64(1))
        .unwrap();

    // Alice is gone
    assert!(storage
        .get_vertex("test_space", &VertexId::from_int64(1))
        .unwrap()
        .is_none());

    // Edge is gone (cascaded)
    let remaining_edges = storage.scan_edges_by_type("test_space", "KNOWS").unwrap();
    assert_eq!(remaining_edges.len(), 0);
}

// ── Scenario 10: Unique Index Constraint ──

/// Business scenario: User creates a unique index and attempts to
/// insert a duplicate value, which should be rejected.
#[test]
fn test_unique_index_rejects_duplicate() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);

    use graphdb_storage::core::types::{Index, IndexConfig, IndexField, IndexType};
    let unique_index = Index::new(IndexConfig {
        id: 1,
        name: "person_name_unique_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String(String::new()),
            false,
        )],
        properties: vec![],
        index_type: IndexType::TagIndex,
        is_unique: true,
        partial_condition: None,
    });

    storage
        .create_tag_index("test_space", &unique_index)
        .unwrap();
    storage
        .rebuild_tag_index("test_space", "person_name_unique_idx")
        .unwrap();

    let alice = common::create_person_vertex(1, "Alice", 30);
    storage.insert_vertex("test_space", alice).unwrap();

    // Insert another vertex with the same name — should fail due to unique constraint
    let duplicate = common::create_person_vertex(2, "Alice", 25);
    let result = storage.insert_vertex("test_space", duplicate);
    assert!(result.is_err(), "Unique index should reject duplicate name");
}

// ── Scenario 11: String VertexId ──

/// Business scenario: User uses string-based vertex IDs instead of integers.
#[test]
fn test_string_vertex_id_operations() {
    use graphdb_storage::core::types::SpaceInfo;

    let mut storage = common::create_in_memory_storage();

    // Create space with String vid type
    let mut space = SpaceInfo::new("str_space".to_string())
        .with_vid_type(graphdb_storage::core::DataType::String)
        .with_comment(Some("string ID space".to_string()));
    storage.create_space(&mut space).unwrap();

    // Create a tag
    let person_tag = graphdb_storage::core::types::TagInfo::new("Person".to_string())
        .with_properties(vec![
            graphdb_storage::core::types::PropertyDef::new(
                "name".to_string(),
                graphdb_storage::core::DataType::String,
            ),
            graphdb_storage::core::types::PropertyDef::new(
                "age".to_string(),
                graphdb_storage::core::DataType::BigInt,
            ),
        ]);
    storage.create_tag("str_space", &person_tag).unwrap();

    // Insert vertices with string IDs
    let alice = Vertex::new(
        VertexId::from_string("user-alice"),
        vec![Tag::new(
            "Person".to_string(),
            vec![
                ("name".to_string(), Value::String("Alice".to_string())),
                ("age".to_string(), Value::BigInt(30)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    let bob = Vertex::new(
        VertexId::from_string("user-bob"),
        vec![Tag::new(
            "Person".to_string(),
            vec![
                ("name".to_string(), Value::String("Bob".to_string())),
                ("age".to_string(), Value::BigInt(25)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    storage.insert_vertex("str_space", alice).unwrap();
    storage.insert_vertex("str_space", bob).unwrap();

    // Verify retrieval
    let alice_retrieved = storage
        .get_vertex("str_space", &VertexId::from_string("user-alice"))
        .unwrap()
        .expect("Alice should exist with string ID");
    assert_eq!(
        alice_retrieved.properties.get("name"),
        Some(&Value::String("Alice".to_string()))
    );

    // Scan all
    let all = storage.scan_vertices("str_space").unwrap();
    assert_eq!(all.len(), 2);
}

// ── Scenario 12: Edge Both Direction Traversal ──

/// Business scenario: User queries edges in both directions.
#[test]
fn test_edge_both_direction_traversal() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);

    let alice = common::create_person_vertex(1, "Alice", 30);
    let bob = common::create_person_vertex(2, "Bob", 25);
    storage.insert_vertex("test_space", alice).unwrap();
    storage.insert_vertex("test_space", bob).unwrap();

    // Alice -> Bob
    let edge = common::create_knows_edge(1, 2, 2020);
    storage.insert_edge("test_space", edge).unwrap();

    // Both direction from Alice: should find 1 edge (Alice -> Bob)
    let both = storage
        .get_node_edges("test_space", &VertexId::from_int64(1), EdgeDirection::Both)
        .unwrap();
    assert_eq!(both.len(), 1, "Both traversal should find 1 edge for Alice");

    // Both direction from Bob: should find 1 edge (Alice -> Bob, incoming)
    let bob_both = storage
        .get_node_edges("test_space", &VertexId::from_int64(2), EdgeDirection::Both)
        .unwrap();
    assert_eq!(
        bob_both.len(),
        1,
        "Both traversal should find 1 incoming edge for Bob"
    );
}

// ── Scenario 13: Update Vertex ──

/// Business scenario: User updates an existing vertex's properties.
#[test]
fn test_update_vertex_properties() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);

    let vertex = common::create_person_vertex(1, "Alice", 30);
    storage.insert_vertex("test_space", vertex).unwrap();

    // Update properties
    let updated = Vertex::new(
        VertexId::from_int64(1),
        vec![Tag::new(
            "Person".to_string(),
            vec![
                (
                    "name".to_string(),
                    Value::String("AliceUpdated".to_string()),
                ),
                ("age".to_string(), Value::BigInt(31)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    storage.update_vertex("test_space", updated).unwrap();

    // Verify update
    let retrieved = storage
        .get_vertex("test_space", &VertexId::from_int64(1))
        .unwrap()
        .expect("Vertex should exist after update");
    assert_eq!(
        retrieved.properties.get("name"),
        Some(&Value::String("AliceUpdated".to_string()))
    );
    assert_eq!(retrieved.properties.get("age"), Some(&Value::BigInt(31)));
}
