use super::*;

#[test]
fn test_insert_and_get_vertex() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let vertex = Vertex::new(
        VertexId::from_int64(101),
        vec![graphdb_core::vertex_edge_path::Tag::new(
            "Person".to_string(),
            vec![
                ("name".to_string(), Value::string("Alice")),
                ("age".to_string(), Value::BigInt(30)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    let vid = storage.insert_vertex("test_space", vertex).unwrap();
    assert_eq!(vid, VertexId::from_int64(101));

    let retrieved = storage
        .get_vertex("test_space", &VertexId::from_int64(101))
        .unwrap();
    assert!(retrieved.is_some());
    let v = retrieved.unwrap();
    assert_eq!(v.properties.get("name"), Some(&Value::string("Alice")));
}

#[test]
fn test_update_vertex() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let index = Index::new(IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 1,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::string(""),
            false,
        )],
        properties: vec![],
        index_type: IndexType::TagIndex,
        is_unique: false,
        covering: false,
        partial_condition: None,
    });
    storage.create_tag_index("test_space", &index).unwrap();

    let vertex = Vertex::new(
        VertexId::from_int64(101),
        vec![graphdb_core::vertex_edge_path::Tag::new(
            "Person".to_string(),
            vec![
                ("name".to_string(), Value::string("Alice")),
                ("age".to_string(), Value::BigInt(30)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    storage.insert_vertex("test_space", vertex).unwrap();

    let before_update = storage
        .lookup_index("test_space", "person_name_idx", &Value::string("Alice"))
        .unwrap();
    assert_eq!(before_update, vec![Value::from(VertexId::from_int64(101))]);

    let updated = Vertex::new(
        VertexId::from_int64(101),
        vec![graphdb_core::vertex_edge_path::Tag::new(
            "Person".to_string(),
            vec![
                ("name".to_string(), Value::string("AliceUpdated")),
                ("age".to_string(), Value::BigInt(31)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    storage.update_vertex("test_space", updated).unwrap();

    let v = storage
        .get_vertex("test_space", &VertexId::from_int64(101))
        .unwrap()
        .unwrap();
    assert_eq!(
        v.properties.get("name"),
        Some(&Value::string("AliceUpdated"))
    );
    assert_eq!(v.properties.get("age"), Some(&Value::BigInt(31)));

    let old_lookup = storage
        .lookup_index("test_space", "person_name_idx", &Value::string("Alice"))
        .unwrap();
    assert!(old_lookup.is_empty());

    let new_lookup = storage
        .lookup_index(
            "test_space",
            "person_name_idx",
            &Value::string("AliceUpdated"),
        )
        .unwrap();
    assert_eq!(new_lookup, vec![Value::from(VertexId::from_int64(101))]);
}

#[test]
fn test_auto_commit_update_rolls_back_before_image_on_abort() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let vertex = Vertex::new(
        VertexId::from_int64(101),
        vec![graphdb_core::vertex_edge_path::Tag::new(
            "Person".to_string(),
            vec![
                ("name".to_string(), Value::string("Alice")),
                ("age".to_string(), Value::BigInt(30)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    storage.insert_vertex("test_space", vertex).unwrap();

    // A failed auto-commit statement must restore the before-image: the
    // in-place property overwrite (age 30 -> 31) has no MVCC version, so
    // aborting the write timestamp alone would leak the new value.
    let mut bound = storage.bind_auto_commit_context().unwrap();
    let updated = Vertex::new(
        VertexId::from_int64(101),
        vec![graphdb_core::vertex_edge_path::Tag::new(
            "Person".to_string(),
            vec![
                ("name".to_string(), Value::string("Alice")),
                ("age".to_string(), Value::BigInt(31)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    bound.update_vertex("test_space", updated).unwrap();
    bound.finalize_operation(false).unwrap();
    drop(bound);

    let v = storage
        .get_vertex("test_space", &VertexId::from_int64(101))
        .unwrap()
        .unwrap();
    assert_eq!(v.properties.get("age"), Some(&Value::BigInt(30)));
    assert_eq!(v.properties.get("name"), Some(&Value::string("Alice")));
}

#[test]
fn test_delete_vertex() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let vertex = Vertex::new(
        VertexId::from_int64(101),
        vec![graphdb_core::vertex_edge_path::Tag::new(
            "Person".to_string(),
            vec![("name".to_string(), Value::string("Alice"))]
                .into_iter()
                .collect(),
        )],
    );
    storage.insert_vertex("test_space", vertex).unwrap();

    storage
        .delete_vertex("test_space", &VertexId::from_int64(101))
        .unwrap();
    assert!(storage
        .get_vertex("test_space", &VertexId::from_int64(101))
        .unwrap()
        .is_none());
}

#[test]
fn test_scan_vertices() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    for i in 1..=5 {
        let vertex = Vertex::new(
            VertexId::from_int64(i),
            vec![graphdb_core::vertex_edge_path::Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("Person{}", i))),
                    ("age".to_string(), Value::BigInt(20 + i)),
                ]
                .into_iter()
                .collect(),
            )],
        );
        storage.insert_vertex("test_space", vertex).unwrap();
    }

    let vertices = storage.scan_vertices("test_space").unwrap();
    assert_eq!(vertices.len(), 5);

    let tagged = storage
        .scan_vertices_by_tag("test_space", "Person")
        .unwrap();
    assert_eq!(tagged.len(), 5);
}

#[test]
fn test_scan_vertices_by_prop() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let vertex = Vertex::new(
        VertexId::from_int64(101),
        vec![graphdb_core::vertex_edge_path::Tag::new(
            "Person".to_string(),
            vec![
                ("name".to_string(), Value::string("Alice")),
                ("age".to_string(), Value::BigInt(30)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    storage.insert_vertex("test_space", vertex).unwrap();

    let results = storage
        .scan_vertices_by_prop("test_space", "Person", "name", &Value::string("Alice"))
        .unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn test_batch_insert_vertices() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let vertices: Vec<Vertex> = (1..=3)
        .map(|i| {
            Vertex::new(
                VertexId::from_int64(i),
                vec![graphdb_core::vertex_edge_path::Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::string(format!("Person{}", i)))]
                        .into_iter()
                        .collect(),
                )],
            )
        })
        .collect();

    let ids = storage
        .batch_insert_vertices("test_space", vertices)
        .unwrap();
    assert_eq!(ids.len(), 3);
}

#[test]
fn test_batch_insert_vertices_rolls_back_on_failure() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let vertices = vec![
        Vertex::new(
            VertexId::from_int64(1),
            vec![graphdb_core::vertex_edge_path::Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::string("Alice"))]
                    .into_iter()
                    .collect(),
            )],
        ),
        Vertex::new(
            VertexId::from_int64(1),
            vec![graphdb_core::vertex_edge_path::Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::string("Duplicate"))]
                    .into_iter()
                    .collect(),
            )],
        ),
    ];

    assert!(storage
        .batch_insert_vertices("test_space", vertices)
        .is_err());
    assert!(storage
        .get_vertex("test_space", &VertexId::from_int64(1))
        .unwrap()
        .is_none());
}

#[test]
fn test_get_vertex_projected() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let vertex = Vertex::new(
        VertexId::from_int64(1),
        vec![Tag::new(
            "Person".to_string(),
            vec![
                ("name".to_string(), Value::string("Alice")),
                ("age".to_string(), Value::BigInt(30)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    storage.insert_vertex("test_space", vertex).unwrap();

    let full = storage
        .get_vertex("test_space", &VertexId::from_int64(1))
        .unwrap()
        .expect("vertex exists");
    assert_eq!(full.properties.len(), 2);

    let projected = storage
        .get_vertex_projected("test_space", &VertexId::from_int64(1), &["age".to_string()])
        .unwrap()
        .expect("vertex exists");
    assert_eq!(projected.properties.len(), 1);
    assert_eq!(projected.properties.get("age"), Some(&Value::BigInt(30)));
    assert!(!projected.properties.contains_key("name"));

    // Full read must not be poisoned by the projected read (cache bypass).
    let full_again = storage
        .get_vertex("test_space", &VertexId::from_int64(1))
        .unwrap()
        .expect("vertex exists");
    assert_eq!(full_again.properties.len(), 2);
}

#[test]
fn test_vertex_idempotent_delete() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let alice = Vertex::new(
        VertexId::from_int64(1),
        vec![Tag::new(
            "Person".to_string(),
            vec![("name".to_string(), Value::string("Alice"))]
                .into_iter()
                .collect(),
        )],
    );
    storage.insert_vertex("test_space", alice).unwrap();

    let result1 = storage.delete_vertex("test_space", &VertexId::from_int64(1));
    assert!(result1.is_ok(), "First delete should succeed");

    let result2 = storage.delete_vertex("test_space", &VertexId::from_int64(1));
    assert!(result2.is_ok(), "Second delete should succeed (idempotent)");

    let result3 = storage.delete_vertex("test_space", &VertexId::from_int64(99999));
    assert!(result3.is_ok(), "Delete non-existent should be idempotent");
}

#[test]
fn test_vertex_with_boundary_properties() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let mut props = std::collections::HashMap::new();
    props.insert("name".to_string(), Value::string("")); // Empty string
    props.insert("age".to_string(), Value::BigInt(i64::MAX)); // Max int

    let vertex = Vertex {
        vid: VertexId::from_int64(1),
        id: 0,
        tags: vec![Tag::new("Person".to_string(), props.clone())],
        properties: props,
    };

    storage.insert_vertex("test_space", vertex).unwrap();

    let retrieved = storage
        .get_vertex("test_space", &VertexId::from_int64(1))
        .unwrap()
        .unwrap();

    assert_eq!(retrieved.properties.get("name"), Some(&Value::string("")));
    assert_eq!(
        retrieved.properties.get("age"),
        Some(&Value::BigInt(i64::MAX))
    );
}
