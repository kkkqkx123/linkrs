use super::*;

#[test]
fn test_serial_auto_allocates_when_column_missing() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_serial_person_tag(&mut storage);

    insert_serial_vertex(&mut storage, 101, "Alice");
    insert_serial_vertex(&mut storage, 102, "Bob");

    let alice = storage
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(101).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(alice.property_value("id"), Some(Value::BigInt(1)));
    let bob = storage
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(102).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(bob.property_value("id"), Some(Value::BigInt(2)));
}

#[test]
fn test_serial_explicit_value_advances_counter() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_serial_person_tag(&mut storage);

    // Explicit id=5 must be accepted and push the counter past 5.
    let explicit = Vertex::new(
        VertexId::try_from_int64(101).expect("test vertex id"),
        Tag::new(
            "Person".to_string(),
            vec![
                ("id".to_string(), Value::BigInt(5)),
                ("name".to_string(), Value::string("Alice")),
            ]
            .into_iter()
            .collect(),
        ),
    );
    storage.insert_vertex("test_space", explicit).unwrap();

    // The next auto allocation must be 6, not 1.
    insert_serial_vertex(&mut storage, 102, "Bob");
    let bob = storage
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(102).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(bob.property_value("id"), Some(Value::BigInt(6)));
}

#[test]
fn test_serial_explicit_duplicate_is_rejected() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_serial_person_tag(&mut storage);

    insert_serial_vertex(&mut storage, 101, "Alice");

    // Re-inserting the already-allocated value must fail.
    let duplicate = Vertex::new(
        VertexId::try_from_int64(102).expect("test vertex id"),
        Tag::new(
            "Person".to_string(),
            vec![
                ("id".to_string(), Value::BigInt(1)),
                ("name".to_string(), Value::string("Bob")),
            ]
            .into_iter()
            .collect(),
        ),
    );
    let error = storage.insert_vertex("test_space", duplicate).unwrap_err();
    assert!(
        error.to_string().contains("SERIAL"),
        "unexpected error: {}",
        error
    );
}

#[test]
fn test_serial_allocates_per_tag_and_per_space() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_serial_person_tag(&mut storage);

    let city = graphdb_core::types::TagInfo::new("City".to_string()).with_properties(vec![
        PropertyDef::new("vid".to_string(), DataType::BigInt),
        PropertyDef::new("id".to_string(), DataType::BigInt).with_serial(true),
        PropertyDef::new("name".to_string(), DataType::String),
    ]);
    storage
        .create_tag("test_space", &city)
        .expect("Failed to create City tag");

    let mut second_space =
        SpaceInfo::new("second_space".to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut second_space).unwrap();

    let second_person =
        graphdb_core::types::TagInfo::new("Person".to_string()).with_properties(vec![
            PropertyDef::new("vid".to_string(), DataType::BigInt),
            PropertyDef::new("id".to_string(), DataType::BigInt).with_serial(true),
            PropertyDef::new("name".to_string(), DataType::String),
        ]);
    storage
        .create_tag("second_space", &second_person)
        .expect("Failed to create second-space Person tag");

    // Person in space 1 -> id 1.
    insert_serial_vertex(&mut storage, 101, "Alice");
    // Person in space 2 -> id 1 (counters are per space).
    let vertex = Vertex::new(
        VertexId::try_from_int64(101).expect("test vertex id"),
        Tag::new(
            "Person".to_string(),
            vec![("name".to_string(), Value::string("Bob"))]
                .into_iter()
                .collect(),
        ),
    );
    storage.insert_vertex("second_space", vertex).unwrap();
    // City in space 1 -> id 1 (counters are per table).
    let city_vertex = Vertex::new(
        VertexId::try_from_int64(201).expect("test vertex id"),
        Tag::new(
            "City".to_string(),
            vec![("name".to_string(), Value::string("Paris"))]
                .into_iter()
                .collect(),
        ),
    );
    storage.insert_vertex("test_space", city_vertex).unwrap();

    let alice = storage
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(101).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(alice.property_value("id"), Some(Value::BigInt(1)));
    let bob = storage
        .get_vertex(
            "second_space",
            "Person",
            &VertexId::try_from_int64(101).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(bob.property_value("id"), Some(Value::BigInt(1)));
    let paris = storage
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(201).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(paris.property_value("id"), Some(Value::BigInt(1)));
}

