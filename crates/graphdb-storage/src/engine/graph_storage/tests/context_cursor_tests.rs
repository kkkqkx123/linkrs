use super::*;

#[test]
fn bound_operation_contexts_are_isolated_across_concurrent_handles() {
    use graphdb_core::types::TransactionId;
    use std::sync::{Arc, Barrier};

    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let mut writer =
        storage.bind_operation_context(StorageOperationContext::transaction_with_timestamps(
            TransactionId::from(1),
            10,
            Some(10),
            false,
            false,
        ));
    writer
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::try_from_int64(1).expect("test vertex id"),
                Tag::new(
                    "Person".to_string(),
                    [("name".to_string(), Value::string("Alice"))]
                        .into_iter()
                        .collect(),
                ),
            ),
        )
        .expect("Failed to insert vertex at timestamp 10");
    // The bound context is a real transaction under the staging contract:
    // the write lands only when the transaction commits.
    drop(writer);
    storage
        .commit_staged_writes(TransactionId::from(1), &[])
        .expect("Failed to commit staged insert");

    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|id| {
            let barrier = barrier.clone();
            let bound = storage.bind_operation_context(
                StorageOperationContext::transaction_with_timestamps(
                    TransactionId::from(id + 100),
                    10,
                    None,
                    true,
                    false,
                ),
            );
            std::thread::spawn(move || {
                barrier.wait();
                let context = bound
                    .operation_context()
                    .expect("Bound context should remain available");
                assert_eq!(context.transaction_id, Some(TransactionId::from(id + 100)));
                assert_eq!(context.read_timestamp, 10);
                assert!(bound
                    .get_vertex(
                        "test_space",
                        "Person",
                        &VertexId::try_from_int64(1).expect("test vertex id")
                    )
                    .expect("Concurrent read failed")
                    .is_some());
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Concurrent context task panicked");
    }
    assert!(storage.operation_context().is_none());
}

#[test]
fn cursor_keeps_the_read_timestamp_from_its_bound_handle() {
    use graphdb_core::types::TransactionId;

    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let mut initial_writer =
        storage.bind_operation_context(StorageOperationContext::transaction_with_timestamps(
            TransactionId::from(1),
            10,
            Some(10),
            false,
            false,
        ));
    initial_writer
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::try_from_int64(1).expect("test vertex id"),
                Tag::new("Person".to_string(), Default::default()),
            ),
        )
        .expect("Failed to insert initial vertex");
    // Commit the initial transaction so the row exists in the main table
    // before the pinned reader takes its snapshot.
    drop(initial_writer);
    storage
        .commit_staged_writes(TransactionId::from(1), &[])
        .expect("Failed to commit staged insert");

    let reader =
        storage.bind_operation_context(StorageOperationContext::transaction_with_timestamps(
            TransactionId::from(2),
            10,
            None,
            true,
            false,
        ));
    let mut cursor = reader
        .create_vertex_cursor("test_space", &ScanOptions::default())
        .expect("Failed to create cursor");

    let mut later_writer =
        storage.bind_operation_context(StorageOperationContext::transaction_with_timestamps(
            TransactionId::from(3),
            20,
            Some(20),
            false,
            false,
        ));
    later_writer
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::try_from_int64(2).expect("test vertex id"),
                Tag::new("Person".to_string(), Default::default()),
            ),
        )
        .expect("Failed to insert later vertex");

    let rows = cursor.next_batch(16).expect("Cursor read failed");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].vid,
        VertexId::try_from_int64(1).expect("test vertex id")
    );
}

#[test]
fn cursor_applies_property_projection_during_scan() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    storage
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::try_from_int64(1).expect("test vertex id"),
                Tag::new(
                    "Person".to_string(),
                    [
                        ("name".to_string(), Value::string("Alice")),
                        ("age".to_string(), Value::BigInt(30)),
                    ]
                    .into_iter()
                    .collect(),
                ),
            ),
        )
        .expect("vertex insert");

    let mut cursor = storage
        .create_vertex_cursor(
            "test_space",
            &ScanOptions::default().with_projection_named(vec!["name".to_string()]),
        )
        .expect("cursor should open");
    let rows = cursor.next_batch(8).expect("cursor batch");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].properties().contains_key("name"));
    assert!(!rows[0].properties().contains_key("age"));
}

