use std::collections::HashMap;
use std::path::Path;

use graphdb_storage::core::types::{
    EdgeTypeInfo, Index, IndexConfig, IndexField, IndexType, PropertyDef, SpaceInfo, VertexId,
};
use graphdb_storage::core::vertex_edge_path::Tag;
use graphdb_storage::core::DataType;
use graphdb_storage::core::{Edge, Value, Vertex};
use graphdb_storage::storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};

/// Create a new in-memory storage for integration testing.
#[allow(dead_code)]
pub fn create_in_memory_storage() -> GraphStorage {
    GraphStorage::new().expect("Failed to create in-memory GraphStorage")
}

/// Create a persistent storage at the given path.
#[allow(dead_code)]
pub fn create_persistent_storage(path: &Path) -> GraphStorage {
    GraphStorage::new_with_path(path.to_path_buf())
        .expect("Failed to create persistent GraphStorage")
}

/// Open a previously persisted storage.
#[allow(dead_code)]
pub fn open_persistent_storage(path: &Path) -> GraphStorage {
    GraphStorage::open(path.to_path_buf()).expect("Failed to open persistent GraphStorage")
}

/// Create test space with BigInt vid type.
pub fn create_space(storage: &mut GraphStorage, name: &str) -> u64 {
    let mut space = SpaceInfo::new(name.to_string())
        .with_vid_type(DataType::BigInt)
        .with_comment(Some("integration test space".to_string()));
    storage.create_space(&mut space).unwrap();
    storage.get_space_id(name).unwrap()
}

/// Create a Person tag with name and age properties.
pub fn create_person_tag(storage: &mut GraphStorage, space: &str) -> u32 {
    let tag =
        graphdb_storage::core::types::TagInfo::new("Person".to_string()).with_properties(vec![
            PropertyDef::new("name".to_string(), DataType::String),
            PropertyDef::new("age".to_string(), DataType::BigInt),
        ]);
    storage
        .create_tag(space, &tag)
        .expect("Failed to create Person tag")
}

/// Create an Employee tag with company and salary properties.
#[allow(dead_code)]
pub fn create_employee_tag(storage: &mut GraphStorage, space: &str) -> u32 {
    let tag =
        graphdb_storage::core::types::TagInfo::new("Employee".to_string()).with_properties(vec![
            PropertyDef::new("company".to_string(), DataType::String),
            PropertyDef::new("salary".to_string(), DataType::BigInt),
        ]);
    storage
        .create_tag(space, &tag)
        .expect("Failed to create Employee tag")
}

/// Create a KNOWS edge type with since property.
pub fn create_knows_edge_type(storage: &mut GraphStorage, space: &str) -> u32 {
    let edge = EdgeTypeInfo::new("KNOWS".to_string())
        .with_src_tag("Person".to_string())
        .with_dst_tag("Person".to_string())
        .with_properties(vec![PropertyDef::new("since".to_string(), DataType::Int)]);
    storage
        .create_edge_type(space, &edge)
        .expect("Failed to create KNOWS edge type")
}

/// Create a WORKS_AT edge type with role property.
#[allow(dead_code)]
pub fn create_works_at_edge_type(storage: &mut GraphStorage, space: &str) -> u32 {
    let edge = EdgeTypeInfo::new("WORKS_AT".to_string())
        .with_src_tag("Person".to_string())
        .with_dst_tag("Employee".to_string())
        .with_properties(vec![PropertyDef::new("role".to_string(), DataType::String)]);
    storage
        .create_edge_type(space, &edge)
        .expect("Failed to create WORKS_AT edge type")
}

/// Create a person vertex with name and age.
pub fn create_person_vertex(id: i64, name: &str, age: i64) -> Vertex {
    Vertex::new(
        VertexId::from_int64(id),
        vec![Tag::new(
            "Person".to_string(),
            vec![
                ("name".to_string(), Value::String(name.to_string())),
                ("age".to_string(), Value::BigInt(age)),
            ]
            .into_iter()
            .collect(),
        )],
    )
}

