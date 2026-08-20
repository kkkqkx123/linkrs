use crate::core::types::StorageVersion;
use crate::core::types::{
    CommitLsn, Index, IndexConfig, IndexField, IndexGeneration, IndexType, SnapshotTimestamp,
    MAX_TIMESTAMP,
};
use crate::core::Value;
use crate::storage::cursor::{
    IndexCursor, IndexPredicate, IndexRow, IndexScanPlan, PartitionSelector,
};
use crate::storage::index::key_codec::KeyBuilder;
use crate::storage::index::manifest::{
    GenerationBuildState, GenerationState, IndexManifest, IndexShard,
};
use crate::storage::index::types::{EdgeIdentity, IndexIdentity};
use crate::storage::index::*;
use crate::storage::persistence::write_versioned_payload;
use std::collections::BTreeMap;

fn write_crashed_build_state(index_root: &std::path::Path, state: &GenerationBuildState) {
    let serialized = postcard::to_allocvec(state).expect("serialize");
    let mut versioned = Vec::new();
    write_versioned_payload(&mut versioned, StorageVersion::CURRENT as u32, &serialized);
    std::fs::create_dir_all(index_root).unwrap();
    std::fs::write(index_root.join("generation_build.bin"), &versioned).unwrap();
}

fn create_tag_index(name: &str, schema_name: &str) -> Index {
    Index::new(IndexConfig {
        id: 1,
        name: name.to_string(),
        space_id: 1,
        schema_name: schema_name.to_string(),
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
    })
}

fn create_edge_index_with_included_properties() -> Index {
    Index::new(IndexConfig {
        id: 1,
        name: "knows_weight_idx".to_string(),
        space_id: 1,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new("weight".to_string(), Value::Int(0), false)],
        properties: vec!["since".to_string()],
        index_type: IndexType::EdgeIndex,
        is_unique: false,
        covering: true,
        partial_condition: None,
    })
}

/// Register + update + lookup vertex index.
#[test]
fn test_update_and_lookup_vertex_index() {
    let manager = IndexDataManagerImpl::new();

    let space_id = 1u64;
    let vertex_id = Value::Int(1);
    let index_name = "idx_person_name";
    let props = vec![("name".to_string(), Value::string("Alice"))];
    let index = create_tag_index(index_name, "person");

    manager
        .register_native_index(space_id, &index)
        .expect("register native index");
    manager
        .update_vertex_indexes_mvcc(space_id, &vertex_id, index_name, &props, MAX_TIMESTAMP)
        .expect("Failed to update vertex indexes");
    let results = manager
        .lookup_tag_index(space_id, &index, &Value::string("Alice"))
        .expect("Failed to lookup tag index");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], vertex_id);
}

/// Same index id in different spaces uses isolated runtimes.
#[test]
fn same_index_id_in_different_spaces_uses_isolated_runtimes() {
    let manager = IndexDataManagerImpl::new();
    let first = create_tag_index("person_name", "person");
    let mut second = create_tag_index("person_name", "person");
    second.space_id = 2;

    manager
        .register_native_index(1, &first)
        .expect("register first space index");
    manager
        .register_native_index(2, &second)
        .expect("register second space index");
    manager
        .update_vertex_indexes_mvcc(
            1,
            &Value::Int(1),
            "person_name",
            &[("name".to_string(), Value::string("Alice"))],
            MAX_TIMESTAMP,
        )
        .expect("write first space index");
    manager
        .update_vertex_indexes_mvcc(
            2,
            &Value::Int(2),
            "person_name",
            &[("name".to_string(), Value::string("Bob"))],
            MAX_TIMESTAMP,
        )
        .expect("write second space index");

    assert_eq!(
        manager
            .lookup_tag_index(1, &first, &Value::string("Alice"))
            .expect("lookup first space"),
        vec![Value::Int(1)]
    );
    assert_eq!(
        manager
            .lookup_tag_index(2, &second, &Value::string("Bob"))
            .expect("lookup second space"),
        vec![Value::Int(2)]
    );
    assert!(manager.manifest_catalog(1, 1).is_some());
    assert!(manager.manifest_catalog(2, 1).is_some());
}

