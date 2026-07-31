//! Online native-index split tests.

mod common;

use graphdb_storage::core::types::{CommitLsn, IndexGeneration, SnapshotTimestamp};
use graphdb_storage::core::Value;
use graphdb_storage::storage::{
    GenerationBuildState, GenerationState, StoragePersistenceOps, StorageReader, StorageSchemaOps,
    StorageWriter,
};
use std::thread;

#[test]
fn split_uses_persisted_manifest_and_survives_restart() {
    let directory = common::create_test_workdir();
    let work_dir = directory.path().to_path_buf();

    {
        let mut storage = common::create_persistent_storage(&work_dir);
        common::create_space(&mut storage, "test_space");
        common::create_person_tag(&mut storage, "test_space");
        common::create_person_name_index(&mut storage, "test_space");
        for (id, name) in [(1, "Alice"), (2, "Mike"), (3, "Zoe")] {
            storage
                .insert_vertex(
                    "test_space",
                    common::create_person_vertex(id, name, 20 + id),
                )
                .expect("vertex should be inserted");
        }
        storage
            .rebuild_tag_index("test_space", "person_name_idx")
            .expect("index should be rebuilt");
        storage
            .split_native_index_at_value("test_space", "person_name_idx", &Value::string("M"))
            .expect("index split should succeed");
        storage.flush().expect("split state should be durable");

        for name in ["Alice", "Mike", "Zoe"] {
            let rows = storage
                .lookup_index("test_space", "person_name_idx", &Value::string(name))
                .expect("lookup should succeed");
            assert_eq!(rows.len(), 1, "lookup should find {name}");
        }
    }

    println!("split test work directory: {}", work_dir.display());
    std::mem::forget(directory);
    let storage = graphdb_storage::storage::GraphStorage::open_with_persistence(
        work_dir.clone(),
        false,
        None,
    )
    .expect("storage should reopen");
    for name in ["Alice", "Mike", "Zoe"] {
        let rows = storage
            .lookup_index("test_space", "person_name_idx", &Value::string(name))
            .expect("lookup after restart should succeed");
        assert_eq!(rows.len(), 1, "restarted lookup should find {name}");
    }
}

#[test]
fn split_and_concurrent_write_preserve_index_entries() {
    let directory = common::create_test_workdir();
    let work_dir = directory.path().to_path_buf();
    let mut storage = common::create_persistent_storage(&work_dir);
    common::create_space(&mut storage, "test_space");
    common::create_person_tag(&mut storage, "test_space");
    common::create_person_name_index(&mut storage, "test_space");
    for id in 1..=100 {
        storage
            .insert_vertex(
                "test_space",
                common::create_person_vertex(id, &format!("Person{id:03}"), 20),
            )
            .expect("vertex should be inserted");
    }
    storage
        .rebuild_tag_index("test_space", "person_name_idx")
        .expect("index should be rebuilt");

    let writer = storage.clone();
    let writer = thread::spawn(move || {
        let mut writer = writer;
        writer
            .insert_vertex(
                "test_space",
                common::create_person_vertex(101, "Concurrent", 30),
            )
            .expect("concurrent vertex should be inserted");
    });
    storage
        .split_native_index_at_value("test_space", "person_name_idx", &Value::string("Person050"))
        .expect("concurrent split should succeed");
    writer.join().expect("writer should finish");

    let rows = storage
        .lookup_index(
            "test_space",
            "person_name_idx",
            &Value::string("Concurrent"),
        )
        .expect("concurrent lookup should succeed");
    assert_eq!(rows.len(), 1);
}

#[test]
fn split_startup_reconciles_publishing_state() {
    let directory = common::create_test_workdir();
    let work_dir = directory.path().to_path_buf();

    {
        let mut storage = common::create_persistent_storage(&work_dir);
        common::create_space(&mut storage, "test_space");
        common::create_person_tag(&mut storage, "test_space");
        common::create_person_name_index(&mut storage, "test_space");
        for (id, name) in [(1, "Alice"), (2, "Zoe")] {
            storage
                .insert_vertex(
                    "test_space",
                    common::create_person_vertex(id, name, 20 + id),
                )
                .expect("vertex should be inserted");
        }
        storage
            .rebuild_tag_index("test_space", "person_name_idx")
            .expect("index should be rebuilt");
        storage
            .split_native_index_at_value("test_space", "person_name_idx", &Value::string("M"))
            .expect("index split should succeed");
        storage.flush().expect("split state should be durable");

        let index_root = work_dir.join("indexes").join("1").join("1");
        let build_state = GenerationBuildState {
            generation: IndexGeneration::new(2),
            snapshot_timestamp: SnapshotTimestamp::new(1),
            start_lsn: CommitLsn::new(1),
            barrier_lsn: Some(CommitLsn::new(1)),
            state: GenerationState::Publishing,
            terminal_reason: None,
        };
        std::fs::write(
            index_root.join("generation_build.bin"),
            postcard::to_allocvec(&build_state).expect("build state should serialize"),
        )
        .expect("build state should be durable");
    }

    let storage = graphdb_storage::storage::GraphStorage::open(work_dir.clone())
        .expect("storage should recover split state");
    assert!(
        !work_dir
            .join("indexes")
            .join("1")
            .join("1")
            .join("generation_build.bin")
            .exists(),
        "published split state should be reconciled on startup"
    );
    for name in ["Alice", "Zoe"] {
        let rows = storage
            .lookup_index("test_space", "person_name_idx", &Value::string(name))
            .expect("lookup after split recovery should succeed");
        assert_eq!(rows.len(), 1, "lookup should find {name}");
    }
}
