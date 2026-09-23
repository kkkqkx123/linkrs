mod revalidation_tests {
    use super::super::super::GraphStorageContext;
    use crate::engine::cache_manager::VertexSeed;
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::{DataType, LabelId, Timestamp};
    use graphdb_core::Value;

    fn setup_ctx() -> (GraphStorageContext, LabelId) {
        let ctx = GraphStorageContext::new();
        // `name` is the primary key and mirrors the external id; `value`
        // carries the mutable payload these tests read and update.
        let label = ctx
            .create_vertex_type(
                "Person",
                vec![
                    StoragePropertyDef::new("name".to_string(), DataType::String),
                    StoragePropertyDef::new("value".to_string(), DataType::String),
                ],
                "name",
            )
            .expect("create vertex type");
        (ctx, label)
    }

    fn insert_person(
        ctx: &GraphStorageContext,
        label: LabelId,
        name: &str,
        value: &str,
        ts: Timestamp,
    ) {
        ctx.insert_vertex(
            label,
            name,
            &[
                ("name".to_string(), Value::string(name)),
                ("value".to_string(), Value::string(value)),
            ],
            ts,
        )
        .expect("insert vertex");
    }

    /// Acquire a real write timestamp from the version manager. Tests must
    /// never use synthetic timestamps: unwatermarked stamps bypass the
    /// read-frontier invariant the cache guards rely on.
    fn write_ts(ctx: &GraphStorageContext) -> Timestamp {
        ctx.persistent
            .version_manager
            .acquire_insert_timestamp()
            .expect("acquire write ts")
    }

    fn commit_ts(ctx: &GraphStorageContext, ts: Timestamp) {
        ctx.persistent
            .version_manager
            .commit_ordered(ts)
            .expect("ordered commit");
    }

    fn live_frontier(ctx: &GraphStorageContext) -> Timestamp {
        ctx.persistent.version_manager.read_timestamp()
    }

    fn read_name(ctx: &GraphStorageContext, label: LabelId, name: &str, ts: Timestamp) -> Value {
        ctx.get_vertex(label, name, ts)
            .expect("vertex must be visible")
            .properties
            .iter()
            .find(|(k, _)| k == "value")
            .map(|(_, v)| v.clone())
            .expect("value property must be present")
    }

    fn internal_id_of(ctx: &GraphStorageContext, label: LabelId, name: &str, ts: Timestamp) -> u32 {
        ctx.persistent
            .data_store
            .with_vertex_tables(|tables| {
                tables.get(&label).and_then(|t| t.get_internal_id(name, ts))
            })
            .expect("internal id resolves")
    }

    #[test]
    fn stale_id_mapping_is_rejected_and_reseeded() {
        let (ctx, label) = setup_ctx();
        let ts = write_ts(&ctx);
        insert_person(&ctx, label, "alice", "A", ts);
        commit_ts(&ctx, ts);
        let frontier = live_frontier(&ctx);
        let real_id = internal_id_of(&ctx, label, "alice", frontier);
        assert!(ctx.get_vertex(label, "alice", frontier).is_some());

        // Poison the ID-index cache the way a racy seed after a GC remap
        // would: the mapping no longer matches the table.
        ctx.persistent
            .cache_manager
            .cache_vertex_id(label, "alice", real_id + 1000, frontier);
        let record = ctx
            .get_vertex(label, "alice", frontier)
            .expect("revalidation must fall back to the version-aware mapping");
        assert_eq!(record.internal_id, real_id);
        // The fresh mapping reseeds the cache.
        assert_eq!(
            ctx.persistent
                .cache_manager
                .get_cached_vertex_id(label, "alice", frontier),
            Some(real_id)
        );
    }

    #[test]
    fn stale_record_fence_returns_fresh_value() {
        let (ctx, label) = setup_ctx();
        let first = write_ts(&ctx);
        insert_person(&ctx, label, "alice", "A", first);
        commit_ts(&ctx, first);
        let frontier = live_frontier(&ctx);
        assert_eq!(
            read_name(&ctx, label, "alice", frontier),
            Value::string("A")
        );
        let internal_id = internal_id_of(&ctx, label, "alice", frontier);
        let seeded = ctx
            .persistent
            .cache_manager
            .get_cached_vertex(label, internal_id, frontier)
            .expect("seeded entry");
        assert_eq!(seeded.create_ts, first);

        // Concurrent commit lands after the seed (write path invalidates).
        let second = write_ts(&ctx);
        ctx.update_vertex_property(label, "alice", "value", &Value::string("B"), second)
            .expect("update");
        commit_ts(&ctx, second);

        // A racy seed publishes the pre-commit value after the invalidation.
        ctx.persistent.cache_manager.cache_vertex(
            label,
            internal_id,
            VertexSeed {
                external_id: "alice",
                properties: &seeded.properties,
                read_ts: first,
                create_ts: seeded.create_ts,
                column_starts: &seeded.column_starts,
            },
        );

        // Hit revalidation must observe the drifted column fence and serve B.
        assert_eq!(
            read_name(&ctx, label, "alice", live_frontier(&ctx)),
            Value::string("B")
        );
    }

    #[test]
    fn historical_read_does_not_poison_future_readers() {
        let (ctx, label) = setup_ctx();
        let first = write_ts(&ctx);
        insert_person(&ctx, label, "alice", "A", first);
        commit_ts(&ctx, first);
        assert_eq!(read_name(&ctx, label, "alice", first), Value::string("A"));
        let second = write_ts(&ctx);
        ctx.update_vertex_property(label, "alice", "value", &Value::string("B"), second)
            .expect("update");
        commit_ts(&ctx, second);

        // The historical snapshot still reads A, but its version is no
        // longer current so the seed must be skipped.
        assert_eq!(read_name(&ctx, label, "alice", first), Value::string("A"));
        let internal_id = internal_id_of(&ctx, label, "alice", live_frontier(&ctx));
        assert!(
            ctx.persistent
                .cache_manager
                .get_cached_vertex(label, internal_id, live_frontier(&ctx))
                .is_none(),
            "stale version must not be seeded"
        );
        assert_eq!(
            read_name(&ctx, label, "alice", live_frontier(&ctx)),
            Value::string("B")
        );
    }

    #[test]
    fn concurrent_read_write_mix_stays_fresh() {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        use std::sync::Arc;

        let (ctx, label) = setup_ctx();
        let base = write_ts(&ctx);
        insert_person(&ctx, label, "key", "v0", base);
        commit_ts(&ctx, base);
        let done = Arc::new(AtomicBool::new(false));

        let writer_ctx = ctx.clone();
        let writer_done = done.clone();
        let writer = std::thread::spawn(move || {
            for i in 1..=50u64 {
                let ts = writer_ctx
                    .persistent
                    .version_manager
                    .acquire_insert_timestamp()
                    .expect("acquire write ts");
                writer_ctx
                    .update_vertex_property(
                        label,
                        "key",
                        "value",
                        &Value::string(format!("v{i}")),
                        ts,
                    )
                    .expect("writer update");
                writer_ctx
                    .persistent
                    .version_manager
                    .commit_ordered(ts)
                    .expect("ordered commit");
            }
            writer_done.store(true, AtomicOrdering::Release);
        });

        let mut readers = Vec::new();
        for _ in 0..3 {
            let reader_ctx = ctx.clone();
            let reader_done = done.clone();
            readers.push(std::thread::spawn(move || {
                let mut iterations = 0;
                while !reader_done.load(AtomicOrdering::Acquire) || iterations < 20 {
                    let frontier = reader_ctx.persistent.version_manager.read_timestamp();
                    let record = reader_ctx
                        .get_vertex(label, "key", frontier)
                        .expect("concurrent read must succeed");
                    let name = record
                        .properties
                        .iter()
                        .find(|(k, _)| k == "value")
                        .map(|(_, v)| v.clone())
                        .expect("value property must be present");
                    let stale = (0..=50u64).all(|i| name != Value::string(format!("v{i}")));
                    assert!(!stale, "no torn values under concurrency");
                    iterations += 1;
                    if iterations > 500 {
                        break;
                    }
                }
            }));
        }

        writer.join().expect("writer");
        for reader in readers {
            reader.join().expect("reader");
        }
        let frontier = ctx.persistent.version_manager.read_timestamp();
        assert_eq!(
            read_name(&ctx, label, "key", frontier),
            Value::string("v50")
        );
    }
}