/// Split places index entries into the correct shards.
#[test]
fn split_writes_only_the_selected_index_to_each_shard() {
    let directory = tempfile::tempdir().expect("create temporary index directory");
    let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
    let first_index = create_tag_index("first", "person");
    let mut second_index = create_tag_index("second", "person");
    second_index.id = 2;
    manager
        .register_native_index(1, &first_index)
        .expect("register first index");
    manager
        .register_native_index(1, &second_index)
        .expect("register second index");

    manager
        .update_vertex_indexes_mvcc(
            1,
            &Value::Int(1),
            "first",
            &[("name".to_string(), Value::string("Alice"))],
            1,
        )
        .expect("write first lower key");
    manager
        .update_vertex_indexes_mvcc(
            1,
            &Value::Int(2),
            "first",
            &[("name".to_string(), Value::string("Zoe"))],
            1,
        )
        .expect("write first upper key");
    manager
        .update_vertex_indexes_mvcc(
            1,
            &Value::Int(3),
            "second",
            &[("name".to_string(), Value::string("Other"))],
            1,
        )
        .expect("write unrelated index key");

    let boundary = KeyBuilder::build_vertex_index_value_prefix(1, "first", &Value::string("M"))
        .expect("build split boundary")
        .0;
    manager
        .split_native_index(
            IndexIdentity {
                space_id: 1,
                index_id: 1,
            },
            boundary,
            SnapshotTimestamp::new(1),
            CommitLsn::new(1),
            || Ok(CommitLsn::new(100)),
            |_, _| Ok(Vec::new()),
        )
        .expect("split first index");

    let catalog = manager.manifest_catalog(1, 1).expect("catalog exists");
    let manifest = catalog.acquire();
    assert_eq!(manifest.manifest().shards.len(), 2);
    let first_prefix = KeyBuilder::build_vertex_index_prefix(1, "first").0;
    let second_prefix = KeyBuilder::build_vertex_index_prefix(1, "second").0;
    let mut shard_entries = 0;
    for shard in &manifest.manifest().shards {
        let shard_runtime =
            crate::storage::index::shard_runtime::ShardRuntime::load_with_pool_capacity(
                shard.checkpoint_file.clone(),
                64 * 1024 * 1024,
            )
            .expect("load split shard");
        let forward = shard_runtime.read_forward().snapshot();
        shard_entries += forward.len();
        assert!(forward.keys().all(|key| key.starts_with(&first_prefix)));
        assert!(forward.keys().all(|key| !key.starts_with(&second_prefix)));
    }
    assert_eq!(shard_entries, 2);
}

// --- Split crash recovery tests ---

#[test]
fn resolve_split_crash_recovery_discards_building_state() {
    let directory = tempfile::tempdir().expect("tempdir");
    let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
    let index = create_tag_index("first", "person");
    manager
        .register_native_index(1, &index)
        .expect("register index");
    manager
        .update_vertex_indexes_mvcc(
            1,
            &Value::Int(1),
            "first",
            &[("name".to_string(), Value::string("Alice"))],
            1,
        )
        .expect("write");

    let index_root = directory.path().join("indexes").join("1").join("1");
    let build_state_path = index_root.join("generation_build.bin");

    let crashed = GenerationBuildState {
        generation: IndexGeneration::new(2),
        snapshot_timestamp: SnapshotTimestamp::new(1),
        start_lsn: CommitLsn::new(10),
        barrier_lsn: None,
        state: GenerationState::Building,
    };
    write_crashed_build_state(&index_root, &crashed);

    manager
        .resolve_split_crash_recovery(&index_root)
        .expect("recovery should succeed");

    assert!(
        !build_state_path.exists(),
        "Building state must be discarded on recovery"
    );

    let results = manager
        .lookup_tag_index(1, &index, &Value::string("Alice"))
        .expect("lookup after recovery");
    assert_eq!(results, vec![Value::Int(1)]);
}

