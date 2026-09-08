use super::*;

#[test]
fn test_create_and_list_spaces() {
    let mut storage = create_test_storage();

    let mut space1 = SpaceInfo::new("space1".to_string()).with_vid_type(DataType::BigInt);
    let mut space2 = SpaceInfo::new("space2".to_string()).with_vid_type(DataType::String);
    storage.create_space(&mut space1).unwrap();
    storage.create_space(&mut space2).unwrap();

    let spaces = storage.list_spaces().unwrap();
    assert_eq!(spaces.len(), 2);
    assert!(storage.space_exists("space1"));
    assert!(storage.space_exists("space2"));
    assert!(!storage.space_exists("space3"));

    assert_eq!(storage.get_space_id("space1").unwrap(), 1);
}

#[test]
fn test_drop_space_cleans_tags_and_edge_types() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    storage.drop_space("test_space").unwrap();
    assert!(!storage.space_exists("test_space"));
}

#[test]
fn test_create_and_get_tag() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);

    let tag_id = setup_person_tag(&mut storage);
    assert!(tag_id > 0);

    let tag = storage.get_tag("test_space", "Person").unwrap();
    assert!(tag.is_some());
    assert_eq!(tag.as_ref().unwrap().tag_name, "Person");
    assert_eq!(tag.as_ref().unwrap().properties.len(), 2);

    let tags = storage.list_tags("test_space").unwrap();
    assert_eq!(tags.len(), 1);
}

#[test]
fn test_drop_tag_removes_tag() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    storage.drop_tag("test_space", "Person").unwrap();
    assert!(storage.get_tag("test_space", "Person").unwrap().is_none());
}

#[test]
fn test_create_and_get_edge_type() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let edge_id = setup_knows_edge(&mut storage);
    assert!(edge_id > 0);

    let edge = storage.get_edge_type("test_space", "KNOWS").unwrap();
    assert!(edge.is_some());
    assert_eq!(edge.as_ref().unwrap().edge_type_name, "KNOWS");

    let edges = storage.list_edge_types("test_space").unwrap();
    assert_eq!(edges.len(), 1);
}

#[test]
fn test_same_schema_names_are_isolated_by_space() {
    let mut storage = create_test_storage();
    let mut alpha = SpaceInfo::new("alpha".to_string()).with_vid_type(DataType::BigInt);
    let mut beta = SpaceInfo::new("beta".to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut alpha).unwrap();
    storage.create_space(&mut beta).unwrap();

    let tag = graphdb_core::types::TagInfo::new("Person".to_string())
        .with_properties(vec![PropertyDef::new("name".to_string(), DataType::String)]);
    let alpha_tag_id = storage.create_tag("alpha", &tag).unwrap();
    let beta_tag_id = storage.create_tag("beta", &tag).unwrap();
    assert_ne!(alpha_tag_id, beta_tag_id);

    let edge_type = EdgeTypeInfo::new("KNOWS".to_string())
        .with_src_tag("Person".to_string())
        .with_dst_tag("Person".to_string());
    let alpha_edge_id = storage.create_edge_type("alpha", &edge_type).unwrap();
    let beta_edge_id = storage.create_edge_type("beta", &edge_type).unwrap();
    assert_ne!(alpha_edge_id, beta_edge_id);

    storage
        .insert_vertex(
            "alpha",
            Vertex::new(
                VertexId::from_int64(1),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::string("Alice"))]
                        .into_iter()
                        .collect(),
                )],
            ),
        )
        .unwrap();
    storage
        .insert_vertex(
            "beta",
            Vertex::new(
                VertexId::from_int64(1),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::string("Bob"))]
                        .into_iter()
                        .collect(),
                )],
            ),
        )
        .unwrap();
    storage
        .insert_vertex(
            "alpha",
            Vertex::new(
                VertexId::from_int64(2),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::string("Carol"))]
                        .into_iter()
                        .collect(),
                )],
            ),
        )
        .unwrap();
    storage
        .insert_vertex(
            "beta",
            Vertex::new(
                VertexId::from_int64(2),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::string("Dave"))]
                        .into_iter()
                        .collect(),
                )],
            ),
        )
        .unwrap();

    storage
        .insert_edge(
            "alpha",
            Edge::new(
                VertexId::from_int64(1),
                VertexId::from_int64(2),
                "KNOWS".to_string(),
                0,
                std::collections::HashMap::new(),
            ),
        )
        .unwrap();
    storage
        .insert_edge(
            "beta",
            Edge::new(
                VertexId::from_int64(1),
                VertexId::from_int64(2),
                "KNOWS".to_string(),
                0,
                std::collections::HashMap::new(),
            ),
        )
        .unwrap();

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
        Some(&Value::string("Alice"))
    );
    assert_eq!(
        beta_vertex.properties.get("name"),
        Some(&Value::string("Bob"))
    );

    assert_eq!(
        storage
            .scan_vertices_by_tag("alpha", "Person")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        storage
            .scan_vertices_by_tag("beta", "Person")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        storage.scan_edges_by_type("alpha", "KNOWS").unwrap().len(),
        1
    );
    assert_eq!(
        storage.scan_edges_by_type("beta", "KNOWS").unwrap().len(),
        1
    );
}

