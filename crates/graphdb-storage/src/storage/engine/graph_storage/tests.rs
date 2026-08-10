#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::core::types::{
        AutoCompactConfig, EdgeTypeInfo, Index, IndexConfig, IndexField, IndexType, PropertyDef,
        SpaceInfo, Timestamp, UserInfo, VertexId,
    };
    use crate::core::vertex_edge_path::Tag;
    use crate::core::DataType;
    use crate::core::{Edge, EdgeDirection, RoleType, Value, Vertex};
    use crate::storage::{
        GraphStorage, PersistenceConfig, PropertyGraphConfig, ResourceConfig, ScanOptions,
        StorageAdmin, StorageAuthOps, StorageOperationContext, StorageOperationContextOps,
        StoragePersistenceOps, StorageReader, StorageSchemaOps, StorageWriter,
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

    #[test]
    fn persistent_storage_retains_caller_resource_config() {
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let mut property_config = PropertyGraphConfig::test();
        property_config.resources = ResourceConfig {
            max_memory_bytes: 32 * 1024 * 1024,
            index_memory_bytes: 8 * 1024 * 1024,
            ..property_config.resources.clone()
        };
        let persistence_config = PersistenceConfig::for_work_dir(temp_dir.path())
            .with_property_graph_config(property_config);
        let storage =
            GraphStorage::new_with_persistence(temp_dir.path().to_path_buf(), persistence_config)
                .expect("Failed to create persistent storage");

        let resources = storage.resource_snapshot();
        assert_eq!(resources.budget.max_memory_bytes, 32 * 1024 * 1024);
        assert_eq!(resources.budget.index_memory_bytes, 8 * 1024 * 1024);
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
    fn resolve_edge_type_name_round_trips_with_index_write_path() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);

        let hash = crate::storage::index::helpers::stable_hash(b"KNOWS") as u32;
        let resolved = storage
            .resolve_edge_type_name("test_space", hash)
            .expect("resolve should succeed")
            .expect("KNOWS must resolve");
        assert_eq!(resolved, "KNOWS");

        let missing = storage
            .resolve_edge_type_name("test_space", 0xdead_beef)
            .expect("resolve should succeed");
        assert!(missing.is_none(), "unknown hash must not resolve");
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

    #[test]
    fn bound_operation_contexts_are_isolated_across_concurrent_handles() {
        use crate::core::types::TransactionId;
        use std::sync::{Arc, Barrier};

        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let mut writer = storage.bind_operation_context(StorageOperationContext::transaction(
            TransactionId::from(1),
            10,
            false,
        ));
        writer
            .insert_vertex(
                "test_space",
                Vertex::new(
                    VertexId::from_int64(1),
                    vec![Tag::new(
                        "Person".to_string(),
                        [("name".to_string(), Value::string("Alice"))]
                            .into_iter()
                            .collect(),
                    )],
                ),
            )
            .expect("Failed to insert vertex at timestamp 10");

        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|id| {
                let barrier = barrier.clone();
                let bound = storage.bind_operation_context(StorageOperationContext::transaction(
                    TransactionId::from(id + 100),
                    10,
                    true,
                ));
                std::thread::spawn(move || {
                    barrier.wait();
                    let context = bound
                        .operation_context()
                        .expect("Bound context should remain available");
                    assert_eq!(context.transaction_id, Some(TransactionId::from(id + 100)));
                    assert_eq!(context.read_timestamp, 10);
                    assert!(bound
                        .get_vertex("test_space", &VertexId::from_int64(1))
                        .expect("Concurrent read failed")
                        .is_some());
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("Concurrent context task panicked");
        }
        assert!(storage.operation_context().is_none());
    }

    #[test]
    fn cursor_keeps_the_read_timestamp_from_its_bound_handle() {
        use crate::core::types::TransactionId;

        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let mut initial_writer = storage.bind_operation_context(
            StorageOperationContext::transaction(TransactionId::from(1), 10, false),
        );
        initial_writer
            .insert_vertex(
                "test_space",
                Vertex::new(
                    VertexId::from_int64(1),
                    vec![Tag::new("Person".to_string(), Default::default())],
                ),
            )
            .expect("Failed to insert initial vertex");

        let reader = storage.bind_operation_context(StorageOperationContext::transaction(
            TransactionId::from(2),
            10,
            true,
        ));
        let mut cursor = reader
            .create_vertex_cursor("test_space", &ScanOptions::default())
            .expect("Failed to create cursor");

        let mut later_writer = storage.bind_operation_context(
            StorageOperationContext::transaction(TransactionId::from(3), 20, false),
        );
        later_writer
            .insert_vertex(
                "test_space",
                Vertex::new(
                    VertexId::from_int64(2),
                    vec![Tag::new("Person".to_string(), Default::default())],
                ),
            )
            .expect("Failed to insert later vertex");

        let rows = cursor.next_batch(16).expect("Cursor read failed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].vid, VertexId::from_int64(1));
    }

    #[test]
    fn cursor_applies_property_projection_during_scan() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        storage
            .insert_vertex(
                "test_space",
                Vertex::new(
                    VertexId::from_int64(1),
                    vec![Tag::new(
                        "Person".to_string(),
                        [
                            ("name".to_string(), Value::string("Alice")),
                            ("age".to_string(), Value::BigInt(30)),
                        ]
                        .into_iter()
                        .collect(),
                    )],
                ),
            )
            .expect("vertex insert");

        let mut cursor = storage
            .create_vertex_cursor(
                "test_space",
                &ScanOptions::default().with_projection_named(vec!["name".to_string()]),
            )
            .expect("cursor should open");
        let rows = cursor.next_batch(8).expect("cursor batch");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].properties.contains_key("name"));
        assert!(!rows[0].properties.contains_key("age"));
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
            vec![crate::core::vertex_edge_path::Tag::new(
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
            vec![crate::core::vertex_edge_path::Tag::new(
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
            vec![crate::core::vertex_edge_path::Tag::new(
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
            vec![crate::core::vertex_edge_path::Tag::new(
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
    fn test_auto_commit_batch_window_reuses_snapshots() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let window = storage.begin_auto_commit_batch().unwrap();
        for i in 0..50 {
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let vertex = Vertex::new(
                VertexId::from_int64(1000 + i),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string(format!("person_{i}"))),
                        ("age".to_string(), Value::BigInt(i)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.insert_vertex("test_space", vertex).unwrap();
            bound.finalize_operation(true).unwrap();
            drop(bound);
        }

        // P4: the whole batch registers MVCC snapshots exactly once.
        assert_eq!(window.statement_count(), 50);
        assert_eq!(window.snapshot_rounds(), 1);

        // Per-statement commits are visible to later statements and reads.
        storage.finalize_auto_commit_batch(&window).unwrap();
        for i in 0..50 {
            let v = storage
                .get_vertex("test_space", &VertexId::from_int64(1000 + i))
                .unwrap()
                .unwrap();
            assert_eq!(v.properties.get("age"), Some(&Value::BigInt(i)));
        }

        // After the window is finalized the write gate is released: a new
        // single auto-commit statement can proceed.
        let mut next = storage.bind_auto_commit_context().unwrap();
        let vertex = Vertex::new(
            VertexId::from_int64(2000),
            vec![Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::string("after"))]
                    .into_iter()
                    .collect(),
            )],
        );
        next.insert_vertex("test_space", vertex).unwrap();
        next.finalize_operation(true).unwrap();
        drop(next);
    }

    #[test]
    fn test_auto_commit_batch_window_failed_statement_rolls_back_itself() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let window = storage.begin_auto_commit_batch().unwrap();
        {
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let vertex = Vertex::new(
                VertexId::from_int64(3001),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string("keep")),
                        ("age".to_string(), Value::BigInt(1)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.insert_vertex("test_space", vertex).unwrap();
            bound.finalize_operation(true).unwrap();
        }
        {
            // Failed statement: overwrite age in place, then abort. Only this
            // statement's partial write must roll back.
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let updated = Vertex::new(
                VertexId::from_int64(3001),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string("keep")),
                        ("age".to_string(), Value::BigInt(2)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.update_vertex("test_space", updated).unwrap();
            bound.finalize_operation(false).unwrap();
        }
        storage.finalize_auto_commit_batch(&window).unwrap();

        let v = storage
            .get_vertex("test_space", &VertexId::from_int64(3001))
            .unwrap()
            .unwrap();
        assert_eq!(v.properties.get("age"), Some(&Value::BigInt(1)));
    }

    #[test]
    fn test_auto_commit_batch_window_with_unique_index() {
        use crate::storage::index::traits::VertexIndexOps;

        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        // Unique index on Person.name exercises the P2 pending-aware unique
        // check inside the batch window: the duplicate must be rejected while
        // the earlier inserts' deltas are still unpublished.
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
            is_unique: true,
            covering: false,
            partial_condition: None,
        });
        storage.create_tag_index("test_space", &index).unwrap();

        let window = storage.begin_auto_commit_batch().unwrap();
        for i in 0..50 {
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let vertex = Vertex::new(
                VertexId::from_int64(1000 + i),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string(format!("person_{i}"))),
                        ("age".to_string(), Value::BigInt(i)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.insert_vertex("test_space", vertex).unwrap();
            bound.finalize_operation(true).unwrap();
            drop(bound);
        }

        let manager = storage.ctx.index_data_manager();

        // Before any read flushes the pending deltas, a duplicate name from a
        // later window statement must be rejected via the pending-aware unique
        // check (person_10 already committed at vid 1010).
        assert!(
            manager
                .read()
                .pending_delta_entries(crate::storage::index::types::IndexIdentity {
                    space_id: 1,
                    index_id: 1,
                })
                > 0,
            "index deltas must still be pending before any read flush"
        );
        {
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let duplicate = Vertex::new(
                VertexId::from_int64(2000),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::string("person_10"))]
                        .into_iter()
                        .collect(),
                )],
            );
            let result = bound.insert_vertex("test_space", duplicate);
            assert!(
                result.is_err(),
                "duplicate unique name must be rejected inside the window"
            );
            bound.finalize_operation(false).unwrap();
            drop(bound);
        }

        // The failed statement left no index residue and no vertex data.
        assert!(storage
            .get_vertex("test_space", &VertexId::from_int64(2000))
            .unwrap()
            .is_none());
        storage.finalize_auto_commit_batch(&window).unwrap();

        // Index lookups observe all 50 committed entries after finalize.
        for i in 0..50 {
            let results = manager
                .read()
                .lookup_tag_index(1, &index, &Value::string(format!("person_{i}")))
                .unwrap();
            assert_eq!(results, vec![Value::BigInt(1000 + i)], "lookup person_{i}");
        }
    }

    #[test]
    fn test_auto_commit_batch_window_via_sync_wrapper() {
        use crate::storage::AutoCommitBatchOps;

        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        // L1: SyncWrapper<S> forwards the batch-window operations to the inner
        // engine, so the server-side QueryApi<SyncWrapper<GraphStorage>> can
        // share one window across statements.
        let wrapper = crate::storage::SyncWrapper::new(storage);

        let window = wrapper.begin_auto_commit_batch().unwrap();
        for i in 0..10 {
            let mut bound = wrapper.bind_auto_commit_statement(&window).unwrap();
            let vertex = Vertex::new(
                VertexId::from_int64(5000 + i),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string(format!("person_{i}"))),
                        ("age".to_string(), Value::BigInt(i)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.insert_vertex("test_space", vertex).unwrap();
            bound.finalize_operation(true).unwrap();
            drop(bound);
        }
        wrapper.finalize_auto_commit_batch(&window).unwrap();

        for i in 0..10 {
            let v = wrapper
                .get_vertex("test_space", &VertexId::from_int64(5000 + i))
                .unwrap()
                .unwrap();
            assert_eq!(v.properties.get("age"), Some(&Value::BigInt(i)));
        }
    }

    // -----------------------------------------------------------------------
    // P0 C: group-commit window tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_group_window_single_ts() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let window = storage.begin_auto_commit_group().unwrap();
        let mut timestamps = Vec::new();
        for i in 0..5 {
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let ts = bound
                .operation_context()
                .and_then(|ctx| ctx.write_timestamp)
                .unwrap();
            timestamps.push(ts);
            let vertex = Vertex::new(
                VertexId::from_int64(6000 + i),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string(format!("gperson_{i}"))),
                        ("age".to_string(), Value::BigInt(i)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.insert_vertex("test_space", vertex).unwrap();
            bound.finalize_operation(true).unwrap();
            drop(bound);
        }

        assert_eq!(window.statement_count(), 5);
        assert_eq!(window.snapshot_rounds(), 1);
        // All statements must share the same write timestamp.
        let first = timestamps[0];
        for ts in &timestamps[1..] {
            assert_eq!(*ts, first, "group mode must reuse the same write timestamp");
        }

        window.finalize_group().unwrap();
        // After finalize, data is visible.
        for i in 0..5 {
            let v = storage
                .get_vertex("test_space", &VertexId::from_int64(6000 + i))
                .unwrap()
                .unwrap();
            assert_eq!(v.properties.get("age"), Some(&Value::BigInt(i)));
        }
    }

    #[test]
    fn test_group_commit_visibility() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let window = storage.begin_auto_commit_group().unwrap();
        // First statement: insert vertex.
        {
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let vertex = Vertex::new(
                VertexId::from_int64(7001),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string("visible".to_string())),
                        ("age".to_string(), Value::BigInt(1)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.insert_vertex("test_space", vertex).unwrap();
            bound.finalize_operation(true).unwrap();
        }
        // Second statement: read own writes (same group ts).
        {
            let bound = storage.bind_auto_commit_statement(&window).unwrap();
            let v = bound
                .get_vertex("test_space", &VertexId::from_int64(7001))
                .unwrap();
            assert!(
                v.is_some(),
                "group-internal read must see prior statement's write"
            );
        }
        window.finalize_group().unwrap();

        // After finalize, external reader sees data.
        let v = storage
            .get_vertex("test_space", &VertexId::from_int64(7001))
            .unwrap()
            .unwrap();
        assert_eq!(v.properties.get("name"), Some(&Value::string("visible")));
    }

    #[test]
    fn test_group_failed_statement_rolls_back_own_writes() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let window = storage.begin_auto_commit_group().unwrap();
        // Statement 1: insert vertex 8001.
        {
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let vertex = Vertex::new(
                VertexId::from_int64(8001),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string("keep".to_string())),
                        ("age".to_string(), Value::BigInt(1)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.insert_vertex("test_space", vertex).unwrap();
            bound.finalize_operation(true).unwrap();
        }
        // Statement 2: update vertex 8001, then rollback.
        {
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let updated = Vertex::new(
                VertexId::from_int64(8001),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string("changed".to_string())),
                        ("age".to_string(), Value::BigInt(2)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.update_vertex("test_space", updated).unwrap();
            bound.finalize_operation(false).unwrap();
        }
        // Statement 3: insert vertex 8002.
        {
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let vertex = Vertex::new(
                VertexId::from_int64(8002),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string("also_keep".to_string())),
                        ("age".to_string(), Value::BigInt(3)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.insert_vertex("test_space", vertex).unwrap();
            bound.finalize_operation(true).unwrap();
        }

        window.finalize_group().unwrap();

        // Vertex 8001 should have original values (statement 2 rolled back).
        let v = storage
            .get_vertex("test_space", &VertexId::from_int64(8001))
            .unwrap()
            .unwrap();
        assert_eq!(v.properties.get("name"), Some(&Value::string("keep")));
        assert_eq!(v.properties.get("age"), Some(&Value::BigInt(1)));
        // Vertex 8002 exists (statement 3 committed).
        let v2 = storage
            .get_vertex("test_space", &VertexId::from_int64(8002))
            .unwrap()
            .unwrap();
        assert_eq!(
            v2.properties.get("name"),
            Some(&Value::string("also_keep"))
        );
    }

    #[test]
    fn test_group_window_without_wal_manager() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        // In-memory storage has no WAL manager; finalize_group sync is a no-op.
        let window = storage.begin_auto_commit_group().unwrap();
        {
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let vertex = Vertex::new(
                VertexId::from_int64(9001),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string("mem".to_string())),
                        ("age".to_string(), Value::BigInt(1)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.insert_vertex("test_space", vertex).unwrap();
            bound.finalize_operation(true).unwrap();
        }
        window.finalize_group().unwrap();

        let v = storage
            .get_vertex("test_space", &VertexId::from_int64(9001))
            .unwrap()
            .unwrap();
        assert_eq!(v.properties.get("name"), Some(&Value::string("mem")));
    }

    #[test]
    fn test_group_empty_window_finalize() {
        let storage = create_test_storage();
        let window = storage.begin_auto_commit_group().unwrap();
        assert_eq!(window.statement_count(), 0);
        // Finalize without binding any statements must not panic.
        window.finalize_group().unwrap();
    }

    #[test]
    fn test_group_window_staged_wal_bounded() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let window = storage.begin_auto_commit_group().unwrap();
        for i in 0..10 {
            let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
            let vertex = Vertex::new(
                VertexId::from_int64(10000 + i),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![
                        ("name".to_string(), Value::string(format!("gbounded_{i}"))),
                        ("age".to_string(), Value::BigInt(i)),
                    ]
                    .into_iter()
                    .collect(),
                )],
            );
            bound.insert_vertex("test_space", vertex).unwrap();
            bound.finalize_operation(true).unwrap();
            drop(bound);
            // Staged WAL must stay bounded (entries are removed by no-wait append).
            let wal_len = storage.staged_wal_len();
            assert!(
                wal_len <= 1,
                "staged WAL must stay <=1 during group, got {wal_len}"
            );
        }
        window.finalize_group().unwrap();
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
                vec![crate::core::vertex_edge_path::Tag::new(
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
            vec![crate::core::vertex_edge_path::Tag::new(
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
                    vec![crate::core::vertex_edge_path::Tag::new(
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
                vec![crate::core::vertex_edge_path::Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::string("Alice"))]
                        .into_iter()
                        .collect(),
                )],
            ),
            Vertex::new(
                VertexId::from_int64(1),
                vec![crate::core::vertex_edge_path::Tag::new(
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

    // ==================== Edge Operations ====================

    fn insert_test_vertex(storage: &mut GraphStorage, id: i64, name: &str) {
        let vertex = Vertex::new(
            VertexId::from_int64(id),
            vec![crate::core::vertex_edge_path::Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::string(name))]
                    .into_iter()
                    .collect(),
            )],
        );
        storage.insert_vertex("test_space", vertex).unwrap();
    }

    #[test]
    fn test_edge_property_index_range_lookup() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);

        let edge_type =
            crate::core::types::EdgeTypeInfo::new("WEIGHTED".to_string()).with_properties(vec![
                PropertyDef::new("weight".to_string(), DataType::BigInt),
            ]);
        storage
            .create_edge_type("test_space", &edge_type)
            .expect("Failed to create edge type");

        insert_test_vertex(&mut storage, 1, "Alice");
        insert_test_vertex(&mut storage, 2, "Bob");
        insert_test_vertex(&mut storage, 3, "Carol");

        let make_edge = |src: i64, dst: i64, weight: i64| {
            Edge::new(
                VertexId::from_int64(src),
                VertexId::from_int64(dst),
                "WEIGHTED".to_string(),
                0,
                [("weight".to_string(), Value::BigInt(weight))]
                    .into_iter()
                    .collect(),
            )
        };
        storage
            .insert_edge("test_space", make_edge(1, 2, 10))
            .unwrap();
        storage
            .insert_edge("test_space", make_edge(1, 3, 25))
            .unwrap();
        storage
            .insert_edge("test_space", make_edge(2, 3, 5))
            .unwrap();

        // Enable after inserts: build path indexes existing edges.
        assert!(!storage
            .has_edge_property_index("test_space", "WEIGHTED")
            .unwrap());
        storage
            .enable_edge_property_index("test_space", "WEIGHTED", 64 * 1024 * 1024)
            .unwrap();
        assert!(storage
            .has_edge_property_index("test_space", "WEIGHTED")
            .unwrap());

        // weight >= 20
        let edges = storage
            .lookup_edges_by_property_range(
                "test_space",
                "WEIGHTED",
                "weight",
                Some(&Value::BigInt(20)),
                None,
                true,
                false,
            )
            .unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src, VertexId::from_int64(1));
        assert_eq!(edges[0].dst, VertexId::from_int64(3));
        assert_eq!(edges[0].props.get("weight"), Some(&Value::BigInt(25)));

        // 5 <= weight < 25
        let edges = storage
            .lookup_edges_by_property_range(
                "test_space",
                "WEIGHTED",
                "weight",
                Some(&Value::BigInt(5)),
                Some(&Value::BigInt(25)),
                true,
                false,
            )
            .unwrap();
        assert_eq!(edges.len(), 2);

        // Disable frees the index.
        storage
            .disable_edge_property_index("test_space", "WEIGHTED")
            .unwrap();
        assert!(!storage
            .has_edge_property_index("test_space", "WEIGHTED")
            .unwrap());
        let edges = storage
            .lookup_edges_by_property_range(
                "test_space",
                "WEIGHTED",
                "weight",
                Some(&Value::BigInt(20)),
                None,
                true,
                false,
            )
            .unwrap();
        assert!(edges.is_empty());
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

    /// Phase B acceptance: the batched accessors must agree with
    /// `get_node_edges` for out/in/both directions and edge-type filtering.
    #[test]
    fn test_batch_accessors_match_get_node_edges() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);

        for i in 1..=5i64 {
            insert_test_vertex(&mut storage, i, &format!("v{i}"));
        }
        for (src, dst) in [(1i64, 2), (1, 3), (2, 3), (3, 1), (4, 1)] {
            let edge = Edge::new(
                VertexId::from_int64(src),
                VertexId::from_int64(dst),
                "KNOWS".to_string(),
                0,
                std::collections::HashMap::new(),
            );
            storage.insert_edge("test_space", edge).unwrap();
        }

        let seeds = [
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            VertexId::from_int64(5),
        ];
        let knowses = vec!["KNOWS".to_string()];

        for direction in [EdgeDirection::Out, EdgeDirection::In, EdgeDirection::Both] {
            for (edge_types, label) in [(Vec::<String>::new(), "all"), (knowses.clone(), "KNOWS")] {
                // Reference: distinct dst ids from get_node_edges.
                let mut expected: Vec<Vec<VertexId>> = Vec::new();
                for seed in &seeds {
                    let edges = storage
                        .get_node_edges("test_space", seed, direction)
                        .unwrap();
                    let mut dsts: Vec<VertexId> = edges
                        .iter()
                        .filter(|e| edge_types.is_empty() || edge_types.contains(&e.edge_type))
                        .map(|e| {
                            if matches!(direction, EdgeDirection::Out) {
                                e.dst
                            } else if matches!(direction, EdgeDirection::In) {
                                e.src
                            } else if e.src == *seed {
                                e.dst
                            } else {
                                e.src
                            }
                        })
                        .collect();
                    dsts.sort();
                    dsts.dedup();
                    expected.push(dsts);
                }

                let batch = storage
                    .neighbor_dst_ids_batch("test_space", &seeds, direction, &edge_types)
                    .unwrap();
                let actual: Vec<Vec<VertexId>> = batch
                    .into_iter()
                    .map(|mut dsts| {
                        dsts.sort();
                        dsts.dedup();
                        dsts
                    })
                    .collect();
                assert_eq!(
                    expected, actual,
                    "neighbor batch mismatch ({label}, {direction:?})"
                );

                let degrees: Vec<usize> = storage
                    .out_degree_batch("test_space", &seeds, direction, &edge_types)
                    .unwrap();
                let expected_degrees: Vec<usize> = seeds
                    .iter()
                    .map(|seed| {
                        storage
                            .get_node_edges("test_space", seed, direction)
                            .unwrap()
                            .iter()
                            .filter(|edge| {
                                edge_types.is_empty() || edge_types.contains(&edge.edge_type)
                            })
                            .count()
                    })
                    .collect();
                assert_eq!(
                    expected_degrees, degrees,
                    "degree batch mismatch ({label}, {direction:?})"
                );
            }
        }
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
                vec![("name".to_string(), Value::string("A"))]
                    .into_iter()
                    .collect(),
            )],
        );
        let v2 = Vertex::new(
            VertexId::from_string("b"),
            vec![Tag::new(
                "Node".to_string(),
                vec![("name".to_string(), Value::string("B"))]
                    .into_iter()
                    .collect(),
            )],
        );
        let v3 = Vertex::new(
            VertexId::from_string("c"),
            vec![Tag::new(
                "Node".to_string(),
                vec![("name".to_string(), Value::string("C"))]
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
                vec![("name".to_string(), Value::string("A"))]
                    .into_iter()
                    .collect(),
            )],
        );
        let v2 = Vertex::new(
            VertexId::from_string("b"),
            vec![Tag::new(
                "Node".to_string(),
                vec![("name".to_string(), Value::string("B"))]
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
                vec![("name".to_string(), Value::string("X"))]
                    .into_iter()
                    .collect(),
            )],
        );
        let v2 = Vertex::new(
            VertexId::from_string("y"),
            vec![Tag::new(
                "Node".to_string(),
                vec![("name".to_string(), Value::string("Y"))]
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
                vec![("name".to_string(), Value::string("Alice"))]
                    .into_iter()
                    .collect(),
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

    // ==================== Freeze Integration Tests ====================

    #[test]
    fn test_background_freeze_manager_basics() {
        use crate::storage::engine::background_freeze::BackgroundFreezeManager;
        use crate::storage::engine::config::{
            FreezeConfig, FreezeDecisionInput, FreezeStrategyType,
        };

        let config = FreezeConfig {
            strategy: FreezeStrategyType::Conservative,
            delta_edge_threshold: 1000,
            delta_memory_threshold_bytes: 256 * 1024 * 1024,
            max_segment_age: Timestamp::MAX,
            deletion_threshold: 0.5,
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
            vid: alice,
            id: 0,
            tags: vec![Tag::new(
                "Person".to_string(),
                [("name".to_string(), Value::string("Alice"))]
                    .iter()
                    .cloned()
                    .collect(),
            )],
            properties: [("name".to_string(), Value::string("Alice"))]
                .iter()
                .cloned()
                .collect(),
        };

        let v2 = Vertex {
            vid: bob,
            id: 0,
            tags: vec![Tag::new(
                "Person".to_string(),
                [("name".to_string(), Value::string("Bob"))]
                    .iter()
                    .cloned()
                    .collect(),
            )],
            properties: [("name".to_string(), Value::string("Bob"))]
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
        assert!(result.is_ok(), "Freeze should succeed: {:?}", result.err());
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
        let _bob = VertexId::from_int64(2);

        // Insert vertices
        let v1 = Vertex {
            vid: alice,
            id: 0,
            tags: vec![Tag::new(
                "Person".to_string(),
                [("name".to_string(), Value::string("Alice"))]
                    .iter()
                    .cloned()
                    .collect(),
            )],
            properties: [("name".to_string(), Value::string("Alice"))]
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
        assert_eq!(
            initial_threshold,
            Timestamp::MAX,
            "Initial cleanup_threshold should be Timestamp::MAX"
        );

        // Acquire a read timestamp (creates a snapshot)
        let read_ts = version_manager
            .acquire_read_timestamp()
            .expect("Failed to acquire read timestamp");
        assert!(read_ts > 0, "Read timestamp should be valid");

        // Now cleanup_threshold should equal the read timestamp
        let threshold_with_active = snapshot_tracker.cleanup_threshold();
        assert_eq!(
            threshold_with_active, read_ts,
            "cleanup_threshold should equal active read timestamp"
        );

        // Release the read timestamp
        version_manager.release_read_timestamp();

        // After releasing, cleanup_threshold should be MAX again
        let final_threshold = snapshot_tracker.cleanup_threshold();
        assert_eq!(
            final_threshold,
            Timestamp::MAX,
            "Final cleanup_threshold should be Timestamp::MAX after releasing"
        );

        // Verify compaction works (it uses cleanup_threshold internally)
        let result = storage.compact(&Default::default());
        assert!(
            result.is_ok(),
            "Compaction should succeed with cleanup_threshold: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_snapshot_tracker_cleanup_threshold_multiple_readers() {
        // Test cleanup_threshold with multiple concurrent read transactions
        let storage = create_test_storage();
        let version_manager = storage.ctx.version_manager().clone();
        let snapshot_tracker = version_manager.snapshot_tracker();

        // Initially, no active snapshots
        assert_eq!(snapshot_tracker.cleanup_threshold(), Timestamp::MAX);
        assert_eq!(snapshot_tracker.active_count(), 0);

        // Acquire multiple read timestamps
        let ts1 = version_manager
            .acquire_read_timestamp()
            .expect("Failed to acquire read timestamp");
        let ts2 = version_manager
            .acquire_read_timestamp()
            .expect("Failed to acquire read timestamp");
        let ts3 = version_manager
            .acquire_read_timestamp()
            .expect("Failed to acquire read timestamp");

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
        assert_eq!(snapshot_tracker.cleanup_threshold(), ts1); // Still active

        // Release another
        version_manager.release_read_timestamp();
        assert_eq!(snapshot_tracker.ref_count(ts1), Some(1));

        // Release the last one
        version_manager.release_read_timestamp();
        assert_eq!(snapshot_tracker.active_count(), 0);
        assert_eq!(snapshot_tracker.cleanup_threshold(), Timestamp::MAX);
    }

    #[test]
    fn test_compact_maintenance_propagates_vertex_remap_to_edge_tables() {
        // Regression: vertex compaction densifies internal IDs; the old-to-new
        // mapping must be propagated into edge CSR rows/neighbors or every
        // edge referencing a surviving vertex breaks (P9 phase 3).
        let (_, mut storage) = create_persistent_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);

        // 100 vertices (internal ids 0..99 in insertion order).
        for i in 1..=100i64 {
            insert_test_vertex(&mut storage, i, &format!("v{i}"));
        }

        // Edges only between vertices that will survive compaction:
        // odd pairs (1,3), (3,5), ..., (77,79) and (79,81), ..., (97,99).
        let expected_edges: Vec<(i64, i64)> =
            (1..=97).step_by(2).map(|src| (src, src + 2)).collect();
        for (src, dst) in &expected_edges {
            let edge = Edge::new(
                VertexId::from_int64(*src),
                VertexId::from_int64(*dst),
                "KNOWS".to_string(),
                0,
                std::collections::HashMap::new(),
            );
            storage.insert_edge("test_space", edge).unwrap();
        }

        // Delete 40 vertices (external ids 2..80 step 2); their internal ids
        // are interleaved with survivors, forcing a real ID remap.
        for i in (2..=80).step_by(2) {
            storage
                .delete_vertex("test_space", &VertexId::from_int64(i))
                .unwrap();
        }

        let vertex_count_before = storage
            .ctx
            .data_store()
            .with_vertex_tables(|tables| {
                Ok::<usize, crate::core::StorageError>(
                    tables.values().map(|t| t.total_count()).sum::<usize>(),
                )
            })
            .unwrap();
        assert_eq!(vertex_count_before, 100);

        storage.compact(&Default::default()).unwrap();

        // 40 vertices removed by compaction.
        let vertex_count_after = storage
            .ctx
            .data_store()
            .with_vertex_tables(|tables| {
                Ok::<usize, crate::core::StorageError>(
                    tables.values().map(|t| t.total_count()).sum::<usize>(),
                )
            })
            .unwrap();
        assert_eq!(
            vertex_count_after, 60,
            "compaction should remove 40 vertices"
        );

        // Deleted vertices no longer resolve.
        assert!(storage
            .get_vertex("test_space", &VertexId::from_int64(2))
            .unwrap()
            .is_none());

        // Every surviving edge still resolves through the remapped edge CSR.
        for (src, dst) in &expected_edges {
            let retrieved = storage
                .get_edge(
                    "test_space",
                    &VertexId::from_int64(*src),
                    &VertexId::from_int64(*dst),
                    "KNOWS",
                    0,
                )
                .unwrap();
            assert!(
                retrieved.is_some(),
                "edge {src}->{dst} lost after vertex compaction remap"
            );
        }

        // Node-edge scans resolve through remapped out/in CSRs.
        let out_edges = storage
            .get_node_edges("test_space", &VertexId::from_int64(1), EdgeDirection::Out)
            .unwrap();
        assert_eq!(out_edges.len(), 1);
        assert_eq!(out_edges[0].dst, VertexId::from_int64(3));
        let in_edges = storage
            .get_node_edges("test_space", &VertexId::from_int64(79), EdgeDirection::In)
            .unwrap();
        assert_eq!(in_edges.len(), 1);
        assert_eq!(in_edges[0].src, VertexId::from_int64(77));
    }

    #[test]
    fn test_auto_vertex_compaction_reclaims_id_holes() {
        // Background maintenance must reclaim deleted-vertex ID holes without
        // an explicit compact transaction when thresholds are exceeded.
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let mut property_config = PropertyGraphConfig::test();
        property_config.auto_compact = AutoCompactConfig {
            enable_vertex_compaction: true,
            min_holes: 20,
            min_hole_ratio: 0.1,
            min_interval_secs: 0,
        };
        let persistence_config = PersistenceConfig::for_work_dir(temp_dir.path())
            .with_property_graph_config(property_config);
        let mut storage =
            GraphStorage::new_with_persistence(temp_dir.path().to_path_buf(), persistence_config)
                .expect("Failed to create persistent storage");
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        setup_knows_edge(&mut storage);

        for i in 1..=100i64 {
            insert_test_vertex(&mut storage, i, &format!("v{i}"));
        }
        let expected_edges: Vec<(i64, i64)> =
            (1..=97).step_by(2).map(|src| (src, src + 2)).collect();
        for (src, dst) in &expected_edges {
            let edge = Edge::new(
                VertexId::from_int64(*src),
                VertexId::from_int64(*dst),
                "KNOWS".to_string(),
                0,
                std::collections::HashMap::new(),
            );
            storage.insert_edge("test_space", edge).unwrap();
        }
        // Delete 40 vertices; holes appear but nothing is reclaimed yet.
        for i in (2..=80).step_by(2) {
            storage
                .delete_vertex("test_space", &VertexId::from_int64(i))
                .unwrap();
        }

        let (live, allocated) = storage
            .ctx
            .data_store()
            .with_vertex_tables(|tables| {
                Ok::<(usize, usize), crate::core::StorageError>(
                    tables
                        .values()
                        .map(|t| t.id_hole_stats(u64::MAX))
                        .fold((0, 0), |(l, a), (x, y)| (l + x, a + y)),
                )
            })
            .unwrap();
        assert_eq!(
            (live, allocated),
            (60, 100),
            "holes must not be reclaimed yet"
        );

        storage.trigger_background_maintenance().unwrap();

        let (live, allocated) = storage
            .ctx
            .data_store()
            .with_vertex_tables(|tables| {
                Ok::<(usize, usize), crate::core::StorageError>(
                    tables
                        .values()
                        .map(|t| t.id_hole_stats(u64::MAX))
                        .fold((0, 0), |(l, a), (x, y)| (l + x, a + y)),
                )
            })
            .unwrap();
        assert_eq!(
            (live, allocated),
            (60, 60),
            "auto compaction should re-densify ID space"
        );

        // Surviving edges still resolve through the remapped edge CSR.
        for (src, dst) in &expected_edges {
            let retrieved = storage
                .get_edge(
                    "test_space",
                    &VertexId::from_int64(*src),
                    &VertexId::from_int64(*dst),
                    "KNOWS",
                    0,
                )
                .unwrap();
            assert!(
                retrieved.is_some(),
                "edge {src}->{dst} lost after auto vertex compaction remap"
            );
        }
    }

    #[test]
    fn debug_edge_property_index() {
        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        let edge_type =
            crate::core::types::EdgeTypeInfo::new("WEIGHTED".to_string()).with_properties(vec![
                PropertyDef::new("weight".to_string(), DataType::BigInt),
            ]);
        storage.create_edge_type("test_space", &edge_type).unwrap();
        insert_test_vertex(&mut storage, 1, "Alice");
        insert_test_vertex(&mut storage, 2, "Bob");
        insert_test_vertex(&mut storage, 3, "Carol");
        let make_edge = |src: i64, dst: i64, weight: i64| {
            Edge::new(
                VertexId::from_int64(src),
                VertexId::from_int64(dst),
                "WEIGHTED".to_string(),
                0,
                [("weight".to_string(), Value::BigInt(weight))]
                    .into_iter()
                    .collect(),
            )
        };
        storage
            .insert_edge("test_space", make_edge(1, 2, 10))
            .unwrap();
        storage
            .insert_edge("test_space", make_edge(1, 3, 25))
            .unwrap();
        storage
            .insert_edge("test_space", make_edge(2, 3, 5))
            .unwrap();
        let all = storage
            .scan_edges_by_type("test_space", "WEIGHTED")
            .unwrap();
        eprintln!("scan_edges_by_type count = {}", all.len());
        for e in &all {
            eprintln!("  edge src={:?} dst={:?} props={:?}", e.src, e.dst, e.props);
        }
        storage
            .enable_edge_property_index("test_space", "WEIGHTED", 64 * 1024 * 1024)
            .unwrap();
        eprintln!(
            "has_index = {:?}",
            storage.has_edge_property_index("test_space", "WEIGHTED")
        );
        let edges = storage
            .lookup_edges_by_property_range(
                "test_space",
                "WEIGHTED",
                "weight",
                Some(&Value::BigInt(20)),
                None,
                true,
                false,
            )
            .unwrap();
        eprintln!("lookup >=20 count = {}", edges.len());
        for e in &edges {
            eprintln!("  edge src={:?} dst={:?} props={:?}", e.src, e.dst, e.props);
        }
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
    fn cold_snapshot_delta_time_machine_merge_flow() {
        use crate::storage::client::StorageSnapshotOps;

        let mut storage = create_test_storage();
        let temp_dir = tempfile::TempDir::new().unwrap();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        let edge_type = crate::core::types::EdgeTypeInfo::new("KNOWS".to_string())
            .with_src_tag("Person".to_string())
            .with_dst_tag("Person".to_string())
            .with_properties(vec![PropertyDef::new("since".to_string(), DataType::Int)]);
        storage.create_edge_type("test_space", &edge_type).unwrap();
        insert_test_vertex(&mut storage, 1, "Alice");
        insert_test_vertex(&mut storage, 2, "Bob");
        insert_test_vertex(&mut storage, 3, "Carol");

        // Export at the storage's actual read timestamps so MVCC visibility
        // is deterministic.
        let make_edge = |src: i64, dst: i64| {
            Edge::new(
                VertexId::from_int64(src),
                VertexId::from_int64(dst),
                "KNOWS".to_string(),
                0,
                [("since".to_string(), Value::Int(2020))]
                    .into_iter()
                    .collect(),
            )
        };
        storage.insert_edge("test_space", make_edge(1, 2)).unwrap();
        let ts_after_edge1 = storage.version_manager().read_timestamp();
        storage.insert_edge("test_space", make_edge(1, 3)).unwrap();
        let ts_after_edge2 = storage.version_manager().read_timestamp();

        // v6: export two snapshots and a delta between them.
        let snap_dir = temp_dir.path().join("cold_snapshots");
        std::fs::create_dir_all(&snap_dir).unwrap();
        let base_path = snap_dir.join("knows_1.lkcs");
        let latest_path = snap_dir.join("knows_2.lkcs");
        let base = storage
            .export_cold_snapshot("test_space", "KNOWS", ts_after_edge1, &base_path)
            .unwrap();
        let latest = storage
            .export_cold_snapshot("test_space", "KNOWS", ts_after_edge2, &latest_path)
            .unwrap();
        assert_eq!(base.edge_count(), 1);
        assert_eq!(latest.edge_count(), 2);

        let delta_path = snap_dir.join("knows_1_2.lkcd");
        let delta = storage
            .export_cold_delta(
                "test_space",
                "KNOWS",
                ts_after_edge1,
                ts_after_edge2,
                &delta_path,
            )
            .unwrap();
        assert_eq!(delta.added.len(), 1);
        assert_eq!(delta.removed.len(), 0);

        // Register snapshots via the StorageSnapshotOps trait.
        let info_base = StorageSnapshotOps::load_cold_snapshot(&storage, &base_path).unwrap();
        assert_eq!(info_base.label, base.label());
        assert_eq!(info_base.label_name, "KNOWS");
        let info_latest = StorageSnapshotOps::load_cold_snapshot(&storage, &latest_path).unwrap();
        assert_eq!(info_latest.edge_count, 2);

        let listed = StorageSnapshotOps::list_cold_snapshots(&storage).unwrap();
        assert_eq!(listed.len(), 2);
        let total_edges: u64 = listed.iter().map(|i| i.edge_count).sum();
        assert_eq!(total_edges, 3);

        // v7: time machine routes to the most recent snapshot not newer than ts.
        let machine = storage.cold_time_machine();
        assert_eq!(machine.version_count(base.label()), 2);
        assert!(machine
            .snapshot_at(base.label(), ts_after_edge1 - 1)
            .is_none());
        assert_eq!(
            machine
                .snapshot_at(base.label(), ts_after_edge1)
                .unwrap()
                .edge_count(),
            1
        );
        assert_eq!(
            machine
                .snapshot_at(base.label(), ts_after_edge2 - 1)
                .unwrap()
                .edge_count(),
            1
        );
        assert_eq!(
            machine
                .snapshot_at(base.label(), u64::MAX)
                .unwrap()
                .edge_count(),
            2
        );

        // v6: replay the delta onto the base registered snapshot.
        let reconstructed = storage.apply_cold_delta(base.label(), &delta_path).unwrap();
        assert_eq!(reconstructed.edge_count(), 2);
        // Structural equality with the latest snapshot: same (src, dst) rows.
        let edge_set = |snapshot: &crate::storage::cold::ColdSnapshot| -> std::collections::HashSet<(u32, i64)> {
            snapshot
                .scan_edges()
                .iter()
                .map(|r| (r.src_internal, r.dst_vid.as_int64().unwrap_or(0)))
                .collect()
        };
        assert_eq!(edge_set(&reconstructed), edge_set(&latest));

        // v9: consolidate the shelf into a single merged snapshot.
        let merged = StorageSnapshotOps::merge_cold_snapshots(&storage, &[base.label()]).unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].edge_count, 2);
        assert_eq!(merged[0].snapshot_ts, ts_after_edge2);
        let after = StorageSnapshotOps::list_cold_snapshots(&storage).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].edge_count, 2);

        // Re-export of the merged snapshot is portable.
        let reexport_path = temp_dir.path().join("reexport.lkcs");
        let info = StorageSnapshotOps::export_cold_snapshot(&storage, base.label(), &reexport_path)
            .unwrap();
        assert_eq!(info.edge_count, 2);
        assert_eq!(
            info.file_size,
            std::fs::metadata(&reexport_path).unwrap().len()
        );

        // Remove unregisters the shelf.
        StorageSnapshotOps::remove_cold_snapshot(&storage, base.label()).unwrap();
        assert!(StorageSnapshotOps::list_cold_snapshots(&storage)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_read_operation_context_pins_and_releases_statement_snapshot() {
        use crate::storage::StorageOperationContextOps;

        let mut storage = create_test_storage();
        setup_space(&mut storage);
        setup_person_tag(&mut storage);
        insert_test_vertex(&mut storage, 1, "Alice");

        let label = storage
            .ctx
            .data_store()
            .with_vertex_tables(|tables| tables.keys().copied().collect::<Vec<_>>())[0];

        // Bind a read-only statement context with a fixed snapshot timestamp.
        let bound = storage.bind_read_operation_context().unwrap();
        let op_ctx = bound.operation_context().expect("read context");
        assert!(op_ctx.read_only, "read context must be read-only");
        assert!(
            op_ctx.write_timestamp.is_none(),
            "read context has no write ts"
        );
        let read_ts = op_ctx.read_timestamp;
        assert!(read_ts > 0, "read context must pin a snapshot timestamp");

        // No snapshot is registered before the first table access (lazy).
        let before = storage
            .ctx
            .data_store()
            .with_vertex_tables(|tables| {
                Ok::<Timestamp, crate::core::StorageError>(
                    tables
                        .get(&label)
                        .map(|t| t.min_active_snapshot_ts())
                        .unwrap_or(Timestamp::MAX),
                )
            })
            .unwrap();

        // First read lazily registers the table snapshot at the read ts.
        let vertex = bound
            .get_vertex("test_space", &VertexId::from_int64(1))
            .unwrap()
            .expect("vertex should resolve");
        assert_eq!(
            vertex.properties.get("name").unwrap(),
            &Value::string("Alice")
        );
        let pinned = storage
            .ctx
            .data_store()
            .with_vertex_tables(|tables| {
                Ok::<Timestamp, crate::core::StorageError>(
                    tables
                        .get(&label)
                        .map(|t| t.min_active_snapshot_ts())
                        .unwrap_or(Timestamp::MAX),
                )
            })
            .unwrap();
        assert_eq!(
            pinned, read_ts,
            "lazy read registration must pin min_active_snapshot_ts to the read ts"
        );
        assert_ne!(before, pinned, "registration must change the pinned min");

        // Finalize unregisters the statement snapshot.
        bound.finalize_operation(true).unwrap();
        let after = storage
            .ctx
            .data_store()
            .with_vertex_tables(|tables| {
                Ok::<Timestamp, crate::core::StorageError>(
                    tables
                        .get(&label)
                        .map(|t| t.min_active_snapshot_ts())
                        .unwrap_or(Timestamp::MAX),
                )
            })
            .unwrap();
        assert_eq!(
            after, before,
            "finalize must unregister the read statement snapshot"
        );
    }
}