#[test]
fn resolve_split_crash_recovery_discards_catching_up_state() {
    let directory = tempfile::tempdir().expect("tempdir");
    let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
    let index = create_tag_index("first", "person");
    manager
        .register_native_index(1, &index)
        .expect("register index");

    let index_root = directory.path().join("indexes").join("1").join("1");
    let build_state_path = index_root.join("generation_build.bin");

    let crashed = GenerationBuildState {
        generation: IndexGeneration::new(2),
        snapshot_timestamp: SnapshotTimestamp::new(1),
        start_lsn: CommitLsn::new(10),
        barrier_lsn: None,
        state: GenerationState::CatchingUp,
    };
    write_crashed_build_state(&index_root, &crashed);

    manager
        .resolve_split_crash_recovery(&index_root)
        .expect("recovery should succeed");

    assert!(
        !build_state_path.exists(),
        "CatchingUp state must be discarded on recovery"
    );
}

#[test]
fn resolve_split_crash_recovery_completes_publishing_state_with_manifest() {
    let directory = tempfile::tempdir().expect("tempdir");
    let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
    let index = create_tag_index("first", "person");
    manager
        .register_native_index(1, &index)
        .expect("register index");
    manager
        .update_vertex_indexes_mvcc(
            1,
            &Value::Int(1),
            "first",
            &[("name".to_string(), Value::string("Alice"))],
            1,
        )
        .expect("write");

    let index_root = directory.path().join("indexes").join("1").join("1");
    let build_state_path = index_root.join("generation_build.bin");

    let manifest = IndexManifest::new(
        1,
        1,
        IndexGeneration::new(2),
        vec![IndexShard {
            shard_id: 0,
            lower: None,
            upper: None,
            checkpoint_file: index_root.join("generation-2").join("shard-0"),
            checksum: None,
        }],
    )
    .expect("new manifest");
    manifest
        .store(&index_root.join("manifest.bin"))
        .expect("store manifest");

    let publishing = GenerationBuildState {
        generation: IndexGeneration::new(2),
        snapshot_timestamp: SnapshotTimestamp::new(1),
        start_lsn: CommitLsn::new(10),
        barrier_lsn: Some(CommitLsn::new(50)),
        state: GenerationState::Publishing,
    };
    write_crashed_build_state(&index_root, &publishing);

    manager
        .resolve_split_crash_recovery(&index_root)
        .expect("recovery should succeed");

    assert!(
        !build_state_path.exists(),
        "Publishing build state removed after completion"
    );
    assert!(
        index_root.join("manifest.bin").exists(),
        "manifest preserved after Publishing completion"
    );

    let results = manager
        .lookup_tag_index(1, &index, &Value::string("Alice"))
        .expect("lookup after publishing recovery");
    assert_eq!(results, vec![Value::Int(1)]);
}

#[test]
fn resolve_split_crash_recovery_discards_publishing_without_manifest() {
    let directory = tempfile::tempdir().expect("tempdir");
    let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
    let index = create_tag_index("first", "person");
    manager
        .register_native_index(1, &index)
        .expect("register index");

    let index_root = directory.path().join("indexes").join("1").join("1");
    let build_state_path = index_root.join("generation_build.bin");

    let publishing = GenerationBuildState {
        generation: IndexGeneration::new(2),
        snapshot_timestamp: SnapshotTimestamp::new(1),
        start_lsn: CommitLsn::new(10),
        barrier_lsn: Some(CommitLsn::new(50)),
        state: GenerationState::Publishing,
    };
    write_crashed_build_state(&index_root, &publishing);

    manager
        .resolve_split_crash_recovery(&index_root)
        .expect("recovery should succeed");

    assert!(
        !build_state_path.exists(),
        "Publishing state without manifest must be discarded"
    );
}

// --- Included columns MVCC tests ---