mod pending_visibility_tests {
    use super::super::super::GraphStorageContext;
    use crate::engine::{EdgeOperationParams, InsertEdgeParams};
    use crate::types::StoragePropertyDef;
    use crate::StorageOperationContext;
    use graphdb_core::types::{DataType, LabelId, Timestamp, TransactionId, VertexId};
    use graphdb_core::Value;

    fn setup_ctx() -> (GraphStorageContext, LabelId) {
        let ctx = GraphStorageContext::new();
        // `name` is the primary key and mirrors the external id; `value`
        // carries the payload these tests write and read.
        let label = ctx
            .create_vertex_type(
                "Person",
                vec![
                    StoragePropertyDef::new("name".to_string(), DataType::String),
                    StoragePropertyDef::new("value".to_string(), DataType::String),
                ],
                "name",
            )
            .expect("create vertex type");
        (ctx, label)
    }

    fn bound_writer(
        ctx: &GraphStorageContext,
        txn: u64,
        read_ts: Timestamp,
        write_ts: Timestamp,
    ) -> GraphStorageContext {
        ctx.with_operation_context(StorageOperationContext::transaction_with_timestamps(
            TransactionId::from(txn),
            read_ts,
            Some(write_ts),
            false,
            false,
        ))
    }

    #[test]
    fn own_write_visible_point_and_projection() {
        let (ctx, label) = setup_ctx();
        let vm = ctx.persistent.version_manager.clone();
        let start = vm.acquire_insert_timestamp().expect("start txn");

        ctx.insert_vertex(
            label,
            "alice",
            &[
                ("name".to_string(), Value::string("alice")),
                ("value".to_string(), Value::string("A")),
            ],
            start,
        )
        .expect("insert own write");

        let bound = bound_writer(&ctx, 1, start, start);
        let record = bound
            .get_vertex(label, "alice", start)
            .expect("own write visible to point lookup");
        assert_eq!(
            record
                .properties
                .iter()
                .find(|(k, _)| k == "value")
                .map(|(_, v)| v),
            Some(&Value::string("A"))
        );
        let projected = bound
            .get_vertex_projected(label, "alice", &["value".to_string()], start)
            .expect("own write visible to projection");
        assert_eq!(
            projected
                .properties
                .iter()
                .find(|(k, _)| k == "value")
                .map(|(_, v)| v),
            Some(&Value::string("A"))
        );
        vm.commit_ordered(start).expect("ordered commit");
    }

