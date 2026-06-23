//! WAL Recovery Integration Tests
//!
//! Tests the WAL (Write-Ahead Log) replay mechanism after various
//! crash scenarios, ensuring the storage can properly recover
//! and maintain consistency.
//!
//! IMPORTANT: save_to_disk() persists state but does NOT advance the
//! WAL checkpoint. To avoid replaying schema setup on reopen we must
//! call create_checkpoint() after save_to_disk().

mod common;

use graphdb_storage::core::types::VertexId;
use graphdb_storage::core::vertex_edge_path::Tag;
use graphdb_storage::core::Value;
use graphdb_storage::core::Vertex;
use graphdb_storage::storage::{StorageAdmin, StoragePersistenceOps, StorageReader, StorageWriter};

/// Helper: save and checkpoint so that prior WAL entries are not replayed.
fn save_and_checkpoint(storage: &mut graphdb_storage::storage::GraphStorage) {
    storage.save_to_disk().expect("save_to_disk failed");
    storage
        .create_checkpoint()
        .expect("create_checkpoint failed");
}

/// Write data, crash (drop) without saving, reopen and verify
/// the data is recovered via WAL.
#[test]
fn test_crash_without_flush_loses_uncommitted_data() {
    let dir = std::env::temp_dir()
        .join("graphdb_storage_wal_test")
        .join("crash_no_flush");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let vid: VertexId;
    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");
        save_and_checkpoint(&mut storage);

        // Insert one more vertex AFTER save+checkpoint
        let extra = Vertex::new(
            VertexId::from_int64(3),
            vec![Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::String("Extra".to_string()))]
                    .into_iter()
                    .collect(),
            )],
        );
        vid = storage.insert_vertex("test_space", extra).unwrap();
    }

    // Reopen — WAL should replay the unflushed insert
    {
        let storage = common::open_persistent_storage(&dir);
        common::verify_test_data(&storage, "test_space");

        let extra_vertex = storage.get_vertex("test_space", &vid).unwrap();
        assert!(
            extra_vertex.is_some(),
            "WAL should have replayed the extra vertex insert"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Test recovery of edge insertions after crash.
#[test]
fn test_crash_recovery_replays_edge_insert() {
    let dir = std::env::temp_dir()
        .join("graphdb_storage_wal_test")
        .join("edge_recovery");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");
        save_and_checkpoint(&mut storage);

        // Insert a new vertex and edge after save+checkpoint
        let charlie = common::create_person_vertex(3, "Charlie", 35);
        storage.insert_vertex("test_space", charlie).unwrap();

        let edge = common::create_knows_edge(1, 3, 2022);
        storage.insert_edge("test_space", edge).unwrap();
    }

    {
        let storage = common::open_persistent_storage(&dir);

        // Edge should exist after WAL replay
        let edge = storage
            .get_edge(
                "test_space",
                &VertexId::from_int64(1),
                &VertexId::from_int64(3),
                "KNOWS",
                0,
            )
            .unwrap();
        assert!(edge.is_some(), "Edge should be recovered via WAL");
        assert_eq!(edge.as_ref().unwrap().ranking, 0);

        let charlie = storage
            .get_vertex("test_space", &VertexId::from_int64(3))
            .unwrap();
        assert!(charlie.is_some(), "Vertex should be recovered via WAL");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Test recovery of vertex deletion after crash.
#[test]
fn test_crash_recovery_replays_vertex_delete() {
    let dir = std::env::temp_dir()
        .join("graphdb_storage_wal_test")
        .join("delete_recovery");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");
        save_and_checkpoint(&mut storage);

        // Delete Alice after save+checkpoint
        storage
            .delete_vertex("test_space", &VertexId::from_int64(1))
            .unwrap();
    }

    {
        let storage = common::open_persistent_storage(&dir);

        let alice = storage
            .get_vertex("test_space", &VertexId::from_int64(1))
            .unwrap();
        assert!(
            alice.is_none(),
            "WAL should have replayed the vertex delete"
        );

        let bob = storage
            .get_vertex("test_space", &VertexId::from_int64(2))
            .unwrap();
        assert!(bob.is_some(), "Bob should still exist");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Test recovery of schema DDL operations after crash.
#[test]
fn test_crash_recovery_replays_tag_creation() {
    let dir = std::env::temp_dir()
        .join("graphdb_storage_wal_test")
        .join("schema_recovery");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::create_space(&mut storage, "test_space");
        save_and_checkpoint(&mut storage);

        // Create tag after save+checkpoint — this goes to WAL
        common::create_person_tag(&mut storage, "test_space");
    }

    {
        let storage = common::open_persistent_storage(&dir);

        let tag = storage.get_tag("test_space", "Person").unwrap();
        assert!(tag.is_some(), "Tag should be recovered via WAL replay");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Test recovery of edge deletion after crash.
#[test]
fn test_crash_recovery_replays_edge_delete() {
    let dir = std::env::temp_dir()
        .join("graphdb_storage_wal_test")
        .join("edge_delete_recovery");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");
        save_and_checkpoint(&mut storage);

        // Delete edge after save+checkpoint
        storage
            .delete_edge(
                "test_space",
                &VertexId::from_int64(1),
                &VertexId::from_int64(2),
                "KNOWS",
                0,
            )
            .unwrap();
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
        assert!(edge.is_none(), "WAL should have replayed the edge delete");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Multiple crash-recovery cycles to ensure WAL position tracking works.
#[test]
fn test_multiple_crash_recovery_cycles() {
    let dir = std::env::temp_dir()
        .join("graphdb_storage_wal_test")
        .join("multi_cycle");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Cycle 1: setup schema, save+checkpoint, insert Alice, crash
    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        save_and_checkpoint(&mut storage);

        let alice = common::create_person_vertex(1, "Alice", 30);
        storage.insert_vertex("test_space", alice).unwrap();
    }

    // Recover, save+checkpoint, insert Bob, crash
    {
        let mut storage = common::open_persistent_storage(&dir);
        assert!(storage
            .get_vertex("test_space", &VertexId::from_int64(1))
            .unwrap()
            .is_some());

        save_and_checkpoint(&mut storage);

        let bob = common::create_person_vertex(2, "Bob", 25);
        storage.insert_vertex("test_space", bob).unwrap();
    }

    // Recover, verify both
    {
        let storage = common::open_persistent_storage(&dir);
        assert!(storage
            .get_vertex("test_space", &VertexId::from_int64(1))
            .unwrap()
            .is_some());
        assert!(storage
            .get_vertex("test_space", &VertexId::from_int64(2))
            .unwrap()
            .is_some());
    }

    let _ = std::fs::remove_dir_all(&dir);
}
