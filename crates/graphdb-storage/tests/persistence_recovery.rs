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

use graphdb_core::types::{Index, IndexConfig, IndexField, IndexType, VertexId};
use graphdb_core::vertex_edge_path::Tag;
use graphdb_core::Value;
use graphdb_core::Vertex;
use graphdb_storage::{
    StorageAdmin, StoragePersistenceOps, StorageReader, StorageSchemaOps, StorageWriter,
};

#[test]
fn test_flush_after_vertex_update() {
    let temp_dir = common::create_test_workdir();
    let dir = temp_dir.path();

    {
        let mut storage = common::create_persistent_storage(dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let mut storage = common::open_persistent_storage(dir);
        let updated = Vertex::new(
            VertexId::try_from_int64(1).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string("Alice")),
                    ("age".to_string(), Value::BigInt(31)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        storage.update_vertex("test_space", updated).unwrap();
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let storage = common::open_persistent_storage(dir);
        let alice = storage
            .get_vertex(
                "test_space",
                "Person",
                &VertexId::try_from_int64(1).expect("test vertex id"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(alice.properties().get("age"), Some(&Value::BigInt(31)));
    }
}

#[test]
fn test_flush_after_edge_delete() {
    let temp_dir = common::create_test_workdir();
    let dir = temp_dir.path();

    {
        let mut storage = common::create_persistent_storage(dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let mut storage = common::open_persistent_storage(dir);
        storage
            .delete_edge(
                "test_space",
                &VertexId::try_from_int64(1).expect("test vertex id"),
                &VertexId::try_from_int64(2).expect("test vertex id"),
                "KNOWS",
                0,
            )
            .unwrap();
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let storage = common::open_persistent_storage(dir);
        let edge = storage
            .get_edge(
                "test_space",
                &VertexId::try_from_int64(1).expect("test vertex id"),
                &VertexId::try_from_int64(2).expect("test vertex id"),
                "KNOWS",
                0,
            )
            .unwrap();
        assert!(edge.is_none(), "Edge should be deleted after reload");
    }
}

#[test]
fn test_flush_with_index_metadata() {
    let temp_dir = common::create_test_workdir();
    let dir = temp_dir.path();

    {
        let mut storage = common::create_persistent_storage(dir);
        common::setup_basic_schema(&mut storage);

        let index = Index::new(IndexConfig {
            id: 1,
            name: "person_name_idx".to_string(),
            space_id: 0,
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
        common::insert_test_data(&mut storage, "test_space");
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let storage = common::open_persistent_storage(dir);
        let indexes = storage.list_tag_indexes("test_space").unwrap();
        assert!(!indexes.is_empty(), "Index metadata should survive flush");
    }
}

/// Collect every `commit_manifest.json` under `dir`: the vertex table
/// commit points whose listed files the open path must trust.
fn collect_commit_manifests(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.file_name().and_then(|n| n.to_str()) == Some("commit_manifest.json") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

#[test]
fn test_corrupt_commit_manifest_refuses_open() {
    let temp_dir = common::create_test_workdir();
    let dir = temp_dir.path();

    {
        let mut storage = common::create_persistent_storage(dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    let manifests = collect_commit_manifests(dir);
    assert!(
        !manifests.is_empty(),
        "checkpoint should persist at least one vertex commit manifest"
    );
    // Corrupt every copy (data dir and checkpoint dirs): whichever copy the
    // open path trusts is corrupt, so a strict loader must refuse the open.
    // Copies the loader never reads are harmless to corrupt.
    for target in &manifests {
        let mut bytes = std::fs::read(target).expect("commit manifest should be readable");
        assert!(!bytes.is_empty(), "commit manifest should not be empty");
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xff;
        std::fs::write(target, &bytes).expect("commit manifest should be writable");
    }

    let reopened = graphdb_storage::GraphStorage::open(dir.to_path_buf());
    assert!(
        reopened.is_err(),
        "open must refuse a table whose commit manifest is corrupt"
    );
}

#[test]
fn test_flush_and_reload_empty_storage() {
    let temp_dir = common::create_test_workdir();
    let dir = temp_dir.path();

    {
        let mut storage = common::create_persistent_storage(dir);
        common::setup_basic_schema(&mut storage);
        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();
    }

    {
        let storage = common::open_persistent_storage(dir);
        assert!(storage.space_exists("test_space"));
    }
}