#[test]
fn included_columns_visible_in_covering_query_after_update() {
    let manager = IndexDataManagerImpl::new();
    let index = create_edge_index_with_included_properties();
    manager
        .register_native_index(1, &index)
        .expect("register edge index");
    let src = Value::Int(1);
    let dst = Value::Int(2);
    let edge = EdgeIdentity::new(1, &src, &dst, "KNOWS", 0);

    manager
        .update_edge_indexes_mvcc(
            &edge,
            "knows_weight_idx",
            &[
                ("weight".to_string(), Value::Int(10)),
                ("since".to_string(), Value::Int(2020)),
            ],
            10,
        )
        .expect("initial write");

    let covering_plan = IndexScanPlan {
        space: "space".to_string(),
        index_id: 1,
        predicate: IndexPredicate::All,
        partition: PartitionSelector::All,
        partition_id_range: None,
        projection: Some(vec!["since".to_string()]),
        limit: None,
        offset: 0,
        read_timestamp: 10,
    };
    let mut cursor = manager
        .open_edge_index_cursor(1, &index, &covering_plan)
        .expect("cursor");
    let rows: Vec<IndexRow> =
        std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
            .flatten()
            .collect();
    assert_eq!(rows.len(), 1);
    match &rows[0] {
        IndexRow::Covering { columns, .. } => {
            assert_eq!(columns.len(), 1);
            assert_eq!(columns[0], ("since".to_string(), Value::Int(2020)));
        }
        _ => panic!("expected covering row"),
    }

    manager
        .update_edge_indexes_mvcc(
            &edge,
            "knows_weight_idx",
            &[("since".to_string(), Value::Int(2024))],
            20,
        )
        .expect("update");

    let after_update_plan = IndexScanPlan {
        space: "space".to_string(),
        index_id: 1,
        predicate: IndexPredicate::All,
        partition: PartitionSelector::All,
        partition_id_range: None,
        projection: Some(vec!["since".to_string()]),
        limit: None,
        offset: 0,
        read_timestamp: 20,
    };
    let mut cursor = manager
        .open_edge_index_cursor(1, &index, &after_update_plan)
        .expect("cursor after update");
    let rows: Vec<IndexRow> =
        std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
            .flatten()
            .collect();
    assert_eq!(rows.len(), 1);
    match &rows[0] {
        IndexRow::Covering { columns, .. } => {
            assert_eq!(columns[0], ("since".to_string(), Value::Int(2024)));
        }
        _ => panic!("expected covering row after update"),
    }

    let snapshot_plan = IndexScanPlan {
        space: "space".to_string(),
        index_id: 1,
        predicate: IndexPredicate::All,
        partition: PartitionSelector::All,
        partition_id_range: None,
        projection: Some(vec!["since".to_string()]),
        limit: None,
        offset: 0,
        read_timestamp: 10,
    };
    let mut cursor = manager
        .open_edge_index_cursor(1, &index, &snapshot_plan)
        .expect("snapshot cursor");
    let rows: Vec<IndexRow> =
        std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
            .flatten()
            .collect();
    assert_eq!(rows.len(), 1);
    match &rows[0] {
        IndexRow::Covering { columns, .. } => {
            assert_eq!(columns[0], ("since".to_string(), Value::Int(2020)));
        }
        _ => panic!("expected covering row at snapshot"),
    }
}

