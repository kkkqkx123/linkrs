//! Persistence Recovery Tests
//!
//! Tests flush/load data integrity — these complement the WAL recovery
//! tests and multi-cycle flush tests found in scenario.rs and wal_recovery.rs.
//!
//! Unique test coverage:
//! - Flush + vertex update + reload
//! - Flush + edge delete + reload
//! - Flush + index metadata persistence
//! - Empty storage flush + reload

mod common;

use graphdb_storage::core::types::{Index, IndexConfig, IndexField, IndexType, VertexId};
use graphdb_storage::core::vertex_edge_path::Tag;
use graphdb_storage::core::Value;
use graphdb_storage::core::Vertex;
use graphdb_storage::storage::{
    StorageAdmin, StoragePersistenceOps, StorageReader, StorageSchemaOps, StorageWriter,
};

#[test]
fn test_flush_after_vertex_update() {
    let dir = std::env::temp_dir()
        .join("graphdb_storage_persist_test")
        .join("flush_update");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let mut storage = common::open_persistent_storage(&dir);
        let updated = Vertex::new(
            VertexId::from_int64(1),
            vec![Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::BigInt(31)),
                ]
                .into_iter()
                .collect(),
            )],
        );
        storage.update_vertex("test_space", updated).unwrap();
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let storage = common::open_persistent_storage(&dir);
        let alice = storage
            .get_vertex("test_space", &VertexId::from_int64(1))
            .unwrap()
            .unwrap();
        assert_eq!(alice.properties.get("age"), Some(&Value::BigInt(31)));
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_flush_after_edge_delete() {
    let dir = std::env::temp_dir()
        .join("graphdb_storage_persist_test")
        .join("flush_edge_delete");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let mut storage = common::open_persistent_storage(&dir);
        storage
            .delete_edge(
                "test_space",
                &VertexId::from_int64(1),
                &VertexId::from_int64(2),
                "KNOWS",
                0,
            )
            .unwrap();
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let storage = common::open_persistent_storage(&dir);
        let edge = storage
            .get_edge(
                "test_space",
                &VertexId::from_int64(1),
                &VertexId::from_int64(2),
                "KNOWS",
                0,
            )
            .unwrap();
        assert!(edge.is_none(), "Edge should be deleted after reload");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_flush_with_index_metadata() {
    let dir = std::env::temp_dir()
        .join("graphdb_storage_persist_test")
        .join("flush_index");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);

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
        common::insert_test_data(&mut storage, "test_space");
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let storage = common::open_persistent_storage(&dir);
        let indexes = storage.list_tag_indexes("test_space").unwrap();
        assert!(!indexes.is_empty(), "Index metadata should survive flush");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_flush_and_reload_empty_storage() {
    let dir = std::env::temp_dir()
        .join("graphdb_storage_persist_test")
        .join("flush_empty");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let storage = common::open_persistent_storage(&dir);
        assert!(storage.space_exists("test_space"));
    }

    let _ = std::fs::remove_dir_all(&dir);
}