    #[test]
    fn concurrent_write_transactions_hide_each_other_pending_writes() {
        let (ctx, label) = setup_ctx();
        let vm = ctx.persistent.version_manager.clone();
        let first = vm.acquire_insert_timestamp().expect("first txn");
        ctx.insert_vertex(
            label,
            "alice",
            &[
                ("name".to_string(), Value::string("alice")),
                ("value".to_string(), Value::string("A")),
            ],
            first,
        )
        .expect("first txn writes");

        let second = vm.acquire_insert_timestamp().expect("second txn");
        let bound_second = bound_writer(&ctx, 2, second, second);
        assert!(
            bound_second.get_vertex(label, "alice", second).is_none(),
            "concurrent writer must not observe the foreign uncommitted row"
        );

        // After the first transaction commits, the same snapshot observes it:
        // the committed stamp is at or below the advanced frontier.
        vm.commit_ordered(first).expect("ordered commit");
        assert!(
            bound_second.get_vertex(label, "alice", second).is_some(),
            "committed row becomes visible to the peer snapshot"
        );

        // A fresh snapshot after both settle observes the row as well.
        vm.abort_write_timestamp(second);
        let frontier = vm.read_timestamp();
        let bound_read =
            ctx.with_operation_context(StorageOperationContext::transaction_with_timestamps(
                TransactionId::from(3),
                frontier,
                None,
                true,
                false,
            ));
        assert!(
            bound_read.get_vertex(label, "alice", frontier).is_some(),
            "row stays visible across frontier advance"
        );
    }

