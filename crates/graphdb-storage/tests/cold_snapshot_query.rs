//! Cold snapshot query integration tests.
//!
//! Verifies that cold snapshot files (`.lkcs`) exported from a live storage
//! are honored by the hot query paths: single-edge lookup, node edge scans,
//! type-wide scans and counts. Edges deleted from the hot engine after the
//! snapshot was taken are served from the cold snapshot.

mod common;

use graphdb_storage::core::types::VertexId;
use graphdb_storage::core::EdgeDirection;
use graphdb_storage::storage::{GraphStorage, StorageReader, StorageWriter};

fn setup_snapshot_pair() -> (tempfile::TempDir, GraphStorage, u64) {
    let dir = common::create_test_workdir();
    let mut storage = common::create_in_memory_storage();
    let space_id = common::setup_basic_schema(&mut storage);
    (dir, storage, space_id)
}

fn export_and_load(storage: &GraphStorage, dir: &std::path::Path, name: &str) {
    let export_ts = storage.version_manager().read_timestamp();
    let path = dir.join(name);
    storage
        .export_cold_snapshot("test_space", "KNOWS", export_ts, &path)
        .expect("export cold snapshot");
    storage
        .load_cold_snapshot(&path)
        .expect("load cold snapshot");
}

#[test]
fn cold_snapshot_get_edge_fallback() {
    let (_dir, mut storage, _space_id) = setup_snapshot_pair();
    common::insert_test_data(&mut storage, "test_space");
    export_and_load(&storage, _dir.path(), "knows.lkcs");

    assert_eq!(storage.list_cold_snapshots().len(), 1);

    // Delete the edge from the hot engine; it should still be served by cold.
    storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .unwrap();

    let edge = storage
        .get_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .unwrap()
        .expect("edge should be served from the cold snapshot");
    assert_eq!(edge.src, VertexId::from_int64(1));
    assert_eq!(edge.dst, VertexId::from_int64(2));
    assert_eq!(edge.ranking, 0);
    assert_eq!(
        edge.props.get("since"),
        Some(&graphdb_storage::core::Value::Int(2020))
    );

    // A non-existent edge still returns None.
    let missing = storage
        .get_edge(
            "test_space",
            &VertexId::from_int64(2),
            &VertexId::from_int64(1),
            "KNOWS",
            0,
        )
        .unwrap();
    assert!(missing.is_none());
}

#[test]
fn cold_snapshot_node_edges_merge() {
    let (_dir, mut storage, _space_id) = setup_snapshot_pair();

    let alice = common::create_person_vertex(1, "Alice", 30);
    let bob = common::create_person_vertex(2, "Bob", 25);
    let carol = common::create_person_vertex(3, "Carol", 35);
    storage.insert_vertex("test_space", alice).unwrap();
    storage.insert_vertex("test_space", bob).unwrap();
    storage.insert_vertex("test_space", carol).unwrap();
    storage
        .insert_edge("test_space", common::create_knows_edge(1, 2, 2020))
        .unwrap();
    storage
        .insert_edge("test_space", common::create_knows_edge(1, 3, 2021))
        .unwrap();

    export_and_load(&storage, _dir.path(), "knows.lkcs");

    // Remove one edge from hot only; the other stays hot.
    storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(3),
            "KNOWS",
            0,
        )
        .unwrap();

    let out_edges = storage
        .get_node_edges("test_space", &VertexId::from_int64(1), EdgeDirection::Out)
        .unwrap();
    assert_eq!(out_edges.len(), 2, "hot + cold edges should be merged");
    let dsts: Vec<_> = out_edges.iter().map(|e| e.dst).collect();
    assert!(dsts.contains(&VertexId::from_int64(2)));
    assert!(dsts.contains(&VertexId::from_int64(3)));
    let since_2021 = out_edges
        .iter()
        .find(|e| e.dst == VertexId::from_int64(3))
        .unwrap();
    assert_eq!(
        since_2021.props.get("since"),
        Some(&graphdb_storage::core::Value::Int(2021))
    );

    // Removing the remaining hot edge keeps it reachable via cold (no duplicate).
    storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .unwrap();
    let out_edges = storage
        .get_node_edges("test_space", &VertexId::from_int64(1), EdgeDirection::Out)
        .unwrap();
    assert_eq!(out_edges.len(), 2, "all edges now served from cold");
}

#[test]
fn cold_snapshot_node_edges_in_and_both() {
    let (_dir, mut storage, _space_id) = setup_snapshot_pair();

    let alice = common::create_person_vertex(1, "Alice", 30);
    let bob = common::create_person_vertex(2, "Bob", 25);
    storage.insert_vertex("test_space", alice).unwrap();
    storage.insert_vertex("test_space", bob).unwrap();
    storage
        .insert_edge("test_space", common::create_knows_edge(1, 2, 2020))
        .unwrap();

    export_and_load(&storage, _dir.path(), "knows.lkcs");
    storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .unwrap();

    let in_edges = storage
        .get_node_edges("test_space", &VertexId::from_int64(2), EdgeDirection::In)
        .unwrap();
    assert_eq!(in_edges.len(), 1);
    assert_eq!(in_edges[0].src, VertexId::from_int64(1));

    let both = storage
        .get_node_edges("test_space", &VertexId::from_int64(1), EdgeDirection::Both)
        .unwrap();
    assert_eq!(both.len(), 1);
    assert_eq!(both[0].dst, VertexId::from_int64(2));
}