#[test]
fn test_read_operation_context_pins_and_releases_statement_snapshot() {
    use crate::StorageOperationContextOps;

    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    insert_test_vertex(&mut storage, 1, "Alice");

    // Bind a read-only statement context with a fixed snapshot timestamp.
    let bound = storage.bind_read_operation_context().unwrap();
    let op_ctx = bound.operation_context().expect("read context");
    assert!(op_ctx.read_only, "read context must be read-only");
    assert!(
        op_ctx.write_timestamp.is_none(),
        "read context has no write ts"
    );
    let read_ts = op_ctx.read_timestamp;
    assert!(read_ts > 0, "read context must pin a snapshot timestamp");

    // Snapshot truth lives in the global tracker: the statement must leave
    // no per-table or global pin behind after finalize.
    let before = storage
        .ctx
        .version_manager()
        .snapshot_tracker()
        .active_count();

    // Reads resolve without any table-level registration.
    let vertex = bound
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(1).expect("test vertex id"),
        )
        .unwrap()
        .expect("vertex should resolve");
    assert_eq!(
        vertex.property_value("name").unwrap(),
        Value::string("Alice")
    );

    // Finalize leaves the global tracker exactly as it was.
    bound.finalize_operation(true).unwrap();
    assert_eq!(
        storage
            .ctx
            .version_manager()
            .snapshot_tracker()
            .active_count(),
        before,
        "read statement must not leak a snapshot into the global tracker"
    );
}

#[test]
fn vertex_column_stats_snapshot_matches_inserted_range() {
    use crate::stats_reader::{ColumnStatsReader, ColumnStatsSnapshot};
    use graphdb_core::vertex_edge_path::Vertex;
    use std::sync::Arc;

    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    // Insert 200 vertices with ages 1..=200.  The snapshot should
    // capture the true global min/max regardless of any sample window.
    let mut writer =
        storage.bind_operation_context(StorageOperationContext::transaction_with_timestamps(
            graphdb_core::types::TransactionId::from(1),
            10,
            Some(10),
            false,
            false,
        ));
    for i in 1..=200i64 {
        writer
            .insert_vertex(
                "test_space",
                Vertex::new(
                    VertexId::try_from_int64(i).expect("test vertex id"),
                    Tag::new(
                        "Person".to_string(),
                        [
                            ("name".to_string(), Value::string(format!("P{i}"))),
                            ("age".to_string(), Value::BigInt(i)),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
            )
            .expect("insert should succeed");
    }
    drop(writer);
    storage
        .commit_staged_writes(graphdb_core::types::TransactionId::from(1), &[])
        .expect("commit");

    // Read the snapshot via the trait.
    let snap: Arc<ColumnStatsSnapshot> = storage
        .vertex_column_stats("test_space", "Person", "age")
        .expect("snapshot should be available");

    assert_eq!(snap.row_count, 200, "row count should match inserts");
    assert_eq!(snap.min_value, Some(Value::BigInt(1)));
    assert_eq!(snap.max_value, Some(Value::BigInt(200)));

    // Also verify a string column.  Lexicographic max of "P1".."P200"
    // is "P99" (since '9' > '2'), which is the zone map's correct
    // bound under the native Value::cmp ordering.
    let name_snap = storage
        .vertex_column_stats("test_space", "Person", "name")
        .expect("name snapshot should be available");
    assert_eq!(name_snap.min_value, Some(Value::string("P1")));
    assert_eq!(name_snap.max_value, Some(Value::string("P99")));
}

#[test]
fn vertex_column_stats_snapshot_row_count_excludes_deleted_rows() {
    use crate::stats_reader::ColumnStatsReader;

    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    for i in 1..=10i64 {
        storage
            .insert_vertex(
                "test_space",
                Vertex::new(
                    VertexId::try_from_int64(i).expect("test vertex id"),
                    Tag::new(
                        "Person".to_string(),
                        [("age".to_string(), Value::BigInt(i))]
                            .into_iter()
                            .collect(),
                    ),
                ),
            )
            .expect("insert should succeed");
    }

    for i in 1..=4i64 {
        storage
            .delete_vertex(
                "test_space",
                "Person",
                &VertexId::try_from_int64(i).expect("test vertex id"),
            )
            .expect("delete should succeed");
    }

    let snap = storage
        .vertex_column_stats("test_space", "Person", "age")
        .expect("snapshot should be available");
    assert_eq!(
        snap.row_count, 6,
        "snapshot row count must be live rows, not allocated slots"
    );
}

#[test]
fn edge_column_stats_snapshot_matches_inserted_range() {
    use crate::stats_reader::ColumnStatsReader;
    use graphdb_core::vertex_edge_path::Edge;
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    // Insert two vertices and two edges, then verify the edge statistics
    // snapshot covers the inserted range.
    let mut writer =
        storage.bind_operation_context(StorageOperationContext::transaction_with_timestamps(
            graphdb_core::types::TransactionId::from(1),
            10,
            Some(10),
            false,
            false,
        ));
    writer
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::try_from_int64(1).expect("test vertex id"),
                Tag::new(
                    "Person".to_string(),
                    [("name".to_string(), Value::string("Alice"))]
                        .into_iter()
                        .collect(),
                ),
            ),
        )
        .unwrap();
    writer
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::try_from_int64(2).expect("test vertex id"),
                Tag::new(
                    "Person".to_string(),
                    [("name".to_string(), Value::string("Bob"))]
                        .into_iter()
                        .collect(),
                ),
            ),
        )
        .unwrap();
    writer
        .insert_edge(
            "test_space",
            Edge::new(
                VertexId::try_from_int64(1).expect("test vertex id"),
                VertexId::try_from_int64(2).expect("test vertex id"),
                "KNOWS".to_string(),
                0,
                [("since".to_string(), Value::Int(2020))]
                    .into_iter()
                    .collect(),
            ),
        )
        .unwrap();
    writer
        .insert_edge(
            "test_space",
            Edge::new(
                VertexId::try_from_int64(2).expect("test vertex id"),
                VertexId::try_from_int64(1).expect("test vertex id"),
                "KNOWS".to_string(),
                0,
                [("since".to_string(), Value::Int(2025))]
                    .into_iter()
                    .collect(),
            ),
        )
        .unwrap();
    drop(writer);
    storage
        .commit_staged_writes(graphdb_core::types::TransactionId::from(1), &[])
        .expect("commit");

    // Edge zone-map bounds follow writes, so the snapshot carries the
    // inserted envelope without waiting for a flush; persisted counts
    // (null/distinct) only appear after the flush-time stats refresh.
    let snap = storage.edge_column_stats("test_space", "KNOWS", "since");
    let snap = snap.expect("edge snapshot should cover the inserted range");
    assert_eq!(snap.row_count, 2);
    assert_eq!(snap.min_value, Some(Value::Int(2020)));
    assert_eq!(snap.max_value, Some(Value::Int(2025)));
    assert!(snap.has_envelope());
    // Unknown columns still fall back conservatively.
    assert!(storage
        .edge_column_stats("test_space", "KNOWS", "missing")
        .is_none());
}

