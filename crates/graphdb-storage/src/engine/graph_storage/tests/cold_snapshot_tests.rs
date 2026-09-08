use super::*;

#[test]
fn cold_snapshot_delta_time_machine_merge_flow() {
    use crate::client::StorageSnapshotOps;

    let mut storage = create_test_storage();
    let temp_dir = tempfile::TempDir::new().unwrap();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    let edge_type = graphdb_core::types::EdgeTypeInfo::new("KNOWS".to_string())
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
    let edge_set =
        |snapshot: &crate::cold::ColdSnapshot| -> std::collections::HashSet<(u32, i64)> {
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
