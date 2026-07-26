//! Generation rebuild recovery tests.

mod common;

use graphdb_storage::core::Value;
use graphdb_storage::storage::{StorageReader, StorageSchemaOps, StorageWriter};

#[test]
fn generation_rebuild_restarts_after_publish_io_failure() {
    let directory = common::create_test_workdir();
    let work_dir = directory.path().to_path_buf();
    let mut storage = common::create_persistent_storage(&work_dir);
    let space_id = common::create_space(&mut storage, "test_space");
    common::create_person_tag(&mut storage, "test_space");
    common::create_person_name_index(&mut storage, "test_space");
    storage
        .insert_vertex("test_space", common::create_person_vertex(1, "Alice", 30))
        .expect("vertex should be inserted");

    // Generation 2 is the first rebuild generation after index registration.
    // A directory at the output file path makes the physical flush fail after
    // the build state has already been persisted.
    let blocked_output = work_dir
        .join("indexes")
        .join(space_id.to_string())
        .join("1")
        .join("generation-2")
        .join("forward_index.bin");
    std::fs::create_dir_all(&blocked_output).expect("failure fixture should be created");

    let result = storage.rebuild_tag_index("test_space", "person_name_idx");
    assert!(
        result.is_err(),
        "the physical output failure should abort rebuild"
    );
    drop(storage);
    std::fs::remove_dir_all(&blocked_output).expect("failure fixture should be removed");

    let mut reopened = common::open_persistent_storage(&work_dir);
    assert!(reopened
        .rebuild_tag_index("test_space", "person_name_idx")
        .expect("rebuild should recover after restart"));
    let indexed = reopened
        .lookup_index(
            "test_space",
            "person_name_idx",
            &Value::string("Alice"),
        )
        .expect("rebuilt index should be readable");
    assert_eq!(
        indexed,
        vec![Value::from(
            graphdb_storage::core::types::VertexId::from_int64(1)
        )]
    );
}
