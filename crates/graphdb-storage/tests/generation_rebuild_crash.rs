//! Generation rebuild recovery tests.

mod common;

use graphdb_core::Value;
use graphdb_storage::{StorageReader, StorageSchemaOps, StorageWriter};

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

    // Index registration auto-rebuilds (publishes generation-2 on disk), and
    // insert_vertex publishes an in-memory delta generation, so the next
    // explicit rebuild targets max on-disk generation + 2 (+ 1 if no delta).
    // Block the output file with a directory for both candidates so the
    // physical flush fails after the build state has already been persisted,
    // regardless of the exact generation numbering.
    let gen_root = work_dir
        .join("indexes")
        .join(space_id.to_string())
        .join("1");
    let max_gen = std::fs::read_dir(&gen_root)
        .expect("index root should exist")
        .filter_map(|entry| {
            entry.ok().and_then(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                name.strip_prefix("generation-")
                    .and_then(|n| n.parse::<u64>().ok())
            })
        })
        .max()
        .unwrap_or(0);
    let mut blocked_outputs = Vec::new();
    for generation in (max_gen + 1)..=(max_gen + 2) {
        let gen_dir = gen_root.join(format!("generation-{generation}"));
        std::fs::write(&gen_dir, b"blocked").expect("failure fixture should be created");
        blocked_outputs.push(gen_dir);
    }

    let result = storage.rebuild_tag_index("test_space", "person_name_idx");
    assert!(
        result.is_err(),
        "the physical output failure should abort rebuild"
    );
    drop(storage);
    for blocked in &blocked_outputs {
        std::fs::remove_file(blocked).expect("failure fixture should be removed");
    }

    let mut reopened = common::open_persistent_storage(&work_dir);
    assert!(reopened
        .rebuild_tag_index("test_space", "person_name_idx")
        .expect("rebuild should recover after restart"));
    let indexed = reopened
        .lookup_index("test_space", "person_name_idx", &Value::string("Alice"))
        .expect("rebuilt index should be readable");
    assert_eq!(
        indexed,
        vec![Value::from(
            graphdb_core::types::VertexId::from_int64(1)
        )]
    );
}