#[test]
fn test_drop_edge_type() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    storage.drop_edge_type("test_space", "KNOWS").unwrap();
    assert!(storage
        .get_edge_type("test_space", "KNOWS")
        .unwrap()
        .is_none());
}

#[test]
fn test_schema_wal_replays_create_and_alter_after_restart() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let work_dir = temp_dir.path().to_path_buf();

    {
        let mut storage = GraphStorage::new_with_path(work_dir.clone())
            .expect("Failed to create persistent GraphStorage");
        setup_space(&mut storage);
        storage
            .save_to_disk()
            .expect("Failed to persist base schema");

        let tag =
            graphdb_core::types::TagInfo::new("Person".to_string()).with_properties(vec![
                PropertyDef::new("name".to_string(), DataType::String),
                PropertyDef::new("age".to_string(), DataType::BigInt),
            ]);
        storage
            .create_tag("test_space", &tag)
            .expect("Failed to create tag");

        let edge = EdgeTypeInfo::new("KNOWS".to_string())
            .with_src_tag("Person".to_string())
            .with_dst_tag("Person".to_string())
            .with_properties(vec![PropertyDef::new("since".to_string(), DataType::Int)]);
        storage
            .create_edge_type("test_space", &edge)
            .expect("Failed to create edge type");

        storage
            .alter_tag(
                "test_space",
                "Person",
                vec![PropertyDef::new("email".to_string(), DataType::String)],
                vec!["age".to_string()],
            )
            .expect("Failed to alter tag");
        storage
            .alter_edge_type(
                "test_space",
                "KNOWS",
                vec![PropertyDef::new("weight".to_string(), DataType::Double)],
                vec!["since".to_string()],
            )
            .expect("Failed to alter edge type");

        storage.flush().expect("Failed to sync WAL");
    }

    let storage =
        GraphStorage::open(work_dir).expect("Failed to reopen persistent GraphStorage");

    let tag = storage
        .get_tag("test_space", "Person")
        .expect("Failed to load tag")
        .expect("Tag should exist after recovery");
    let tag_props: Vec<String> = tag
        .properties
        .iter()
        .map(|prop| prop.name.clone())
        .collect();
    assert!(tag_props.contains(&"name".to_string()));
    assert!(tag_props.contains(&"email".to_string()));
    assert!(!tag_props.contains(&"age".to_string()));

    let edge = storage
        .get_edge_type("test_space", "KNOWS")
        .expect("Failed to load edge type")
        .expect("Edge type should exist after recovery");
    let edge_props: Vec<String> = edge
        .properties
        .iter()
        .map(|prop| prop.name.clone())
        .collect();
    assert!(edge_props.contains(&"weight".to_string()));
    assert!(!edge_props.contains(&"since".to_string()));
}

#[test]
fn test_schema_wal_replays_drop_after_restart() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let work_dir = temp_dir.path().to_path_buf();

    {
        let mut storage = GraphStorage::new_with_path(work_dir.clone())
            .expect("Failed to create persistent GraphStorage");
        setup_space(&mut storage);
        storage
            .save_to_disk()
            .expect("Failed to persist base schema");

        setup_person_tag(&mut storage);
        let edge = EdgeTypeInfo::new("KNOWS".to_string())
            .with_src_tag("Person".to_string())
            .with_dst_tag("Person".to_string())
            .with_properties(vec![PropertyDef::new("since".to_string(), DataType::Int)]);
        storage
            .create_edge_type("test_space", &edge)
            .expect("Failed to create edge type");

        storage
            .drop_edge_type("test_space", "KNOWS")
            .expect("Failed to drop edge type");
        storage
            .drop_tag("test_space", "Person")
            .expect("Failed to drop tag");

        storage.flush().expect("Failed to sync WAL");
    }

    let storage =
        GraphStorage::open(work_dir).expect("Failed to reopen persistent GraphStorage");

    assert!(storage
        .get_tag("test_space", "Person")
        .expect("Failed to load tag")
        .is_none());
    assert!(storage
        .get_edge_type("test_space", "KNOWS")
        .expect("Failed to load edge type")
        .is_none());
}

