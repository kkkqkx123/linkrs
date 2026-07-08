//! Full Lifecycle Integration Tests
//!
//! Tests the complete call chain from schema → data → index → persistence,
//! validating that all modules interact correctly. Each test simulates a
//! real business scenario that a user would perform.
//!
//! Test scenarios:
//! 1. Schema → Vertex → Edge lifecycle
//! 2. Post-insert index creation and lookup
//! 3. Index rebuild with existing data
//! 4. Vertex update with index refresh
//! 5. Cascade delete vertex with edges
//! 6. Edge direction traversal (out/in/both)
//! 7. Persistence round-trip with full state

mod common;

use graphdb_storage::core::types::{EdgeTypeInfo, PropertyDef, VertexId};
use graphdb_storage::core::DataType;
use graphdb_storage::core::Value;
use graphdb_storage::storage::{StorageReader, StorageSchemaOps, StorageWriter};

// ── Scenario 1: Schema → Vertex → Edge Full Lifecycle ──

/// Business scenario: User creates a graph space, defines schema,
/// inserts vertices, connects them with edges, then queries everything back.
#[test]
fn test_schema_vertex_edge_full_lifecycle() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);

    // Insert data
    common::insert_test_data(&mut storage, "test_space");
    common::verify_test_data(&storage, "test_space");

    // Space metadata
    assert!(storage.space_exists("test_space"));
    assert_eq!(storage.list_spaces().unwrap().len(), 1);
    assert!(storage.get_tag("test_space", "Person").unwrap().is_some());
    assert!(storage
        .get_edge_type("test_space", "KNOWS")
        .unwrap()
        .is_some());
}

// ── Scenario 2: Index Created AFTER Data Insertion ──

/// Business scenario: User already has data in the graph, then decides
/// to create an index on a property for faster lookups.
/// The index should be populated with existing data after creation.
#[test]
fn test_index_created_after_data_insertion() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);
    common::insert_test_data(&mut storage, "test_space");

    // Create index AFTER data exists (post-insertion)
    common::create_person_name_index(&mut storage, "test_space");

    // Index needs to be rebuilt to pick up existing data
    // (Index metadata is created by create_tag_index, but data entries
    //  are added only on subsequent insert/update. Rebuild scans existing
    //  vertices and populates the index.)
    storage
        .rebuild_tag_index("test_space", "person_name_idx")
        .unwrap();

    // Verify index is populated
    let alice = storage
        .lookup_index(
            "test_space",
            "person_name_idx",
            &Value::String("Alice".to_string()),
        )
        .unwrap();
    assert_eq!(alice, vec![Value::from(VertexId::from_int64(1))]);

    let bob = storage
        .lookup_index(
            "test_space",
            "person_name_idx",
            &Value::String("Bob".to_string()),
        )
        .unwrap();
    assert_eq!(bob, vec![Value::from(VertexId::from_int64(2))]);
}

// ── Scenario 8: Space Isolation ──

/// Business scenario: Multiple spaces should be completely isolated.
/// Operations in one space should not affect another.
#[test]
fn test_space_isolation() {
    let mut storage = common::create_in_memory_storage();

    // Create two spaces
    common::create_space(&mut storage, "alpha");
    common::create_space(&mut storage, "beta");

    // Create Person tag in both spaces (with name+age to match create_person_vertex)
    let person_tag = graphdb_storage::core::types::TagInfo::new("Person".to_string())
        .with_properties(vec![
            PropertyDef::new("name".to_string(), DataType::String),
            PropertyDef::new("age".to_string(), DataType::BigInt),
        ]);
    storage.create_tag("alpha", &person_tag).unwrap();
    storage.create_tag("beta", &person_tag).unwrap();

    // Create KNOWS edge type in alpha only
    let knows = EdgeTypeInfo::new("KNOWS".to_string())
        .with_src_tag("Person".to_string())
        .with_dst_tag("Person".to_string())
        .with_properties(vec![PropertyDef::new("since".to_string(), DataType::Int)]);
    storage.create_edge_type("alpha", &knows).unwrap();

    // Insert vertex in alpha
    let alice = common::create_person_vertex(1, "Alice", 30);
    storage.insert_vertex("alpha", alice).unwrap();

    // Insert same ID in beta with different data
    let bob = common::create_person_vertex(1, "Bob", 25);
    storage.insert_vertex("beta", bob).unwrap();

    // Verify isolation: same ID, different data
    let alpha_vertex = storage
        .get_vertex("alpha", &VertexId::from_int64(1))
        .unwrap()
        .unwrap();
    let beta_vertex = storage
        .get_vertex("beta", &VertexId::from_int64(1))
        .unwrap()
        .unwrap();
    assert_eq!(
        alpha_vertex.properties.get("name"),
        Some(&Value::String("Alice".to_string()))
    );
    assert_eq!(
        beta_vertex.properties.get("name"),
        Some(&Value::String("Bob".to_string()))
    );

    // Edge type exists only in alpha
    assert!(storage.get_edge_type("alpha", "KNOWS").unwrap().is_some());
    assert!(storage.get_edge_type("beta", "KNOWS").unwrap().is_none());
}

// ── Scenario 9: Drop Index Does Not Affect Data ──

/// Business scenario: User drops an index but still wants to access
/// data via full scan (just slower, no data loss).
#[test]
fn test_drop_index_keeps_data_intact() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);
    common::insert_test_data(&mut storage, "test_space");

    // Create and rebuild index
    common::create_person_name_index(&mut storage, "test_space");
    storage
        .rebuild_tag_index("test_space", "person_name_idx")
        .unwrap();
    storage
        .drop_tag_index("test_space", "person_name_idx")
        .unwrap();

    // Data still accessible via scan
    let vertices = storage.scan_vertices("test_space").unwrap();
    assert_eq!(vertices.len(), 2);
    let names: Vec<&str> = vertices
        .iter()
        .filter_map(|v| {
            v.properties.get("name").and_then(|v| match v {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            })
        })
        .collect();
    assert!(names.contains(&"Alice"));
    assert!(names.contains(&"Bob"));
}

// ── Scenario 10: Scan Vertices by Property ──

/// Business scenario: User wants to find all vertices matching
/// a specific property value (full scan with filter).
#[test]
fn test_scan_vertices_by_property_match() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);

    // Insert multiple persons
    for i in 1..=5 {
        let vertex = common::create_person_vertex(i, &format!("Person{}", i), 20 + i);
        storage.insert_vertex("test_space", vertex).unwrap();
    }

    // Scan by matching name
    let result = storage
        .scan_vertices_by_prop(
            "test_space",
            "Person",
            "name",
            &Value::String("Person3".to_string()),
        )
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].properties.get("name"),
        Some(&Value::String("Person3".to_string()))
    );

    // Scan by matching age
    let age_result = storage
        .scan_vertices_by_prop("test_space", "Person", "age", &Value::BigInt(23))
        .unwrap();
    assert_eq!(age_result.len(), 1);

    // Non-existent prop should return empty
    let empty_result = storage
        .scan_vertices_by_prop(
            "test_space",
            "Person",
            "name",
            &Value::String("Nobody".to_string()),
        )
        .unwrap();
    assert!(empty_result.is_empty());
}