#[test]
fn cursor_allowlist_requires_tag_filter() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    insert_test_vertex(&mut storage, 1, "Alice");

    let err = storage
        .create_vertex_cursor(
            "test_space",
            &ScanOptions::default().with_internal_id_allowlist(vec![0]),
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("tag"),
        "allowlist without tag must name the tag requirement: {err}"
    );
}

#[test]
fn cursor_allowlist_decodes_exact_id_set() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");
    insert_test_vertex(&mut storage, 3, "Carol");

    let mut probe = storage
        .create_vertex_cursor(
            "test_space",
            &ScanOptions::default().with_tag("Person".to_string()),
        )
        .expect("probe cursor should open");
    let flat = probe.next_flat_batch(16).expect("probe batch");
    assert_eq!(flat.len(), 3);
    // u32::MAX is out of range for every shard and must yield no row,
    // mirroring batch point-lookup semantics. Expected vids come from the
    // probed rows themselves: flat order follows shard layout, not
    // insertion order.
    let wanted = vec![
        flat[0].internal_id as u32,
        flat[2].internal_id as u32,
        u32::MAX,
    ];
    let mut expected: Vec<i64> = vec![flat[0].vid, flat[2].vid]
        .into_iter()
        .filter_map(|v| v.as_int64())
        .collect();
    expected.sort_unstable();

    let mut cursor = storage
        .create_vertex_cursor(
            "test_space",
            &ScanOptions::default()
                .with_tag("Person".to_string())
                .with_internal_id_allowlist(wanted.clone()),
        )
        .expect("allowlist cursor should open");
    let rows = cursor.next_batch(16).expect("allowlist batch");
    assert_eq!(rows.len(), 2);
    let mut vids: Vec<i64> = rows.iter().filter_map(|r| r.vid.as_int64()).collect();
    vids.sort_unstable();
    assert_eq!(vids, expected);

    // The column-block path honors the same allowlist.
    let mut column_cursor = storage
        .create_vertex_cursor(
            "test_space",
            &ScanOptions::default()
                .with_tag("Person".to_string())
                .with_internal_id_allowlist(wanted),
        )
        .expect("column allowlist cursor should open");
    let batch = column_cursor
        .next_column_batch(&["name".to_string()], 16)
        .expect("column batch");
    assert_eq!(batch.len(), 2);
    let mut batch_vids: Vec<i64> = batch.vids.iter().filter_map(|v| v.as_int64()).collect();
    batch_vids.sort_unstable();
    assert_eq!(batch_vids, expected);
}
