use super::*;

#[test]
fn test_trigger_background_freeze_execution() {
    let mut storage = create_test_storage();
    let _space_id = setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    // Insert vertices
    let alice = VertexId::from_int64(1);
    let bob = VertexId::from_int64(2);

    let v1 = Vertex {
        vid: alice,
        id: 0,
        tags: vec![Tag::new(
            "Person".to_string(),
            [("name".to_string(), Value::string("Alice"))]
                .iter()
                .cloned()
                .collect(),
        )],
        properties: [("name".to_string(), Value::string("Alice"))]
            .iter()
            .cloned()
            .collect(),
    };

    let v2 = Vertex {
        vid: bob,
        id: 0,
        tags: vec![Tag::new(
            "Person".to_string(),
            [("name".to_string(), Value::string("Bob"))]
                .iter()
                .cloned()
                .collect(),
        )],
        properties: [("name".to_string(), Value::string("Bob"))]
            .iter()
            .cloned()
            .collect(),
    };

    storage.insert_vertex("test_space", v1).unwrap();
    storage.insert_vertex("test_space", v2).unwrap();

    // Insert edge
    let edge = Edge {
        src: alice,
        dst: bob,
        edge_type: "KNOWS".to_string(),
        ranking: 0,
        props: [("since".to_string(), Value::Int(2020))]
            .iter()
            .cloned()
            .collect(),
    };

    storage.insert_edge("test_space", edge).unwrap();

    // Trigger freeze - should succeed
    let result = storage.trigger_background_freeze();
    assert!(result.is_ok(), "Freeze should succeed: {:?}", result.err());
}

#[test]
fn test_cleanup_threshold_gc_integration() {
    // Test that compaction uses cleanup_threshold from SnapshotTracker
    let (_, mut storage) = create_persistent_storage();
    let _space_id = setup_space(&mut storage);
    let _person_tag = setup_person_tag(&mut storage);
    let _knows_edge = setup_knows_edge(&mut storage);

    // Create vertices
    let alice = VertexId::from_int64(1);

    // Insert vertices
    let v1 = Vertex {
        vid: alice,
        id: 0,
        tags: vec![Tag::new(
            "Person".to_string(),
            [("name".to_string(), Value::string("Alice"))]
                .iter()
                .cloned()
                .collect(),
        )],
        properties: [("name".to_string(), Value::string("Alice"))]
            .iter()
            .cloned()
            .collect(),
    };

    storage.insert_vertex("test_space", v1).unwrap();

    // Verify SnapshotTracker is accessible through VersionManager
    let version_manager = storage.ctx.version_manager().clone();
    let snapshot_tracker = version_manager.snapshot_tracker();

    // Before any snapshots, cleanup_threshold should be MAX
    let initial_threshold = snapshot_tracker.cleanup_threshold();
    assert_eq!(
        initial_threshold,
        Timestamp::MAX,
        "Initial cleanup_threshold should be Timestamp::MAX"
    );

    // Acquire a read timestamp (creates a snapshot)
    let read_ts = version_manager
        .acquire_read_timestamp()
        .expect("Failed to acquire read timestamp");
    assert!(read_ts > 0, "Read timestamp should be valid");

    // Now cleanup_threshold should equal the read timestamp
    let threshold_with_active = snapshot_tracker.cleanup_threshold();
    assert_eq!(
        threshold_with_active, read_ts,
        "cleanup_threshold should equal active read timestamp"
    );

    // Release the read timestamp
    version_manager.release_read_timestamp();

    // After releasing, cleanup_threshold should be MAX again
    let final_threshold = snapshot_tracker.cleanup_threshold();
    assert_eq!(
        final_threshold,
        Timestamp::MAX,
        "Final cleanup_threshold should be Timestamp::MAX after releasing"
    );

    // Verify compaction works (it uses cleanup_threshold internally)
    let result = storage.compact(&Default::default());
    assert!(
        result.is_ok(),
        "Compaction should succeed with cleanup_threshold: {:?}",
        result.err()
    );
}