    #[test]
    fn own_edge_write_visible_point_and_traversal() {
        use crate::edge::EdgeStrategy;

        let ctx = GraphStorageContext::new();
        let src_label = ctx
            .create_vertex_type(
                "Person",
                vec![
                    StoragePropertyDef::new("name".to_string(), DataType::String),
                    StoragePropertyDef::new("value".to_string(), DataType::String),
                ],
                "name",
            )
            .expect("src label");
        let dst_label = ctx
            .create_vertex_type(
                "City",
                vec![
                    StoragePropertyDef::new("name".to_string(), DataType::String),
                    StoragePropertyDef::new("value".to_string(), DataType::String),
                ],
                "name",
            )
            .expect("dst label");
        let edge_label = ctx
            .create_edge_type(
                "LIVES_IN",
                src_label,
                dst_label,
                vec![],
                EdgeStrategy::Multiple,
                EdgeStrategy::Multiple,
            )
            .expect("edge type");

        let vm = ctx.persistent.version_manager.clone();
        let start = vm.acquire_insert_timestamp().expect("start txn");
        ctx.insert_vertex_by_i64(
            src_label,
            1,
            &[
                ("name".to_string(), Value::string("1")),
                ("value".to_string(), Value::string("A")),
            ],
            start,
        )
        .expect("src vertex");
        ctx.insert_vertex_by_i64(
            dst_label,
            2,
            &[
                ("name".to_string(), Value::string("2")),
                ("value".to_string(), Value::string("B")),
            ],
            start,
        )
        .expect("dst vertex");
        ctx.insert_edge(InsertEdgeParams {
            edge_label,
            src_label,
            src_id: VertexId::try_from_int64(1).expect("test vertex id"),
            dst_label,
            dst_id: VertexId::try_from_int64(2).expect("test vertex id"),
            rank: 0,
            properties: &[],
            ts: start,
        })
        .expect("own edge write");

        let bound = bound_writer(&ctx, 1, start, start);
        let params = EdgeOperationParams {
            edge_label,
            src_label,
            src_id: VertexId::try_from_int64(1).expect("test vertex id"),
            dst_label,
            dst_id: VertexId::try_from_int64(2).expect("test vertex id"),
            rank: 0,
        };
        assert!(
            bound.get_edge(&params, start).is_some(),
            "own edge write visible to point lookup"
        );
        let neighbors = bound
            .out_edges_projected(
                edge_label,
                src_label,
                VertexId::try_from_int64(1).expect("test vertex id"),
                start,
                None,
            )
            .expect("traversal resolves");
        assert_eq!(neighbors.len(), 1);

        // A concurrent writer still pending must not observe the edge.
        let peer = vm.acquire_insert_timestamp().expect("peer txn");
        let bound_peer = bound_writer(&ctx, 2, peer, peer);
        assert!(
            bound_peer.get_edge(&params, peer).is_none(),
            "concurrent writer must not observe the foreign uncommitted edge"
        );
        vm.commit_ordered(start).expect("ordered commit");
        vm.abort_write_timestamp(peer);
    }

    #[test]
    fn scan_and_projection_hide_foreign_pending_row() {
        let (ctx, label) = setup_ctx();
        let vm = ctx.persistent.version_manager.clone();
        let first = vm.acquire_insert_timestamp().expect("first txn");
        ctx.insert_vertex(
            label,
            "alice",
            &[
                ("name".to_string(), Value::string("alice")),
                ("value".to_string(), Value::string("A")),
            ],
            first,
        )
        .expect("first txn writes");

        let second = vm.acquire_insert_timestamp().expect("second txn");
        let bound_second = bound_writer(&ctx, 2, second, second);
        assert!(
            bound_second
                .get_vertex_projected(label, "alice", &["value".to_string()], second)
                .is_none(),
            "projection must not observe the foreign uncommitted row"
        );
        let scanned = bound_second
            .scan_vertices(label, second)
            .expect("scan resolves");
        assert!(
            scanned.is_empty(),
            "scan must not observe the foreign uncommitted row"
        );

        vm.commit_ordered(first).expect("ordered commit");
        assert!(
            bound_second
                .get_vertex_projected(label, "alice", &["value".to_string()], second)
                .is_some(),
            "projection observes the row after commit"
        );
        let scanned = bound_second
            .scan_vertices(label, second)
            .expect("scan resolves");
        assert_eq!(scanned.len(), 1);
        vm.abort_write_timestamp(second);
    }
}
