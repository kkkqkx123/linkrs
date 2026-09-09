use super::*;

#[test]
fn bound_operation_contexts_are_isolated_across_concurrent_handles() {
    use graphdb_core::types::TransactionId;
    use std::sync::{Arc, Barrier};

    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let mut writer = storage.bind_operation_context(StorageOperationContext::transaction(
        TransactionId::from(1),
        10,
        false,
    ));
    writer
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::from_int64(1),
                vec![Tag::new(
                    "Person".to_string(),
                    [("name".to_string(), Value::string("Alice"))]
                        .into_iter()
                        .collect(),
                )],
            ),
        )
        .expect("Failed to insert vertex at timestamp 10");

    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|id| {
            let barrier = barrier.clone();
            let bound = storage.bind_operation_context(StorageOperationContext::transaction(
                TransactionId::from(id + 100),
                10,
                true,
            ));
            std::thread::spawn(move || {
                barrier.wait();
                let context = bound
                    .operation_context()
                    .expect("Bound context should remain available");
                assert_eq!(context.transaction_id, Some(TransactionId::from(id + 100)));
                assert_eq!(context.read_timestamp, 10);
                assert!(bound
                    .get_vertex("test_space", &VertexId::from_int64(1))
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

    let mut initial_writer = storage.bind_operation_context(StorageOperationContext::transaction(
        TransactionId::from(1),
        10,
        false,
    ));
    initial_writer
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::from_int64(1),
                vec![Tag::new("Person".to_string(), Default::default())],
            ),
        )
        .expect("Failed to insert initial vertex");

    let reader = storage.bind_operation_context(StorageOperationContext::transaction(
        TransactionId::from(2),
        10,
        true,
    ));
    let mut cursor = reader
        .create_vertex_cursor("test_space", &ScanOptions::default())
        .expect("Failed to create cursor");

    let mut later_writer = storage.bind_operation_context(StorageOperationContext::transaction(
        TransactionId::from(3),
        20,
        false,
    ));
    later_writer
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::from_int64(2),
                vec![Tag::new("Person".to_string(), Default::default())],
            ),
        )
        .expect("Failed to insert later vertex");

    let rows = cursor.next_batch(16).expect("Cursor read failed");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].vid, VertexId::from_int64(1));
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
                VertexId::from_int64(1),
                vec![Tag::new(
                    "Person".to_string(),
                    [
                        ("name".to_string(), Value::string("Alice")),
                        ("age".to_string(), Value::BigInt(30)),
                    ]
                    .into_iter()
                    .collect(),
                )],
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
    assert!(rows[0].properties.contains_key("name"));
    assert!(!rows[0].properties.contains_key("age"));
}

#[test]
fn test_read_operation_context_pins_and_releases_statement_snapshot() {
    use crate::StorageOperationContextOps;

    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    insert_test_vertex(&mut storage, 1, "Alice");

    let label = storage
        .ctx
        .data_store()
        .with_vertex_tables(|tables| tables.keys().copied().collect::<Vec<_>>())[0];

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

    // No snapshot is registered before the first table access (lazy).
    let before = storage
        .ctx
        .data_store()
        .with_vertex_tables(|tables| {
            Ok::<Timestamp, graphdb_core::StorageError>(
                tables
                    .get(&label)
                    .map(|t| t.min_active_snapshot_ts())
                    .unwrap_or(Timestamp::MAX),
            )
        })
        .unwrap();

    // First read lazily registers the table snapshot at the read ts.
    let vertex = bound
        .get_vertex("test_space", &VertexId::from_int64(1))
        .unwrap()
        .expect("vertex should resolve");
    assert_eq!(
        vertex.properties.get("name").unwrap(),
        &Value::string("Alice")
    );
    let pinned = storage
        .ctx
        .data_store()
        .with_vertex_tables(|tables| {
            Ok::<Timestamp, graphdb_core::StorageError>(
                tables
                    .get(&label)
                    .map(|t| t.min_active_snapshot_ts())
                    .unwrap_or(Timestamp::MAX),
            )
        })
        .unwrap();
    assert_eq!(
        pinned, read_ts,
        "lazy read registration must pin min_active_snapshot_ts to the read ts"
    );
    assert_ne!(before, pinned, "registration must change the pinned min");

    // Finalize unregisters the statement snapshot.
    bound.finalize_operation(true).unwrap();
    let after = storage
        .ctx
        .data_store()
        .with_vertex_tables(|tables| {
            Ok::<Timestamp, graphdb_core::StorageError>(
                tables
                    .get(&label)
                    .map(|t| t.min_active_snapshot_ts())
                    .unwrap_or(Timestamp::MAX),
            )
        })
        .unwrap();
    assert_eq!(
        after, before,
        "finalize must unregister the read statement snapshot"
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
    let mut writer = storage.bind_operation_context(StorageOperationContext::transaction(
        graphdb_core::types::TransactionId::from(1),
        10,
        false,
    ));
    for i in 1..=200i64 {
        writer
            .insert_vertex(
                "test_space",
                Vertex::new(
                    VertexId::from_int64(i),
                    vec![Tag::new(
                        "Person".to_string(),
                        [
                            ("name".to_string(), Value::string(format!("P{i}"))),
                            ("age".to_string(), Value::BigInt(i)),
                        ]
                        .into_iter()
                        .collect(),
                    )],
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
fn edge_column_stats_snapshot_returns_none_for_unpopulated_columnar_store() {
    use crate::stats_reader::ColumnStatsReader;
    use graphdb_core::vertex_edge_path::Edge;

    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    // Insert two vertices and two edges.  The edge columnar store is
    // not populated until flush/compaction, so the snapshot should
    // gracefully return None rather than panicking.
    let mut writer = storage.bind_operation_context(StorageOperationContext::transaction(
        graphdb_core::types::TransactionId::from(1),
        10,
        false,
    ));
    writer
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::from_int64(1),
                vec![Tag::new(
                    "Person".to_string(),
                    [("name".to_string(), Value::string("Alice"))]
                        .into_iter()
                        .collect(),
                )],
            ),
        )
        .unwrap();
    writer
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::from_int64(2),
                vec![Tag::new(
                    "Person".to_string(),
                    [("name".to_string(), Value::string("Bob"))]
                        .into_iter()
                        .collect(),
                )],
            ),
        )
        .unwrap();
    writer
        .insert_edge(
            "test_space",
            Edge::new(
                VertexId::from_int64(1),
                VertexId::from_int64(2),
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
                VertexId::from_int64(2),
                VertexId::from_int64(1),
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

    // The edge columnar store is not populated until flush, so the
    // snapshot gracefully returns None (conservative fallback).
    let snap = storage.edge_column_stats("test_space", "KNOWS", "since");
    assert!(
        snap.is_none() || !snap.as_ref().unwrap().has_envelope(),
        "edge snapshot should be None or empty when columnar store is unpopulated"
    );
}