#[test]
fn test_space_wal_replays_create_alter_and_clear_after_restart() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let work_dir = temp_dir.path().to_path_buf();

    {
        let mut storage = GraphStorage::new_with_path(work_dir.clone())
            .expect("Failed to create persistent GraphStorage");
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);
        storage.flush().expect("Failed to sync WAL");
    }

    {
        let mut storage =
            GraphStorage::open(work_dir.clone()).expect("Failed to reopen storage");
        let space_id = storage
            .get_space_id("test_space")
            .expect("space id should exist");

        assert!(storage.space_exists("test_space"));
        assert_eq!(
            storage
                .list_tags("test_space")
                .expect("Failed to list tags")
                .len(),
            1
        );
        assert_eq!(
            storage
                .list_edge_types("test_space")
                .expect("Failed to list edge types")
                .len(),
            1
        );

        storage
            .save_to_disk()
            .expect("Failed to persist recovered schema");

        storage
            .alter_space_comment(space_id, "updated comment".to_string())
            .expect("Failed to alter space comment");
        storage
            .clear_space("test_space")
            .expect("Failed to clear space");
        storage.flush().expect("Failed to sync WAL");
    }

    let storage =
        GraphStorage::open(work_dir).expect("Failed to reopen persistent GraphStorage");

    let space = storage
        .get_space("test_space")
        .expect("Failed to load space")
        .expect("Space should still exist after clear");
    assert_eq!(space.comment, Some("updated comment".to_string()));
    assert_eq!(
        storage
            .list_tags("test_space")
            .expect("Failed to list tags")
            .len(),
        0
    );
    assert_eq!(
        storage
            .list_edge_types("test_space")
            .expect("Failed to list edge types")
            .len(),
        0
    );
}

#[test]
fn test_space_wal_replays_drop_after_restart() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let work_dir = temp_dir.path().to_path_buf();

    {
        let mut storage = GraphStorage::new_with_path(work_dir.clone())
            .expect("Failed to create persistent GraphStorage");
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);
        storage
            .drop_space("test_space")
            .expect("Failed to drop space");
        storage.flush().expect("Failed to sync WAL");
    }

    let storage =
        GraphStorage::open(work_dir).expect("Failed to reopen persistent GraphStorage");

    assert!(!storage.space_exists("test_space"));
    assert!(storage
        .list_spaces()
        .expect("Failed to list spaces")
        .is_empty());
}

#[test]
fn test_create_and_drop_tag_index() {
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

    let indexes = storage.list_tag_indexes("test_space").unwrap();
    assert_eq!(indexes.len(), 1);

    storage
        .drop_tag_index("test_space", "person_name_idx")
        .unwrap();
    let indexes = storage.list_tag_indexes("test_space").unwrap();
    assert_eq!(indexes.len(), 0);
}

#[test]
fn test_snapshot_admin_methods() {
    let (_temp_dir, storage) = create_persistent_storage();

    let initial_stats = storage.snapshot_stats();
    assert_eq!(initial_stats.snapshot_count, 0);
    assert_eq!(initial_stats.total_size_bytes, 0);
    assert_eq!(initial_stats.latest_snapshot_id, None);

    let checkpoint = storage
        .create_checkpoint()
        .expect("checkpoint should succeed")
        .expect("persistence should be enabled");

    assert!(checkpoint.snapshot_created);
    assert!(storage
        .verify_snapshot(checkpoint.checkpoint_id)
        .expect("snapshot verification should succeed"));

    let stats = storage.snapshot_stats();
    assert_eq!(stats.snapshot_count, 1);
    assert_eq!(stats.latest_snapshot_id, Some(checkpoint.checkpoint_id));

    let deleted = storage
        .cleanup_snapshots()
        .expect("snapshot cleanup should succeed");
    assert_eq!(deleted, 0);
}