#[test]
fn included_columns_not_visible_after_delete() {
    let manager = IndexDataManagerImpl::new();
    let index = create_edge_index_with_included_properties();
    manager
        .register_native_index(1, &index)
        .expect("register edge index");
    let src = Value::Int(1);
    let dst = Value::Int(2);
    let edge = EdgeIdentity::new(1, &src, &dst, "KNOWS", 0);

    manager
        .update_edge_indexes_mvcc(
            &edge,
            "knows_weight_idx",
            &[
                ("weight".to_string(), Value::Int(10)),
                ("since".to_string(), Value::Int(2020)),
            ],
            10,
        )
        .expect("write");

    let covering_plan = IndexScanPlan {
        space: "space".to_string(),
        index_id: 1,
        predicate: IndexPredicate::All,
        partition: PartitionSelector::All,
        partition_id_range: None,
        projection: Some(vec!["since".to_string()]),
        limit: None,
        offset: 0,
        read_timestamp: 10,
    };
    let mut cursor = manager
        .open_edge_index_cursor(1, &index, &covering_plan)
        .expect("cursor");
    let rows: Vec<IndexRow> =
        std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
            .flatten()
            .collect();
    assert_eq!(rows.len(), 1, "one edge before delete");

    manager
        .delete_edge_indexes_mvcc(&edge, &["knows_weight_idx".to_string()], 20)
        .expect("delete");

    let after_delete_plan = IndexScanPlan {
        space: "space".to_string(),
        index_id: 1,
        predicate: IndexPredicate::All,
        partition: PartitionSelector::All,
        partition_id_range: None,
        projection: Some(vec!["since".to_string()]),
        limit: None,
        offset: 0,
        read_timestamp: 20,
    };
    let mut cursor = manager
        .open_edge_index_cursor(1, &index, &after_delete_plan)
        .expect("cursor after delete");
    let rows: Vec<IndexRow> =
        std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
            .flatten()
            .collect();
    assert!(
        rows.is_empty(),
        "covering query must not return deleted edge"
    );
}

#[test]
fn included_columns_survive_rebuild_from_snapshot() {
    let directory = tempfile::tempdir().expect("tempdir");
    let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
    let index = create_edge_index_with_included_properties();
    manager
        .register_native_index(1, &index)
        .expect("register index");
    let src = Value::Int(1);
    let dst = Value::Int(2);
    let edge = EdgeIdentity::new(1, &src, &dst, "KNOWS", 0);

    manager
        .update_edge_indexes_mvcc(
            &edge,
            "knows_weight_idx",
            &[
                ("weight".to_string(), Value::Int(10)),
                ("since".to_string(), Value::Int(2020)),
            ],
            10,
        )
        .expect("write");

    let runtime = manager.runtime(1, 1).expect("runtime");
    let catalog = manager.manifest_catalog(1, 1).expect("catalog");
    let manifest = catalog.acquire();
    let generation = runtime
        .generation(manifest.manifest().generation)
        .expect("active generation");
    let mut forward = BTreeMap::new();
    let mut reverse = BTreeMap::new();
    for shard in generation.shards() {
        let (f, r) = shard.snapshot();
        forward.extend(f);
        reverse.extend(r);
    }
    drop(manifest);

    let next_gen = IndexGeneration::new(3);
    let checkpoint_dir = directory
        .path()
        .join("indexes")
        .join("1")
        .join("1")
        .join(format!("generation-{}", next_gen.get()))
        .join("shard-0");
    let fwd_idx = crate::storage::index::chunk::chunked_index::ChunkedIndex::from_btree(
        vec![],
        &forward,
        64 * 1024 * 1024,
    );
    let rev_idx = crate::storage::index::chunk::chunked_index::ChunkedIndex::from_btree(
        vec![],
        &reverse,
        64 * 1024 * 1024,
    );
    crate::storage::index::chunk::serialize::write_chunked_index_checkpoint(
        checkpoint_dir.join("forward_chunks"),
        &fwd_idx,
    )
    .expect("flush forward checkpoint");
    crate::storage::index::chunk::serialize::write_chunked_index_checkpoint(
        checkpoint_dir.join("reverse_chunks"),
        &rev_idx,
    )
    .expect("flush reverse checkpoint");

    let next_manifest = IndexManifest::new(
        1,
        1,
        next_gen,
        vec![IndexShard {
            shard_id: 0,
            lower: None,
            upper: None,
            checkpoint_file: checkpoint_dir,
            checksum: None,
        }],
    )
    .expect("new manifest");
    manager
        .publish_native_index(next_manifest, forward, reverse, CommitLsn::ZERO)
        .expect("publish");

    let covering_plan = IndexScanPlan {
        space: "space".to_string(),
        index_id: 1,
        predicate: IndexPredicate::All,
        partition: PartitionSelector::All,
        partition_id_range: None,
        projection: Some(vec!["since".to_string()]),
        limit: None,
        offset: 0,
        read_timestamp: 10,
    };
    let mut cursor = manager
        .open_edge_index_cursor(1, &index, &covering_plan)
        .expect("cursor after rebuild");
    let rows: Vec<IndexRow> =
        std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
            .flatten()
            .collect();
    assert_eq!(rows.len(), 1, "rebuilt index should have one entry");
    match &rows[0] {
        IndexRow::Covering { columns, .. } => {
            assert_eq!(columns[0], ("since".to_string(), Value::Int(2020)));
        }
        _ => panic!("expected covering row after rebuild"),
    }
}

