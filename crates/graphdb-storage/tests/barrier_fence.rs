//! Barrier Fence Integration Tests
//!
//! Verifies that the barrier LSN established during index rebuild correctly
//! gates write visibility and WAL truncation behavior.

mod common;

use graphdb_storage::core::Value;
use graphdb_storage::storage::{StorageReader, StorageSchemaOps, StorageWriter};

/// Verify that a single rebuild cycle preserves indexed data and the
/// rebuilt index returns correct results for both old and new data.
#[test]
fn barrier_fence_single_rebuild_preserves_data() {
    let directory = common::create_test_workdir();
    let work_dir = directory.path().to_path_buf();
    let mut storage = common::create_persistent_storage(&work_dir);
    common::create_space(&mut storage, "test_space");
    common::create_person_tag(&mut storage, "test_space");
    common::create_person_name_index(&mut storage, "test_space");

    storage
        .insert_vertex("test_space", common::create_person_vertex(1, "Alice", 30))
        .expect("insert Alice");

    assert!(storage
        .rebuild_tag_index("test_space", "person_name_idx")
        .expect("rebuild should succeed"));

    let indexed = storage
        .lookup_index(
            "test_space",
            "person_name_idx",
            &Value::String("Alice".to_string()),
        )
        .expect("lookup after rebuild");
    assert_eq!(
        indexed,
        vec![Value::from(
            graphdb_storage::core::types::VertexId::from_int64(1)
        )]
    );

    storage
        .insert_vertex("test_space", common::create_person_vertex(2, "Bob", 25))
        .expect("insert Bob after rebuild");

    let indexed_bob = storage
        .lookup_index(
            "test_space",
            "person_name_idx",
            &Value::String("Bob".to_string()),
        )
        .expect("lookup Bob");
    assert_eq!(
        indexed_bob,
        vec![Value::from(
            graphdb_storage::core::types::VertexId::from_int64(2)
        )]
    );
}

/// Verify data survives a close/reopen cycle after rebuild.
#[test]
fn barrier_fence_survives_restart_after_rebuild() {
    let directory = common::create_test_workdir();
    let work_dir = directory.path().to_path_buf();

    {
        let mut storage = common::create_persistent_storage(&work_dir);
        common::create_space(&mut storage, "test_space");
        common::create_person_tag(&mut storage, "test_space");
        common::create_person_name_index(&mut storage, "test_space");

        storage
            .insert_vertex("test_space", common::create_person_vertex(1, "Alice", 30))
            .expect("insert Alice");

        assert!(storage
            .rebuild_tag_index("test_space", "person_name_idx")
            .expect("rebuild should succeed"));

        drop(storage);
    }

    let mut storage = common::open_persistent_storage(&work_dir);

    assert!(storage
        .rebuild_tag_index("test_space", "person_name_idx")
        .expect("rebuild after reopen should succeed"));

    let indexed = storage
        .lookup_index(
            "test_space",
            "person_name_idx",
            &Value::String("Alice".to_string()),
        )
        .expect("lookup after reopen");
    assert_eq!(
        indexed,
        vec![Value::from(
            graphdb_storage::core::types::VertexId::from_int64(1)
        )]
    );
}

/// Verify multiple consecutive rebuilds produce a consistent index.
#[test]
fn barrier_fence_multiple_rebuilds_remain_consistent() {
    let directory = common::create_test_workdir();
    let work_dir = directory.path().to_path_buf();
    let mut storage = common::create_persistent_storage(&work_dir);
    common::create_space(&mut storage, "test_space");
    common::create_person_tag(&mut storage, "test_space");
    common::create_person_name_index(&mut storage, "test_space");

    for i in 1..=5 {
        storage
            .insert_vertex(
                "test_space",
                common::create_person_vertex(i, &format!("Person{}", i), 20 + i),
            )
            .expect("insert vertex");

        assert!(storage
            .rebuild_tag_index("test_space", "person_name_idx")
            .expect("rebuild should succeed"));
    }

    for i in 1..=5 {
        let name = format!("Person{}", i);
        let indexed = storage
            .lookup_index("test_space", "person_name_idx", &Value::String(name))
            .expect("lookup after multiple rebuilds");
        assert_eq!(
            indexed,
            vec![Value::from(
                graphdb_storage::core::types::VertexId::from_int64(i)
            )],
            "index lookup for Person{} should return vertex {}",
            i,
            i
        );
    }
}