#[test]
fn test_snapshot_tracker_cleanup_threshold_multiple_readers() {
    // Test cleanup_threshold with multiple concurrent read transactions
    let storage = create_test_storage();
    let version_manager = storage.ctx.version_manager().clone();
    let snapshot_tracker = version_manager.snapshot_tracker();

    // Initially, no active snapshots
    assert_eq!(snapshot_tracker.cleanup_threshold(), Timestamp::MAX);
    assert_eq!(snapshot_tracker.active_count(), 0);

    // Acquire multiple read timestamps
    let ts1 = version_manager
        .acquire_read_timestamp()
        .expect("Failed to acquire read timestamp");
    let ts2 = version_manager
        .acquire_read_timestamp()
        .expect("Failed to acquire read timestamp");
    let ts3 = version_manager
        .acquire_read_timestamp()
        .expect("Failed to acquire read timestamp");

    // All should use the same read_ts (due to MVCC design)
    assert_eq!(ts1, ts2);
    assert_eq!(ts2, ts3);

    // cleanup_threshold should be the minimum active
    assert_eq!(snapshot_tracker.cleanup_threshold(), ts1);

    // Reference count should be 3
    assert_eq!(snapshot_tracker.ref_count(ts1), Some(3));

    // Release one
    version_manager.release_read_timestamp();
    assert_eq!(snapshot_tracker.ref_count(ts1), Some(2));
    assert_eq!(snapshot_tracker.cleanup_threshold(), ts1); // Still active

    // Release another
    version_manager.release_read_timestamp();
    assert_eq!(snapshot_tracker.ref_count(ts1), Some(1));

    // Release the last one
    version_manager.release_read_timestamp();
    assert_eq!(snapshot_tracker.active_count(), 0);
    assert_eq!(snapshot_tracker.cleanup_threshold(), Timestamp::MAX);
}

#[test]
fn test_compact_maintenance_propagates_vertex_remap_to_edge_tables() {
    // Regression: vertex compaction densifies internal IDs; the old-to-new
    // mapping must be propagated into edge CSR rows/neighbors or every
    // edge referencing a surviving vertex breaks.
    let (_, mut storage) = create_persistent_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    // 100 vertices (internal ids 0..99 in insertion order).
    for i in 1..=100i64 {
        insert_test_vertex(&mut storage, i, &format!("v{i}"));
    }

    // Edges only between vertices that will survive compaction:
    // odd pairs (1,3), (3,5), ..., (77,79) and (79,81), ..., (97,99).
    let expected_edges: Vec<(i64, i64)> = (1..=97).step_by(2).map(|src| (src, src + 2)).collect();
    for (src, dst) in &expected_edges {
        let edge = Edge::new(
            VertexId::from_int64(*src),
            VertexId::from_int64(*dst),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        );
        storage.insert_edge("test_space", edge).unwrap();
    }

    // Delete 40 vertices (external ids 2..80 step 2); their internal ids
    // are interleaved with survivors, forcing a real ID remap.
    for i in (2..=80).step_by(2) {
        storage
            .delete_vertex("test_space", &VertexId::from_int64(i))
            .unwrap();
    }

    let vertex_count_before = storage
        .ctx
        .data_store()
        .with_vertex_tables(|tables| {
            Ok::<usize, graphdb_core::StorageError>(
                tables.values().map(|t| t.total_count()).sum::<usize>(),
            )
        })
        .unwrap();
    assert_eq!(vertex_count_before, 100);

    storage.compact(&Default::default()).unwrap();

    // 40 vertices removed by compaction.
    let vertex_count_after = storage
        .ctx
        .data_store()
        .with_vertex_tables(|tables| {
            Ok::<usize, graphdb_core::StorageError>(
                tables.values().map(|t| t.total_count()).sum::<usize>(),
            )
        })
        .unwrap();
    assert_eq!(
        vertex_count_after, 60,
        "compaction should remove 40 vertices"
    );

    // Deleted vertices no longer resolve.
    assert!(storage
        .get_vertex("test_space", &VertexId::from_int64(2))
        .unwrap()
        .is_none());

    // Every surviving edge still resolves through the remapped edge CSR.
    for (src, dst) in &expected_edges {
        let retrieved = storage
            .get_edge(
                "test_space",
                &VertexId::from_int64(*src),
                &VertexId::from_int64(*dst),
                "KNOWS",
                0,
            )
            .unwrap();
        assert!(
            retrieved.is_some(),
            "edge {src}->{dst} lost after vertex compaction remap"
        );
    }

    // Node-edge scans resolve through remapped out/in CSRs.
    let out_edges = storage
        .get_node_edges("test_space", &VertexId::from_int64(1), EdgeDirection::Out)
        .unwrap();
    assert_eq!(out_edges.len(), 1);
    assert_eq!(out_edges[0].dst, VertexId::from_int64(3));
    let in_edges = storage
        .get_node_edges("test_space", &VertexId::from_int64(79), EdgeDirection::In)
        .unwrap();
    assert_eq!(in_edges.len(), 1);
    assert_eq!(in_edges[0].src, VertexId::from_int64(77));
}