/// Create a person+employee vertex with both tags.
#[allow(dead_code)]
pub fn create_multi_tag_vertex(
    id: i64,
    name: &str,
    age: i64,
    company: &str,
    salary: i64,
) -> Vertex {
    let mut props: HashMap<String, Value> = HashMap::new();
    props.insert("name".to_string(), Value::String(name.to_string()));
    props.insert("age".to_string(), Value::BigInt(age));
    props.insert("company".to_string(), Value::String(company.to_string()));
    props.insert("salary".to_string(), Value::BigInt(salary));

    Vertex {
        vid: VertexId::from_int64(id),
        id: 0,
        tags: vec![
            Tag::new("Person".to_string(), {
                let mut p = HashMap::new();
                p.insert("name".to_string(), Value::String(name.to_string()));
                p.insert("age".to_string(), Value::BigInt(age));
                p
            }),
            Tag::new("Employee".to_string(), {
                let mut p = HashMap::new();
                p.insert("company".to_string(), Value::String(company.to_string()));
                p.insert("salary".to_string(), Value::BigInt(salary));
                p
            }),
        ],
        properties: props,
    }
}

/// Create a KNOWS edge between two vertices.
#[allow(dead_code)]
pub fn create_knows_edge(src: i64, dst: i64, since: i32) -> Edge {
    Edge::new(
        VertexId::from_int64(src),
        VertexId::from_int64(dst),
        "KNOWS".to_string(),
        0,
        vec![("since".to_string(), Value::Int(since))]
            .into_iter()
            .collect(),
    )
}

/// Setup basic schema and return space_id.
#[allow(dead_code)]
pub fn setup_basic_schema(storage: &mut GraphStorage) -> u64 {
    let space_id = create_space(storage, "test_space");
    create_person_tag(storage, "test_space");
    create_knows_edge_type(storage, "test_space");
    space_id
}

/// Setup schema with Person+Employee+KNOWS+WORKS_AT for multi-tag scenarios.
#[allow(dead_code)]
pub fn setup_multi_tag_schema(storage: &mut GraphStorage) -> u64 {
    let space_id = create_space(storage, "test_space");
    create_person_tag(storage, "test_space");
    create_employee_tag(storage, "test_space");
    create_knows_edge_type(storage, "test_space");
    create_works_at_edge_type(storage, "test_space");
    space_id
}

/// Create a name index on Person tag.
#[allow(dead_code)]
pub fn create_person_name_index(storage: &mut GraphStorage, space: &str) {
    let index = Index::new(IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String(String::new()),
            false,
        )],
        properties: vec![],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });
    storage
        .create_tag_index(space, &index)
        .expect("Failed to create person name index");
}

/// Insert test data: Alice (30) and Bob (25) with a KNOWS edge.
#[allow(dead_code)]
pub fn insert_test_data(storage: &mut GraphStorage, space: &str) {
    let alice = create_person_vertex(1, "Alice", 30);
    let bob = create_person_vertex(2, "Bob", 25);
    storage.insert_vertex(space, alice).unwrap();
    storage.insert_vertex(space, bob).unwrap();

    let edge = create_knows_edge(1, 2, 2020);
    storage.insert_edge(space, edge).unwrap();
}

/// Verify test data integrity.
#[allow(dead_code)]
pub fn verify_test_data(storage: &GraphStorage, space: &str) {
    let alice = storage
        .get_vertex(space, &VertexId::from_int64(1))
        .unwrap()
        .expect("Alice should exist");
    assert_eq!(
        alice.properties.get("name"),
        Some(&Value::String("Alice".to_string()))
    );
    assert_eq!(alice.properties.get("age"), Some(&Value::BigInt(30)));

    let bob = storage
        .get_vertex(space, &VertexId::from_int64(2))
        .unwrap()
        .expect("Bob should exist");
    assert_eq!(
        bob.properties.get("name"),
        Some(&Value::String("Bob".to_string()))
    );

    let edge = storage
        .get_edge(
            space,
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .unwrap()
        .expect("Edge should exist");
    assert_eq!(edge.src, VertexId::from_int64(1));
    assert_eq!(edge.dst, VertexId::from_int64(2));
}