/// Phase B: the batched accessors must merge cold snapshots with the same
/// dedup semantics as `get_node_edges`.
#[test]
fn cold_snapshot_batch_accessors_match_get_node_edges() {
    let (_dir, mut storage, _space_id) = setup_snapshot_pair();

    let alice = common::create_person_vertex(1, "Alice", 30);
    let bob = common::create_person_vertex(2, "Bob", 25);
    let carol = common::create_person_vertex(3, "Carol", 35);
    storage.insert_vertex("test_space", alice).unwrap();
    storage.insert_vertex("test_space", bob).unwrap();
    storage.insert_vertex("test_space", carol).unwrap();
    storage
        .insert_edge("test_space", common::create_knows_edge(1, 2, 2020))
        .unwrap();
    storage
        .insert_edge("test_space", common::create_knows_edge(1, 3, 2021))
        .unwrap();

    export_and_load(&storage, _dir.path(), "knows.lkcs");

    // Remove one edge from hot only; the other stays hot. Both must be served
    // (one from hot, one from cold) without duplicates.
    storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(3),
            "KNOWS",
            0,
        )
        .unwrap();

    let seeds = [VertexId::from_int64(1), VertexId::from_int64(2)];
    let direction = EdgeDirection::Out;
    let batch = storage
        .neighbor_dst_ids_batch("test_space", &seeds, direction, &[])
        .unwrap();
    let mut got = batch.clone();
    for row in got.iter_mut() {
        row.sort();
        row.dedup();
    }
    let want: Vec<Vec<VertexId>> = seeds
        .iter()
        .map(|s| {
            let mut dsts: Vec<_> = storage
                .get_node_edges("test_space", s, direction)
                .unwrap()
                .iter()
                .map(|e| e.dst)
                .collect();
            dsts.sort();
            dsts.dedup();
            dsts
        })
        .collect();
    assert_eq!(got, want, "cold + hot neighbor batch mismatch");

    let degrees = storage.out_degree_batch("test_space", &seeds, direction, &[]).unwrap();
    let want_degrees: Vec<usize> = seeds
        .iter()
        .map(|s| storage.get_node_edges("test_space", s, direction).unwrap().len())
        .collect();
    assert_eq!(degrees, want_degrees, "cold + hot degree batch mismatch");
}


#[test]
fn cold_snapshot_scan_edges_by_type() {
    let (_dir, mut storage, _space_id) = setup_snapshot_pair();

    let alice = common::create_person_vertex(1, "Alice", 30);
    let bob = common::create_person_vertex(2, "Bob", 25);
    let carol = common::create_person_vertex(3, "Carol", 35);
    storage.insert_vertex("test_space", alice).unwrap();
    storage.insert_vertex("test_space", bob).unwrap();
    storage.insert_vertex("test_space", carol).unwrap();
    storage
        .insert_edge("test_space", common::create_knows_edge(1, 2, 2020))
        .unwrap();
    storage
        .insert_edge("test_space", common::create_knows_edge(2, 3, 2021))
        .unwrap();

    export_and_load(&storage, _dir.path(), "knows.lkcs");

    // Remove all edges from hot so the scan is served purely by cold.
    storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .unwrap();
    storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(2),
            &VertexId::from_int64(3),
            "KNOWS",
            0,
        )
        .unwrap();

    let edges = storage.scan_edges_by_type("test_space", "KNOWS").unwrap();
    assert_eq!(edges.len(), 2);
    let mut pairs: Vec<(VertexId, VertexId)> = edges.iter().map(|e| (e.src, e.dst)).collect();
    pairs.sort_by_key(|(_, dst)| *dst);
    assert_eq!(pairs[0], (VertexId::from_int64(1), VertexId::from_int64(2)));
    assert_eq!(pairs[1], (VertexId::from_int64(2), VertexId::from_int64(3)));
}

#[test]
fn cold_snapshot_count_edges_by_type() {
    let (_dir, mut storage, _space_id) = setup_snapshot_pair();

    common::insert_test_data(&mut storage, "test_space");
    export_and_load(&storage, _dir.path(), "knows.lkcs");

    // Both edges removed from hot; count comes from the cold snapshot.
    storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .unwrap();

    assert_eq!(
        storage.count_edges_by_type("test_space", "KNOWS").unwrap(),
        1
    );
}

#[test]
fn cold_snapshot_dir_load_and_remove() {
    let (_dir, mut storage, _space_id) = setup_snapshot_pair();
    common::insert_test_data(&mut storage, "test_space");

    export_and_load(&storage, _dir.path(), "knows.lkcs");
    let loaded = storage.list_cold_snapshots();
    assert_eq!(loaded.len(), 1);
    let label = loaded[0];

    storage.remove_cold_snapshot(label);
    assert!(storage.list_cold_snapshots().is_empty());

    // After removal, a hot-deleted edge is gone for good.
    storage
        .delete_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .unwrap();
    let edge = storage
        .get_edge(
            "test_space",
            &VertexId::from_int64(1),
            &VertexId::from_int64(2),
            "KNOWS",
            0,
        )
        .unwrap();
    assert!(edge.is_none());
}