#[test]
fn test_auto_vertex_compaction_reclaims_id_holes() {
    // Background maintenance must reclaim deleted-vertex ID holes without
    // an explicit compact transaction when thresholds are exceeded.
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let mut property_config = PropertyGraphConfig::test();
    property_config.auto_compact = AutoCompactConfig {
        enable_vertex_compaction: true,
        min_holes: 20,
        min_hole_ratio: 0.1,
        min_interval_secs: 0,
    };
    let persistence_config = PersistenceConfig::for_work_dir(temp_dir.path())
        .with_property_graph_config(property_config);
    let mut storage =
        GraphStorage::new_with_persistence(temp_dir.path().to_path_buf(), persistence_config)
            .expect("Failed to create persistent storage");
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    for i in 1..=100i64 {
        insert_test_vertex(&mut storage, i, &format!("v{i}"));
    }
    let expected_edges: Vec<(i64, i64)> = (1..=97).step_by(2).map(|src| (src, src + 2)).collect();
    for (src, dst) in &expected_edges {
        let edge = Edge::new(
            VertexId::from_int64(*src),
            VertexId::from_int64(*dst),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        );
        storage.insert_edge("test_space", edge).unwrap();
    }
    // Delete 40 vertices; holes appear but nothing is reclaimed yet.
    for i in (2..=80).step_by(2) {
        storage
            .delete_vertex("test_space", &VertexId::from_int64(i))
            .unwrap();
    }

    let (live, allocated) = storage
        .ctx
        .data_store()
        .with_vertex_tables(|tables| {
            Ok::<(usize, usize), graphdb_core::StorageError>(
                tables
                    .values()
                    .map(|t| t.id_hole_stats(u64::MAX))
                    .fold((0, 0), |(l, a), (x, y)| (l + x, a + y)),
            )
        })
        .unwrap();
    assert_eq!(
        (live, allocated),
        (60, 100),
        "holes must not be reclaimed yet"
    );

    storage.trigger_background_maintenance().unwrap();

    let (live, allocated) = storage
        .ctx
        .data_store()
        .with_vertex_tables(|tables| {
            Ok::<(usize, usize), graphdb_core::StorageError>(
                tables
                    .values()
                    .map(|t| t.id_hole_stats(u64::MAX))
                    .fold((0, 0), |(l, a), (x, y)| (l + x, a + y)),
            )
        })
        .unwrap();
    assert_eq!(
        (live, allocated),
        (60, 60),
        "auto compaction should re-densify ID space"
    );

    // Surviving edges still resolve through the remapped edge CSR.
    for (src, dst) in &expected_edges {
        let retrieved = storage
            .get_edge(
                "test_space",
                &VertexId::from_int64(*src),
                &VertexId::from_int64(*dst),
                "KNOWS",
                0,
            )
            .unwrap();
        assert!(
            retrieved.is_some(),
            "edge {src}->{dst} lost after auto vertex compaction remap"
        );
    }
}
