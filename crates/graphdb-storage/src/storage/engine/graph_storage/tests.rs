#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::core::types::{
        EdgeTypeInfo, Index, IndexConfig, IndexField, IndexType, PropertyDef, SpaceInfo, UserInfo,
        VertexId,
    };
    use crate::core::vertex_edge_path::Tag;
    use crate::core::DataType;
    use crate::core::{Edge, EdgeDirection, RoleType, Value, Vertex};
    use crate::storage::{
        GraphStorage, StorageAdmin, StorageAuthOps, StoragePersistenceOps, StorageReader,
        StorageSchemaOps, StorageWriter,
    };

    fn create_test_storage() -> GraphStorage {
        GraphStorage::new().expect("Failed to create GraphStorage")
    }

    fn create_persistent_storage() -> (tempfile::TempDir, GraphStorage) {
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let storage = GraphStorage::new_with_path(temp_dir.path().to_path_buf())
            .expect("Failed to create persistent GraphStorage");
        (temp_dir, storage)
    }

    fn setup_space(storage: &mut GraphStorage) -> u64 {
        let mut space = SpaceInfo::new("test_space".to_string())
            .with_vid_type(DataType::BigInt)
            .with_comment(Some("test".to_string()));
        storage.create_space(&mut space).unwrap();
        storage.get_space_id("test_space").unwrap()
    }

    fn setup_person_tag(storage: &mut GraphStorage) -> u32 {
        let tag = crate::core::types::TagInfo::new("Person".to_string()).with_properties(vec![
            PropertyDef::new("name".to_string(), DataType::String),
            PropertyDef::new("age".to_string(), DataType::BigInt),
        ]);
        storage
            .create_tag("test_space", &tag)
            .expect("Failed to create tag")
    }

    fn setup_knows_edge(storage: &mut GraphStorage) -> u32 {
        let edge = EdgeTypeInfo::new("KNOWS".to_string())
            .with_properties(vec![PropertyDef::new("since".to_string(), DataType::Int)]);
        storage
            .create_edge_type("test_space", &edge)
            .expect("Failed to create edge type")
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

    // ==================== Schema Operations ====================

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

        let tag = crate::core::types::TagInfo::new("Person".to_string())
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
                        vec![("name".to_string(), Value::String("Alice".to_string()))]
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
                        vec![("name".to_string(), Value::String("Bob".to_string()))]
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
                        vec![("name".to_string(), Value::String("Carol".to_string()))]
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
                        vec![("name".to_string(), Value::String("Dave".to_string()))]
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
            Some(&Value::String("Alice".to_string()))
        );
        assert_eq!(
            beta_vertex.properties.get("name"),
            Some(&Value::String("Bob".to_string()))
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

            let tag = crate::core::types::TagInfo::new("Person".to_string()).with_properties(vec![
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
        storage.create_tag_index("test_space", &index).unwrap();

        let indexes = storage.list_tag_indexes("test_space").unwrap();
        assert_eq!(indexes.len(), 1);

        storage
            .drop_tag_index("test_space", "person_name_idx")
            .unwrap();
        let indexes = storage.list_tag_indexes("test_space").unwrap();
        assert_eq!(indexes.len(), 0);
    }

    // ==================== Vertex Operations ====================

    #[test]
    fn test_insert_and_get_vertex() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let vertex = Vertex::new(
            VertexId::from_int64(101),
            vec![crate::core::vertex_edge_path::Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::String("Alice".to_string())),
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
        assert_eq!(
            v.properties.get("name"),
            Some(&Value::String("Alice".to_string()))
        );
    }

    #[test]
    fn test_update_vertex() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

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
        storage.create_tag_index("test_space", &index).unwrap();

        let vertex = Vertex::new(
            VertexId::from_int64(101),
            vec![crate::core::vertex_edge_path::Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::BigInt(30)),
                ]
                .into_iter()
                .collect(),
            )],
        );
        storage.insert_vertex("test_space", vertex).unwrap();

        let before_update = storage
            .lookup_index(
                "test_space",
                "person_name_idx",
                &Value::String("Alice".to_string()),
            )
            .unwrap();
        assert_eq!(before_update, vec![Value::from(VertexId::from_int64(101))]);

        let updated = Vertex::new(
            VertexId::from_int64(101),
            vec![crate::core::vertex_edge_path::Tag::new(
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

        let v = storage
            .get_vertex("test_space", &VertexId::from_int64(101))
            .unwrap()
            .unwrap();
        assert_eq!(
            v.properties.get("name"),
            Some(&Value::String("AliceUpdated".to_string()))
        );
        assert_eq!(v.properties.get("age"), Some(&Value::BigInt(31)));

        let old_lookup = storage
            .lookup_index(
                "test_space",
                "person_name_idx",
                &Value::String("Alice".to_string()),
            )
            .unwrap();
        assert!(old_lookup.is_empty());

        let new_lookup = storage
            .lookup_index(
                "test_space",
                "person_name_idx",
                &Value::String("AliceUpdated".to_string()),
            )
            .unwrap();
        assert_eq!(new_lookup, vec![Value::from(VertexId::from_int64(101))]);
    }

    #[test]
    fn test_delete_vertex() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let vertex = Vertex::new(
            VertexId::from_int64(101),
            vec![crate::core::vertex_edge_path::Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::String("Alice".to_string()))]
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
                vec![crate::core::vertex_edge_path::Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::String(format!("Person{}", i))),
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
            vec![crate::core::vertex_edge_path::Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::BigInt(30)),
                ]
                .into_iter()
                .collect(),
            )],
        );
        storage.insert_vertex("test_space", vertex).unwrap();

        let results = storage
            .scan_vertices_by_prop(
                "test_space",
                "Person",
                "name",
                &Value::String("Alice".to_string()),
            )
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
                    vec![crate::core::vertex_edge_path::Tag::new(
                        "Person".to_string(),
                        vec![("name".to_string(), Value::String(format!("Person{}", i)))]
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
                vec![crate::core::vertex_edge_path::Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::String("Alice".to_string()))]
                        .into_iter()
                        .collect(),
                )],
            ),
            Vertex::new(
                VertexId::from_int64(1),
                vec![crate::core::vertex_edge_path::Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::String("Duplicate".to_string()))]
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

    // ==================== Edge Operations ====================

    fn insert_test_vertex(storage: &mut GraphStorage, id: i64, name: &str) {
        let vertex = Vertex::new(
            VertexId::from_int64(id),
            vec![crate::core::vertex_edge_path::Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::String(name.to_string()))]
                    .into_iter()
                    .collect(),
            )],
        );
        storage.insert_vertex("test_space", vertex).unwrap();
    }

    #[test]
    fn test_insert_and_get_edge() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);

        insert_test_vertex(&mut storage, 1, "Alice");
        insert_test_vertex(&mut storage, 2, "Bob");

        let edge = Edge::new(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "KNOWS".to_string(),
            0,
            vec![("since".to_string(), Value::Int(2020))]
                .into_iter()
                .collect(),
        );
        storage.insert_edge("test_space", edge).unwrap();

        let retrieved = storage
            .get_edge(
                "test_space",
                &VertexId::from_int64(1),
                &VertexId::from_int64(2),
                "KNOWS",
                0,
            )
            .unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.as_ref().unwrap().src, VertexId::from_int64(1));
        assert_eq!(retrieved.as_ref().unwrap().dst, VertexId::from_int64(2));
    }

    #[test]
    fn test_delete_edge() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);

        insert_test_vertex(&mut storage, 1, "Alice");
        insert_test_vertex(&mut storage, 2, "Bob");

        let edge = Edge::new(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        );
        storage.insert_edge("test_space", edge).unwrap();

        storage
            .delete_edge(
                "test_space",
                &VertexId::from_int64(1),
                &VertexId::from_int64(2),
                "KNOWS",
                0,
            )
            .unwrap();

        let retrieved = storage
            .get_edge(
                "test_space",
                &VertexId::from_int64(1),
                &VertexId::from_int64(2),
                "KNOWS",
                0,
            )
            .unwrap();
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_get_node_edges() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);

        insert_test_vertex(&mut storage, 1, "Alice");
        insert_test_vertex(&mut storage, 2, "Bob");
        insert_test_vertex(&mut storage, 3, "Charlie");

        for dst in &[2i64, 3] {
            let edge = Edge::new(
                VertexId::from_int64(1),
                VertexId::from_int64(*dst),
                "KNOWS".to_string(),
                0,
                std::collections::HashMap::new(),
            );
            storage.insert_edge("test_space", edge).unwrap();
        }

        let out_edges = storage
            .get_node_edges("test_space", &VertexId::from_int64(1), EdgeDirection::Out)
            .unwrap();
        assert_eq!(out_edges.len(), 2);

        let in_edges = storage
            .get_node_edges("test_space", &VertexId::from_int64(2), EdgeDirection::In)
            .unwrap();
        assert_eq!(in_edges.len(), 1);
    }

    #[test]
    fn test_scan_edges_by_type() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);

        insert_test_vertex(&mut storage, 1, "Alice");
        insert_test_vertex(&mut storage, 2, "Bob");

        let edge = Edge::new(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        );
        storage.insert_edge("test_space", edge).unwrap();

        let edges = storage.scan_edges_by_type("test_space", "KNOWS").unwrap();
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn test_batch_insert_edges_rolls_back_on_failure() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);

        insert_test_vertex(&mut storage, 1, "Alice");
        insert_test_vertex(&mut storage, 2, "Bob");

        let edges = vec![
            Edge::new(
                VertexId::from_int64(1),
                VertexId::from_int64(2),
                "KNOWS".to_string(),
                0,
                std::collections::HashMap::new(),
            ),
            Edge::new(
                VertexId::from_int64(1),
                VertexId::from_int64(3),
                "KNOWS".to_string(),
                0,
                std::collections::HashMap::new(),
            ),
        ];

        assert!(storage.batch_insert_edges("test_space", edges).is_err());
        assert_eq!(
            storage
                .scan_edges_by_type("test_space", "KNOWS")
                .unwrap()
                .len(),
            0
        );
    }

    // ==================== User / Auth Operations ====================

    #[test]
    fn test_create_and_drop_user() {
        let mut storage = create_test_storage();

        let user = UserInfo::new("test_user".to_string(), "password123".to_string()).unwrap();
        storage.create_user(&user).unwrap();

        storage.drop_user("test_user").unwrap();
    }

    #[test]
    fn test_grant_and_revoke_role() {
        let mut storage = create_test_storage();
        let space_id = setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let user = UserInfo::new("role_user".to_string(), "pass".to_string()).unwrap();
        storage.create_user(&user).unwrap();

        storage
            .grant_role("role_user", space_id, RoleType::Admin)
            .unwrap();
        storage.revoke_role("role_user", space_id).unwrap();

        storage.drop_user("role_user").unwrap();
    }

    #[test]
    fn test_user_storage_persists_across_reload() {
        let (temp_dir, mut storage) = create_persistent_storage();

        let user = UserInfo::new("persist_user".to_string(), "password123".to_string())
            .expect("UserInfo::new should succeed")
            .with_locked(true)
            .with_max_queries_per_hour(42);

        storage.create_user(&user).unwrap();
        storage.save_to_disk().unwrap();

        let mut reloaded = GraphStorage::open(temp_dir.path().to_path_buf())
            .expect("Failed to reopen GraphStorage");

        assert!(reloaded.user_exists("persist_user"));
        assert!(reloaded.create_user(&user).unwrap());
    }

    // ==================== Storage Admin Operations ====================

    #[test]
    fn test_get_storage_stats_empty() {
        let storage = create_test_storage();
        let stats = storage.get_storage_stats();
        assert_eq!(stats.total_vertices, 0);
        assert_eq!(stats.total_edges, 0);
        assert_eq!(stats.total_spaces, 0);
    }

    #[test]
    fn test_get_storage_stats_with_data() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);

        insert_test_vertex(&mut storage, 1, "Alice");
        insert_test_vertex(&mut storage, 2, "Bob");

        let edge = Edge::new(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        );
        storage.insert_edge("test_space", edge).unwrap();

        let stats = storage.get_storage_stats();
        // Note: vertex/edge counts depend on MVCC visibility
        assert!(stats.total_spaces >= 1);
        assert!(stats.total_tags >= 1);
        assert!(stats.total_edge_types >= 1);
    }

    #[test]
    fn test_get_db_path() {
        let storage = create_test_storage();
        // Default db_path is empty for new() without path
        let path = storage.get_db_path();
        assert!(path.is_empty() || path.contains("test"));
    }

    // ==================== Edge Case Tests ====================

    #[test]
    fn test_get_nonexistent_vertex() {
        let storage = create_test_storage();
        let result = storage.get_vertex("nonexistent", &VertexId::from_int64(999));
        assert!(result.is_err());
    }

    #[test]
    fn test_get_nonexistent_edge() {
        let storage = create_test_storage();
        let result = storage.get_edge(
            "nonexistent",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "UNKNOWN",
            0,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_nonexistent_vertex() {
        let mut storage = create_test_storage();
        let result = storage.delete_vertex("nonexistent", &VertexId::from_int64(999));
        assert!(result.is_err());
    }

    // ==================== String ID Edge Tests ====================

    fn setup_string_id_space(storage: &mut GraphStorage) {
        let mut space = SpaceInfo::new("str_space".to_string()).with_vid_type(DataType::String);
        storage.create_space(&mut space).unwrap();

        let tag = crate::core::types::TagInfo::new("Node".to_string())
            .with_properties(vec![PropertyDef::new("name".to_string(), DataType::String)]);
        storage.create_tag("str_space", &tag).unwrap();

        let edge = EdgeTypeInfo::new("LINK".to_string());
        storage.create_edge_type("str_space", &edge).unwrap();
    }

    #[test]
    fn test_string_id_get_node_edges_in() {
        let mut storage = create_test_storage();
        setup_string_id_space(&mut storage);

        let v1 = Vertex::new(
            VertexId::from_string("a"),
            vec![Tag::new(
                "Node".to_string(),
                vec![("name".to_string(), Value::String("A".to_string()))]
                    .into_iter()
                    .collect(),
            )],
        );
        let v2 = Vertex::new(
            VertexId::from_string("b"),
            vec![Tag::new(
                "Node".to_string(),
                vec![("name".to_string(), Value::String("B".to_string()))]
                    .into_iter()
                    .collect(),
            )],
        );
        let v3 = Vertex::new(
            VertexId::from_string("c"),
            vec![Tag::new(
                "Node".to_string(),
                vec![("name".to_string(), Value::String("C".to_string()))]
                    .into_iter()
                    .collect(),
            )],
        );
        storage.insert_vertex("str_space", v1).unwrap();
        storage.insert_vertex("str_space", v2).unwrap();
        storage.insert_vertex("str_space", v3).unwrap();

        let edge1 = Edge::new(
            VertexId::from_string("b"),
            VertexId::from_string("a"),
            "LINK".to_string(),
            0,
            std::collections::HashMap::new(),
        );
        let edge2 = Edge::new(
            VertexId::from_string("c"),
            VertexId::from_string("a"),
            "LINK".to_string(),
            0,
            std::collections::HashMap::new(),
        );
        storage.insert_edge("str_space", edge1).unwrap();
        storage.insert_edge("str_space", edge2).unwrap();

        let in_edges = storage
            .get_node_edges("str_space", &VertexId::from_string("a"), EdgeDirection::In)
            .unwrap();
        assert_eq!(in_edges.len(), 2, "Node 'a' should have 2 incoming edges");

        for edge in &in_edges {
            assert_eq!(edge.dst, VertexId::from_string("a"), "dst should be 'a'");
            assert!(
                edge.src == VertexId::from_string("b") || edge.src == VertexId::from_string("c"),
                "src should be 'b' or 'c', got {:?}",
                edge.src
            );
        }
    }

    #[test]
    fn test_string_id_get_node_edges_out() {
        let mut storage = create_test_storage();
        setup_string_id_space(&mut storage);

        let v1 = Vertex::new(
            VertexId::from_string("a"),
            vec![Tag::new(
                "Node".to_string(),
                vec![("name".to_string(), Value::String("A".to_string()))]
                    .into_iter()
                    .collect(),
            )],
        );
        let v2 = Vertex::new(
            VertexId::from_string("b"),
            vec![Tag::new(
                "Node".to_string(),
                vec![("name".to_string(), Value::String("B".to_string()))]
                    .into_iter()
                    .collect(),
            )],
        );
        storage.insert_vertex("str_space", v1).unwrap();
        storage.insert_vertex("str_space", v2).unwrap();

        let edge = Edge::new(
            VertexId::from_string("a"),
            VertexId::from_string("b"),
            "LINK".to_string(),
            0,
            std::collections::HashMap::new(),
        );
        storage.insert_edge("str_space", edge).unwrap();

        let out_edges = storage
            .get_node_edges("str_space", &VertexId::from_string("a"), EdgeDirection::Out)
            .unwrap();
        assert_eq!(out_edges.len(), 1, "Node 'a' should have 1 outgoing edge");
        assert_eq!(out_edges[0].src, VertexId::from_string("a"));
        assert_eq!(out_edges[0].dst, VertexId::from_string("b"));
    }

    #[test]
    fn test_string_id_scan_edges_by_type() {
        let mut storage = create_test_storage();
        setup_string_id_space(&mut storage);

        let v1 = Vertex::new(
            VertexId::from_string("x"),
            vec![Tag::new(
                "Node".to_string(),
                vec![("name".to_string(), Value::String("X".to_string()))]
                    .into_iter()
                    .collect(),
            )],
        );
        let v2 = Vertex::new(
            VertexId::from_string("y"),
            vec![Tag::new(
                "Node".to_string(),
                vec![("name".to_string(), Value::String("Y".to_string()))]
                    .into_iter()
                    .collect(),
            )],
        );
        storage.insert_vertex("str_space", v1).unwrap();
        storage.insert_vertex("str_space", v2).unwrap();

        let edge = Edge::new(
            VertexId::from_string("x"),
            VertexId::from_string("y"),
            "LINK".to_string(),
            0,
            std::collections::HashMap::new(),
        );
        storage.insert_edge("str_space", edge).unwrap();

        let edges = storage.scan_edges_by_type("str_space", "LINK").unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src, VertexId::from_string("x"));
        assert_eq!(edges[0].dst, VertexId::from_string("y"));
    }

    #[test]
    fn test_vertex_idempotent_delete() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        // Insert test data
        let alice = Vertex::new(
            VertexId::from_int64(1),
            vec![Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::String("Alice".to_string()))].into_iter().collect(),
            )],
        );
        storage.insert_vertex("test_space", alice).unwrap();

        // First deletion should succeed
        let result1 = storage.delete_vertex("test_space", &VertexId::from_int64(1));
        assert!(result1.is_ok(), "First delete should succeed");

        // Second deletion of same vertex should also succeed (idempotent)
        let result2 = storage.delete_vertex("test_space", &VertexId::from_int64(1));
        assert!(result2.is_ok(), "Second delete should succeed (idempotent)");

        // Delete non-existent vertex should not error
        let result3 = storage.delete_vertex("test_space", &VertexId::from_int64(99999));
        assert!(result3.is_ok(), "Delete non-existent should be idempotent");
    }

    #[test]
    fn test_vertex_with_boundary_properties() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        // Create vertex with boundary values
        let mut props = std::collections::HashMap::new();
        props.insert("name".to_string(), Value::String("".to_string())); // Empty string
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

        assert_eq!(
            retrieved.properties.get("name"),
            Some(&Value::String("".to_string()))
        );
        assert_eq!(
            retrieved.properties.get("age"),
            Some(&Value::BigInt(i64::MAX))
        );
    }

    // ==================== Freeze Integration Tests ====================

    #[test]
    fn test_background_freeze_manager_basics() {
        use crate::storage::engine::background_freeze::{BackgroundFreezeManager};
        use crate::storage::engine::config::{FreezeConfig, FreezeDecisionInput, FreezeStrategyType};

        let config = FreezeConfig {
            strategy: FreezeStrategyType::Conservative,
            delta_edge_threshold: 1000,
            delta_memory_threshold_bytes: 256 * 1024 * 1024,
            max_segment_age: u32::MAX,
            deletion_threshold: 0.5,
            max_segment_size_bytes: 8 * 1024 * 1024,
            adaptive_segment_threshold: 50,
            adaptive_maximum_segments: 150,
            lsm_segment_pressure_threshold: 200,
        };
        let manager = BackgroundFreezeManager::from_config(config);

        // Test should_freeze decision (only edge count threshold)
        let input1 = FreezeDecisionInput {
            delta_edge_count: 500,
            delta_memory_bytes: 100 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };
        assert!(!manager.should_freeze_with_stats(&input1));

        let input2 = FreezeDecisionInput {
            delta_edge_count: 1000,
            ..input1
        };
        assert!(manager.should_freeze_with_stats(&input2));

        let input3 = FreezeDecisionInput {
            delta_edge_count: 1500,
            ..input1
        };
        assert!(manager.should_freeze_with_stats(&input3));

        // Test should_freeze with memory threshold exceeded
        let input4 = FreezeDecisionInput {
            delta_edge_count: 500,
            delta_memory_bytes: 300 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };
        assert!(manager.should_freeze_with_stats(&input4));

        // Test record_freeze
        manager.record_freeze(100, 50);
        let stats = manager.get_stats();
        assert_eq!(stats.freeze_count, 1);
        assert_eq!(stats.total_frozen_edges, 100);
        assert_eq!(stats.last_freeze_duration_ms, 50);

        // Test record_delta_size
        manager.record_delta_size(750);
        let stats = manager.get_stats();
        assert_eq!(stats.current_delta_edges, 750);
    }

    #[test]
    fn test_trigger_background_freeze_execution() {
        let mut storage = create_test_storage();
        let _space_id = setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);

        // Insert vertices
        let alice = VertexId::from_int64(1);
        let bob = VertexId::from_int64(2);

        let v1 = Vertex {
            vid: alice.clone(),
            id: 0,
            tags: vec![Tag::new(
                "Person".to_string(),
                [("name".to_string(), Value::String("Alice".to_string()))]
                    .iter()
                    .cloned()
                    .collect(),
            )],
            properties: [("name".to_string(), Value::String("Alice".to_string()))]
                .iter()
                .cloned()
                .collect(),
        };

        let v2 = Vertex {
            vid: bob.clone(),
            id: 0,
            tags: vec![Tag::new(
                "Person".to_string(),
                [("name".to_string(), Value::String("Bob".to_string()))]
                    .iter()
                    .cloned()
                    .collect(),
            )],
            properties: [("name".to_string(), Value::String("Bob".to_string()))]
                .iter()
                .cloned()
                .collect(),
        };

        storage.insert_vertex("test_space", v1).unwrap();
        storage.insert_vertex("test_space", v2).unwrap();

        // Insert edge
        let edge = Edge {
            src: alice,
            dst: bob,
            edge_type: "KNOWS".to_string(),
            ranking: 0,
            props: [("since".to_string(), Value::Int(2020))]
                .iter()
                .cloned()
                .collect(),
        };

        storage.insert_edge("test_space", edge).unwrap();

        // Trigger freeze - should succeed
        let result = storage.trigger_background_freeze();
        assert!(
            result.is_ok(),
            "Freeze should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_p9_phase3_cleanup_threshold_gc_integration() {
        // Test that compaction uses cleanup_threshold from SnapshotTracker (P9 Phase 3)
        let (_, mut storage) = create_persistent_storage();
        let _space_id = setup_space(&mut storage);
        let _person_tag = setup_person_tag(&mut storage);
        let _knows_edge = setup_knows_edge(&mut storage);

        // Create vertices
        let alice = VertexId::from_int64(1);
        let bob = VertexId::from_int64(2);

        // Insert vertices
        let v1 = Vertex {
            vid: alice.clone(),
            id: 0,
            tags: vec![Tag::new(
                "Person".to_string(),
                [("name".to_string(), Value::String("Alice".to_string()))]
                    .iter()
                    .cloned()
                    .collect(),
            )],
            properties: [("name".to_string(), Value::String("Alice".to_string()))]
                .iter()
                .cloned()
                .collect(),
        };

        storage.insert_vertex("test_space", v1).unwrap();

        // Verify SnapshotTracker is accessible through VersionManager
        let version_manager = storage.ctx.version_manager().clone();
        let snapshot_tracker = version_manager.snapshot_tracker();

        // Before any snapshots, cleanup_threshold should be MAX
        let initial_threshold = snapshot_tracker.cleanup_threshold();
        assert_eq!(initial_threshold, u32::MAX, "Initial cleanup_threshold should be u32::MAX");

        // Acquire a read timestamp (creates a snapshot)
        let read_ts = version_manager.acquire_read_timestamp();
        assert!(read_ts > 0, "Read timestamp should be valid");

        // Now cleanup_threshold should equal the read timestamp
        let threshold_with_active = snapshot_tracker.cleanup_threshold();
        assert_eq!(threshold_with_active, read_ts,
            "cleanup_threshold should equal active read timestamp");

        // Release the read timestamp
        version_manager.release_read_timestamp();

        // After releasing, cleanup_threshold should be MAX again
        let final_threshold = snapshot_tracker.cleanup_threshold();
        assert_eq!(final_threshold, u32::MAX, "Final cleanup_threshold should be u32::MAX after releasing");

        // Verify compaction works (it uses cleanup_threshold internally)
        let result = storage.compact(&Default::default());
        assert!(result.is_ok(), "Compaction should succeed with cleanup_threshold: {:?}", result.err());
    }

    #[test]
    fn test_snapshot_tracker_cleanup_threshold_multiple_readers() {
        // Test cleanup_threshold with multiple concurrent read transactions
        let storage = create_test_storage();
        let version_manager = storage.ctx.version_manager().clone();
        let snapshot_tracker = version_manager.snapshot_tracker();

        // Initially, no active snapshots
        assert_eq!(snapshot_tracker.cleanup_threshold(), u32::MAX);
        assert_eq!(snapshot_tracker.active_count(), 0);

        // Acquire multiple read timestamps
        let ts1 = version_manager.acquire_read_timestamp();
        let ts2 = version_manager.acquire_read_timestamp();
        let ts3 = version_manager.acquire_read_timestamp();

        // All should use the same read_ts (due to MVCC design)
        assert_eq!(ts1, ts2);
        assert_eq!(ts2, ts3);

        // cleanup_threshold should be the minimum active
        assert_eq!(snapshot_tracker.cleanup_threshold(), ts1);

        // Reference count should be 3
        assert_eq!(snapshot_tracker.ref_count(ts1), Some(3));

        // Release one
        version_manager.release_read_timestamp();
        assert_eq!(snapshot_tracker.ref_count(ts1), Some(2));
        assert_eq!(snapshot_tracker.cleanup_threshold(), ts1);  // Still active

        // Release another
        version_manager.release_read_timestamp();
        assert_eq!(snapshot_tracker.ref_count(ts1), Some(1));

        // Release the last one
        version_manager.release_read_timestamp();
        assert_eq!(snapshot_tracker.active_count(), 0);
        assert_eq!(snapshot_tracker.cleanup_threshold(), u32::MAX);
    }
}