#[test]
fn test_serial_edge_type_auto_allocates() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let edge_type =
        graphdb_core::types::EdgeTypeInfo::new("KNOWS".to_string()).with_properties(vec![
            PropertyDef::new("seq".to_string(), DataType::BigInt).with_serial(true),
        ]);
    storage
        .create_edge_type("test_space", &edge_type)
        .expect("Failed to create edge type");

    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");

    let edge = Edge::new(
        VertexId::try_from_int64(1).expect("test vertex id"),
        VertexId::try_from_int64(2).expect("test vertex id"),
        "KNOWS".to_string(),
        0,
        std::collections::HashMap::new(),
    );
    storage.insert_edge("test_space", edge).unwrap();

    let retrieved = storage
        .get_edge(
            "test_space",
            &VertexId::try_from_int64(1).expect("test vertex id"),
            &VertexId::try_from_int64(2).expect("test vertex id"),
            "KNOWS",
            0,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        retrieved.props.get("seq"),
        Some(&Value::BigInt(1)),
        "edge serial column must be auto-allocated"
    );
}

#[test]
fn test_serial_validation_rejects_default_and_multiple_columns() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);

    let with_default =
        graphdb_core::types::TagInfo::new("BadDefault".to_string()).with_properties(vec![
            PropertyDef::new("id".to_string(), DataType::BigInt)
                .with_serial(true)
                .with_default(Some(Value::BigInt(1))),
        ]);
    let error = storage
        .create_tag("test_space", &with_default)
        .expect_err("SERIAL with DEFAULT must be rejected");
    assert!(error.to_string().contains("DEFAULT"));

    let two_serials =
        graphdb_core::types::TagInfo::new("BadMultiple".to_string()).with_properties(vec![
            PropertyDef::new("id".to_string(), DataType::BigInt).with_serial(true),
            PropertyDef::new("seq".to_string(), DataType::BigInt).with_serial(true),
        ]);
    let error = storage
        .create_tag("test_space", &two_serials)
        .expect_err("two SERIAL columns must be rejected");
    assert!(error.to_string().contains("only one SERIAL"));
}

#[test]
fn test_serial_survives_save_load_round_trip() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let mut storage = GraphStorage::new_with_path(temp_dir.path().to_path_buf())
        .expect("Failed to create persistent storage");
    {
        let mut space = SpaceInfo::new("test_space".to_string()).with_vid_type(DataType::BigInt);
        storage.create_space(&mut space).unwrap();
        setup_serial_person_tag(&mut storage);
        insert_serial_vertex(&mut storage, 101, "Alice");
        insert_serial_vertex(&mut storage, 102, "Bob");
    }
    storage.save_to_disk().expect("save to disk");
    drop(storage);

    let mut reloaded =
        GraphStorage::new_with_path(temp_dir.path().to_path_buf()).expect("reload storage");
    reloaded.load_from_disk().expect("load from disk");

    // The counter continues after the persisted high water mark.
    insert_serial_vertex(&mut reloaded, 103, "Carol");
    let carol = reloaded
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(103).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(carol.property_value("id"), Some(Value::BigInt(3)));

    // Deleted rows must not bring the counter back down after reload.
    reloaded
        .delete_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(102).expect("test vertex id"),
        )
        .expect("delete vertex");
    reloaded.save_to_disk().expect("save to disk");
    drop(reloaded);

    let mut reloaded2 =
        GraphStorage::new_with_path(temp_dir.path().to_path_buf()).expect("reload storage");
    reloaded2.load_from_disk().expect("load from disk");
    insert_serial_vertex(&mut reloaded2, 104, "Dave");
    let dave = reloaded2
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(104).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        dave.property_value("id"),
        Some(Value::BigInt(4)),
        "counter must not fall back after row deletion"
    );
}