#[test]
fn wal_recovers_data_after_checkpoint() {
    use crate::core::types::storage_ids::VertexId;
    use crate::core::wal::EntityRef;
    use crate::storage::index::shard_runtime::ShardRuntime;
    use crate::storage::index::types::IndexRecord;

    let temp_dir = std::env::temp_dir().join("graphdb_wal_test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();

    let checkpoint_file = temp_dir.join("checkpoint");
    std::fs::create_dir_all(&checkpoint_file).unwrap();

    // Create a shard and insert data
    let shard = ShardRuntime::empty_with_capacity(checkpoint_file.clone(), 64 * 1024 * 1024);
    let mut forward = BTreeMap::new();
    let mut reverse = BTreeMap::new();

    forward.insert(
        vec![1, 2, 3],
        IndexRecord::new(100)
            .with_entity_ref(EntityRef::Vertex(VertexId::from_int64(42)))
            .with_entity_version(50),
    );
    reverse.insert(
        vec![4, 5, 6],
        IndexRecord::new(100).with_entity_ref(EntityRef::Vertex(VertexId::from_int64(42))),
    );

    shard.replace(forward, reverse);

    // Flush WAL to disk
    shard.flush_wal().unwrap();

    // Verify WAL file exists
    let wal_path = checkpoint_file.join("index.wal");
    assert!(wal_path.exists(), "WAL file should exist after flush_wal");

    // Load a new shard from the same checkpoint - should replay WAL
    let loaded_shard =
        ShardRuntime::load_with_pool_capacity(checkpoint_file.clone(), 128 * 1024 * 1024).unwrap();

    let fwd = loaded_shard.read_forward();
    let rev = loaded_shard.read_reverse();
    assert_eq!(
        fwd.snapshot().len(),
        1,
        "Should have 1 forward entry after WAL replay"
    );
    assert_eq!(
        rev.snapshot().len(),
        1,
        "Should have 1 reverse entry after WAL replay"
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn checkpoint_clears_wal() {
    use crate::storage::index::shard_runtime::ShardRuntime;
    use crate::storage::index::types::IndexRecord;

    let temp_dir = std::env::temp_dir().join("graphdb_checkpoint_test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();

    let checkpoint_file = temp_dir.join("checkpoint");
    std::fs::create_dir_all(&checkpoint_file).unwrap();

    let shard = ShardRuntime::empty_with_capacity(checkpoint_file.clone(), 64 * 1024 * 1024);
    let mut forward = BTreeMap::new();
    forward.insert(vec![1, 2, 3], IndexRecord::new(100));
    shard.replace(forward, BTreeMap::new());

    // Flush WAL
    shard.flush_wal().unwrap();
    let wal_path = checkpoint_file.join("index.wal");
    assert!(wal_path.exists());

    // Checkpoint - should clear WAL
    shard.checkpoint().unwrap();
    assert!(
        !wal_path.exists(),
        "WAL file should be removed after checkpoint"
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn memory_limit_triggers_compaction() {
    let manager = IndexDataManagerImpl::new();
    let index = create_tag_index("name_idx", "Person");

    // Set a very low memory limit to trigger compaction
    manager.set_memory_limit_bytes(1);

    manager
        .register_native_index(1, &index)
        .expect("register index");

    // First update
    manager
        .update_vertex_indexes_mvcc(
            1,
            &Value::Int(1),
            "name_idx",
            &[("name".to_string(), Value::string("Alice"))],
            100,
        )
        .expect("first update");

    // Second update - should trigger compaction due to memory limit
    manager
        .update_vertex_indexes_mvcc(
            1,
            &Value::Int(2),
            "name_idx",
            &[("name".to_string(), Value::string("Bob"))],
            200,
        )
        .expect("second update with memory limit");

    // Verify data is still accessible
    let results = manager
        .lookup_tag_index_mvcc(1, &index, &Value::string("Alice"), 300)
        .expect("lookup Alice");
    assert!(!results.is_empty(), "Alice should be found");

    // Cleanup - reset limit
    manager.set_memory_limit_bytes(0);
}

#[test]
fn retire_generations_reclaims_retired_checkpoint_dirs() {
    let directory = tempfile::tempdir().expect("tempdir");
    let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
    // force per-statement publication so each update below creates a
    // retired generation (the accumulation path is covered elsewhere).
    manager.set_delta_publish_threshold(1);
    let index = create_tag_index("name_idx", "Person");
    manager
        .register_native_index(1, &index)
        .expect("register index");

    manager
        .update_vertex_indexes_mvcc(
            1,
            &Value::Int(1),
            "name_idx",
            &[("name".to_string(), Value::string("Alice"))],
            100,
        )
        .expect("first update");
    manager
        .update_vertex_indexes_mvcc(
            1,
            &Value::Int(2),
            "name_idx",
            &[("name".to_string(), Value::string("Bob"))],
            200,
        )
        .expect("second update");

    // Simulate durable checkpoints for the two retired generations by
    // materializing their shard directories at the manifest-declared paths.
    let catalog = manager
        .manifest_catalog(1, 1)
        .expect("manifest catalog should exist");
    let retired = catalog.retired_reclaimable(|_| true);
    assert_eq!(retired.len(), 2);
    for manifest in &retired {
        for shard in &manifest.shards {
            std::fs::create_dir_all(&shard.checkpoint_file)
                .expect("fixture checkpoint dir should be created");
        }
    }
    let index_root = directory.path().join("indexes").join("1").join("1");
    let gen1 = index_root.join("generation-1");
    let gen2 = index_root.join("generation-2");
    assert!(gen1.is_dir(), "generation 1 checkpoint should exist");
    assert!(gen2.is_dir(), "generation 2 checkpoint should exist");

    // Advancing safe_ts past both retired generations' max_ts removes them
    // from the runtime and reclaims their checkpoint files.
    let retired_count = manager.retire_generations(300);
    assert_eq!(retired_count, 2);

    assert!(
        !gen1.exists(),
        "generation 1 files should be reclaimed after retirement"
    );
    assert!(
        !gen2.exists(),
        "generation 2 files should be reclaimed after retirement"
    );
}

// --- delta accumulation batches generation publication ---

#[test]
fn delta_accumulation_batches_generation_publication() {
    use crate::storage::index::traits::VertexIndexOps;

    let manager = IndexDataManagerImpl::new();
    let index = create_tag_index("name_idx", "Person");
    manager.register_native_index(1, &index).unwrap();

    // Default threshold (64): 200 entries → a handful of generations instead
    // of one per statement.
    for i in 0..200u64 {
        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(i as i32),
                "name_idx",
                &[("name".to_string(), Value::string(format!("person_{i}")))],
                i + 1,
            )
            .unwrap();
    }

    let runtime = manager.runtime(1, 1).unwrap();
    let published = runtime.generations().len();
    assert!(
        published <= 2,
        "200 writes (400 entries) below the 512-entry threshold should stay pending, got {published}"
    );

    // A read flushes any remaining pending delta and sees all writes.
    let results = manager
        .lookup_tag_index(1, &index, &Value::string("person_150"))
        .unwrap();
    assert_eq!(results, vec![Value::Int(150)]);

    // The pending buffer is drained after the read.
    let identity = crate::storage::index::types::IndexIdentity {
        space_id: 1,
        index_id: 1,
    };
    assert_eq!(manager.pending_delta_entries(identity), 0);
}

#[test]
fn delta_accumulation_rollback_path_publishes_per_statement() {
    use crate::storage::index::traits::VertexIndexOps;

    let manager = IndexDataManagerImpl::new();
    manager.set_delta_publish_threshold(1);
    let index = create_tag_index("name_idx", "Person");
    manager.register_native_index(1, &index).unwrap();

    for i in 0..10u64 {
        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(i as i32),
                "name_idx",
                &[("name".to_string(), Value::string(format!("person_{i}")))],
                i + 1,
            )
            .unwrap();
    }

    let runtime = manager.runtime(1, 1).unwrap();
    // 1 base generation from registration + 10 statement publications.
    assert_eq!(
        runtime.generations().len(),
        11,
        "threshold 1 must publish one generation per statement"
    );
}

/// The pending-aware lookup (no generation publish) must agree with the
/// publish-first lookup on live entries, tombstones, and overwrites while the
/// delta is still buffered.
#[test]
fn pending_aware_lookup_matches_published_lookup() {
    use crate::storage::index::traits::VertexIndexOps;

    let manager = IndexDataManagerImpl::new();
    let index = create_tag_index("name_idx", "Person");
    manager.register_native_index(1, &index).unwrap();

    // Writes accumulate in the pending buffer (default threshold 512).
    for i in 0..10u64 {
        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(i as i32),
                "name_idx",
                &[("name".to_string(), Value::string(format!("name_{i}")))],
                i + 1,
            )
            .unwrap();
    }

    // Pending-aware read (no publish) sees the live entries.
    assert_eq!(
        manager
            .lookup_tag_index_pending_aware_mvcc(
                1,
                &index,
                &Value::string("name_3"),
                MAX_TIMESTAMP,
            )
            .unwrap(),
        vec![Value::Int(3)]
    );
    assert_eq!(
        manager
            .lookup_tag_index_pending_aware_mvcc(
                1,
                &index,
                &Value::string("name_9"),
                MAX_TIMESTAMP,
            )
            .unwrap(),
        vec![Value::Int(9)]
    );

    // Publishing and re-reading yields the identical result.
    let published = manager
        .lookup_tag_index_mvcc(1, &index, &Value::string("name_3"), MAX_TIMESTAMP)
        .unwrap();
    assert_eq!(published, vec![Value::Int(3)]);

    // Overwrite: change vid 5 from name_5 to name_50, both deltas stay
    // pending. The old value must be tombstoned and the new value visible.
    manager
        .update_vertex_indexes_mvcc(
            1,
            &Value::Int(5),
            "name_idx",
            &[("name".to_string(), Value::string("name_50"))],
            20,
        )
        .unwrap();
    assert!(
        manager
            .lookup_tag_index_pending_aware_mvcc(
                1,
                &index,
                &Value::string("name_5"),
                MAX_TIMESTAMP,
            )
            .unwrap()
            .is_empty(),
        "old value must be tombstoned while pending"
    );
    assert_eq!(
        manager
            .lookup_tag_index_pending_aware_mvcc(
                1,
                &index,
                &Value::string("name_50"),
                MAX_TIMESTAMP,
            )
            .unwrap(),
        vec![Value::Int(5)]
    );
    // The publish-first read agrees after flushing.
    assert!(manager
        .lookup_tag_index_mvcc(1, &index, &Value::string("name_5"), MAX_TIMESTAMP)
        .unwrap()
        .is_empty());
    assert_eq!(
        manager
            .lookup_tag_index_mvcc(1, &index, &Value::string("name_50"), MAX_TIMESTAMP)
            .unwrap(),
        vec![Value::Int(5)]
    );

    // Tombstone accounting reconciles pending overwrites: name_5's tombstone
    // (added at ts 20) was replaced by a live entry at ts 30, so only the
    // name_50 tombstone remains counted (forward + reverse → 2). Without
    // reconciliation the counter would read 4.
    assert_eq!(
        manager.cached_tombstone_count(),
        2,
        "tombstone counter must reconcile pending overwrites"
    );
}
