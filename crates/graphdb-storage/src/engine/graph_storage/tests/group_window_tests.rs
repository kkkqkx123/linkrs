use super::*;

#[test]
fn test_group_window_single_ts() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let window = storage.begin_auto_commit_group().unwrap();
    let mut timestamps = Vec::new();
    for i in 0..5 {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let ts = bound
            .operation_context()
            .and_then(|ctx| ctx.write_timestamp)
            .unwrap();
        timestamps.push(ts);
        let vertex = Vertex::new(
            VertexId::try_from_int64(6000 + i).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("gperson_{i}"))),
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

    assert_eq!(window.statement_count(), 5);
    assert_eq!(window.snapshot_rounds(), 1);
    // All statements must share the same write timestamp.
    let first = timestamps[0];
    for ts in &timestamps[1..] {
        assert_eq!(*ts, first, "group mode must reuse the same write timestamp");
    }

    window.finalize_group().unwrap();
    // After finalize, data is visible.
    for i in 0..5 {
        let v = storage
            .get_vertex(
                "test_space",
                "Person",
                &VertexId::try_from_int64(6000 + i).expect("test vertex id"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(v.property_value("age"), Some(Value::BigInt(i)));
    }
}

#[test]
fn test_group_commit_visibility() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let window = storage.begin_auto_commit_group().unwrap();
    // First statement: insert vertex.
    {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(7001).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string("visible")),
                    ("age".to_string(), Value::BigInt(1)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        bound.insert_vertex("test_space", vertex).unwrap();
        bound.finalize_operation(true).unwrap();
    }
    // Second statement: read own writes (same group ts).
    {
        let bound = storage.bind_auto_commit_statement(&window).unwrap();
        let v = bound
            .get_vertex(
                "test_space",
                "Person",
                &VertexId::try_from_int64(7001).expect("test vertex id"),
            )
            .unwrap();
        assert!(
            v.is_some(),
            "group-internal read must see prior statement's write"
        );
    }
    window.finalize_group().unwrap();

    // After finalize, external reader sees data.
    let v = storage
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(7001).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(v.property_value("name"), Some(Value::string("visible")));
}

#[test]
fn test_group_failed_statement_rolls_back_own_writes() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let window = storage.begin_auto_commit_group().unwrap();
    // Statement 1: insert vertex 8001.
    {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(8001).expect("test vertex id"),
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
    // Statement 2: update vertex 8001, then rollback.
    {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let updated = Vertex::new(
            VertexId::try_from_int64(8001).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string("changed")),
                    ("age".to_string(), Value::BigInt(2)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        bound.update_vertex("test_space", updated).unwrap();
        bound.finalize_operation(false).unwrap();
    }
    // Statement 3: insert vertex 8002.
    {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(8002).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string("also_keep")),
                    ("age".to_string(), Value::BigInt(3)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        bound.insert_vertex("test_space", vertex).unwrap();
        bound.finalize_operation(true).unwrap();
    }

    window.finalize_group().unwrap();

    // Vertex 8001 should have original values (statement 2 rolled back).
    let v = storage
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(8001).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(v.property_value("name"), Some(Value::string("keep")));
    assert_eq!(v.property_value("age"), Some(Value::BigInt(1)));
    // Vertex 8002 exists (statement 3 committed).
    let v2 = storage
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(8002).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(v2.property_value("name"), Some(Value::string("also_keep")));
}

#[test]
fn test_group_window_without_wal_manager() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    // In-memory storage has no WAL manager; finalize_group sync is a no-op.
    let window = storage.begin_auto_commit_group().unwrap();
    {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(9001).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string("mem")),
                    ("age".to_string(), Value::BigInt(1)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        bound.insert_vertex("test_space", vertex).unwrap();
        bound.finalize_operation(true).unwrap();
    }
    window.finalize_group().unwrap();

    let v = storage
        .get_vertex(
            "test_space",
            "Person",
            &VertexId::try_from_int64(9001).expect("test vertex id"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(v.property_value("name"), Some(Value::string("mem")));
}

#[test]
fn test_group_empty_window_finalize() {
    let storage = create_test_storage();
    let window = storage.begin_auto_commit_group().unwrap();
    assert_eq!(window.statement_count(), 0);
    // Finalize without binding any statements must not panic.
    window.finalize_group().unwrap();
}

#[test]
fn test_group_window_staged_wal_bounded() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let window = storage.begin_auto_commit_group().unwrap();
    for i in 0..10 {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(10000 + i).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("gbounded_{i}"))),
                    ("age".to_string(), Value::BigInt(i)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        bound.insert_vertex("test_space", vertex).unwrap();
        bound.finalize_operation(true).unwrap();
        drop(bound);
        // Staged WAL must stay bounded (entries are removed by no-wait append).
        let wal_len = storage.staged_wal_len();
        assert!(
            wal_len <= 1,
            "staged WAL must stay <=1 during group, got {wal_len}"
        );
    }
    window.finalize_group().unwrap();
}

#[test]
fn test_group_window_unregisters_lazily_registered_snapshots() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let window = storage.begin_auto_commit_group().unwrap();
    let before = storage
        .ctx
        .version_manager()
        .snapshot_tracker()
        .active_count();
    for i in 0..20 {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(2000 + i).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("gleak_{i}"))),
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
    window.finalize_group().unwrap();

    assert_eq!(
        storage
            .ctx
            .version_manager()
            .snapshot_tracker()
            .active_count(),
        before,
        "group window leaked snapshots into the global tracker"
    );
}

#[test]
fn test_group_finalize_publishes_write_set() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let window = storage.begin_auto_commit_group().unwrap();
    {
        let mut bound = storage.bind_auto_commit_statement(&window).unwrap();
        let vertex = Vertex::new(
            VertexId::try_from_int64(11001).expect("test vertex id"),
            Tag::new(
                "Person".to_string(),
                vec![("name".to_string(), Value::string("published"))]
                    .into_iter()
                    .collect(),
            ),
        );
        bound.insert_vertex("test_space", vertex).unwrap();
        bound.finalize_operation(true).unwrap();
    }
    window.finalize_group().unwrap();

    let mut probe = graphdb_transaction::types::WriteSet::new();
    probe.record_vertex(VertexId::try_from_int64(11001).expect("test vertex id"));
    assert!(
        storage.ctx.committed_write_conflict_probe(&probe, 0),
        "group commit must publish its write set for later certification"
    );
}
