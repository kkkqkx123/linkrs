use super::*;

#[test]
fn test_auto_commit_batch_window_reuses_snapshots() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let window = storage.begin_auto_commit_batch().unwrap();
    for i in 0..50 {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(1000 + i).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("person_{i}"))),
                    ("age".to_string(), Value::BigInt(i)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        bound.insert_vertex("test_space", vertex).unwrap();
        bound.finalize_operation(true).unwrap();
        drop(bound);
    }

    // the whole batch registers MVCC snapshots exactly once.
    assert_eq!(window.statement_count(), 50);
    assert_eq!(window.snapshot_rounds(), 1);

    // Per-statement commits are visible to later statements and reads.
    storage.finalize_auto_commit_batch(&window).unwrap();
    for i in 0..50 {
        let v = storage
            .get_vertex(
                "test_space",
                "Person",
                &VertexId::try_from_int64(1000 + i).expect("test vertex id"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(v.property_value("age"), Some(Value::BigInt(i)));
    }

    // After the window is finalized the write gate is released: a new
    // single auto-commit statement can proceed.
    let mut next = storage.bind_auto_commit_context().unwrap();
    let vertex = Vertex::new(
        VertexId::try_from_int64(2000).expect("test vertex id"),
        Tag::new(
            "Person".to_string(),
            vec![("name".to_string(), Value::string("after"))]
                .into_iter()
                .collect(),
        ),
    );
    next.insert_vertex("test_space", vertex).unwrap();
    next.finalize_operation(true).unwrap();
    drop(next);
}

#[test]
fn test_batch_window_unregisters_lazily_registered_snapshots() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let window = storage.begin_auto_commit_batch().unwrap();
    for i in 0..20 {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(1000 + i).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("leak_{i}"))),
                    ("age".to_string(), Value::BigInt(i)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        bound.insert_vertex("test_space", vertex).unwrap();
        bound.finalize_operation(true).unwrap();
        drop(bound);
    }
    storage.finalize_auto_commit_batch(&window).unwrap();

    // Every snapshot registered lazily by the window's statements must be
    // released at window finalize; otherwise the table's GC watermark is
    // pinned forever (version data can never be reclaimed).
    let active = storage.ctx.data_store().with_vertex_tables(|tables| {
        tables
            .values()
            .map(|table| table.active_snapshot_count())
            .sum::<usize>()
    });
    assert_eq!(active, 0, "batch window leaked vertex snapshots");
}

#[test]
fn test_auto_commit_batch_window_failed_statement_rolls_back_itself() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let window = storage.begin_auto_commit_batch().unwrap();
    {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(3001).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string("keep")),
                    ("age".to_string(), Value::BigInt(1)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        bound.insert_vertex("test_space", vertex).unwrap();
        bound.finalize_operation(true).unwrap();
    }
    {
        // Failed statement: overwrite age in place, then abort. Only this
        // statement's partial write must roll back.
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let updated = Vertex::new(
            VertexId::try_from_int64(3001).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string("keep")),
                    ("age".to_string(), Value::BigInt(2)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        bound.update_vertex("test_space", updated).unwrap();
        bound.finalize_operation(false).unwrap();
    }
    storage.finalize_auto_commit_batch(&window).unwrap();

    let v = storage
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(3001).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(v.property_value("age"), Some(Value::BigInt(1)));
}

#[test]
fn test_auto_commit_batch_window_with_unique_index() {
    use crate::index::traits::VertexIndexOps;

    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    // Unique index on Person.name exercises the pending-aware unique
    // check inside the batch window: the duplicate must be rejected while
    // the earlier inserts' deltas are still unpublished.
    let index = Index::new(IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 1,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::string(""),
            false,
        )],
        properties: vec![],
        index_type: IndexType::TagIndex,
        is_unique: true,
        covering: false,
        partial_condition: None,
    });
    storage.create_tag_index("test_space", &index).unwrap();

    let window = storage.begin_auto_commit_batch().unwrap();
    for i in 0..50 {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(1000 + i).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("person_{i}"))),
                    ("age".to_string(), Value::BigInt(i)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        bound.insert_vertex("test_space", vertex).unwrap();
        bound.finalize_operation(true).unwrap();
        drop(bound);
    }

    let manager = storage.ctx.index_data_manager();

    // Before any read flushes the pending deltas, a duplicate name from a
    // later window statement must be rejected via the pending-aware unique
    // check (person_10 already committed at vid 1010).
    assert!(
        manager
            .read()
            .pending_delta_entries(crate::index::types::IndexIdentity {
                space_id: 1,
                index_id: 1,
            })
            > 0,
        "index deltas must still be pending before any read flush"
    );
    {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let duplicate = Vertex::new(
            VertexId::try_from_int64(2000).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::string("person_10"))]
                    .into_iter()
                    .collect(),
            ),
        );
        let result = bound.insert_vertex("test_space", duplicate);
        assert!(
            result.is_err(),
            "duplicate unique name must be rejected inside the window"
        );
        bound.finalize_operation(false).unwrap();
        drop(bound);
    }

    // The failed statement left no index residue and no vertex data.
    assert!(storage
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(2000).expect("test vertex id")
        )
        .unwrap()
        .is_none());
    storage.finalize_auto_commit_batch(&window).unwrap();

    // Index lookups observe all 50 committed entries after finalize.
    for i in 0..50 {
        let results = manager
            .read()
            .lookup_tag_index(1, &index, &Value::string(format!("person_{i}")))
            .unwrap();
        assert_eq!(results, vec![Value::BigInt(1000 + i)], "lookup person_{i}");
    }
}

#[test]
fn test_auto_commit_batch_window_via_sync_wrapper() {
    use crate::AutoCommitBatchOps;

    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    // L1: SyncWrapper<S> forwards the batch-window operations to the inner
    // engine, so the server-side QueryApi<SyncWrapper<GraphStorage>> can
    // share one window across statements.
    let wrapper = crate::SyncWrapper::new(storage);

    let window = wrapper.begin_auto_commit_batch().unwrap();
    for i in 0..10 {
        let mut bound = wrapper.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(5000 + i).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("person_{i}"))),
                    ("age".to_string(), Value::BigInt(i)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        bound.insert_vertex("test_space", vertex).unwrap();
        bound.finalize_operation(true).unwrap();
        drop(bound);
    }
    wrapper.finalize_auto_commit_batch(&window).unwrap();

    for i in 0..10 {
        let v = wrapper
            .get_vertex(
                "test_space",
                "Person",
                &VertexId::try_from_int64(5000 + i).expect("test vertex id"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(v.property_value("age"), Some(Value::BigInt(i)));
    }
}
