
use super::*;
use crate::executor::base::MemoryBudget;
use crate::executor::streaming::operators::base::OperatorBase;
use crate::executor::streaming::runtime::{ExecutionRuntime, QueryIdentity};
use linkrs_core::Value;

fn source(kind: SourceOperatorKind) -> SourceOperator {
    SourceOperator::new(kind, Arc::new(SlotLayout::new(Vec::new())))
}

fn config_for(base: &OperatorBase) -> OperatorConfig {
    OperatorConfig {
        chunk_size: base.chunk_size,
        partition_id: base.partition_id,
        physical_operator_id: base.physical_operator_id,
    }
}

#[test]
fn scan_source_terminates_after_consuming_its_buffer() {
    let mut source = source(SourceOperatorKind::ScanVertices {
        buffer: vec![vec![Value::BigInt(1)]],
        current_index: 0,
        col_names: Vec::new(),
    });
    let base = OperatorBase::new(0);

    source.inject_context(base.runtime.as_ref(), config_for(&base));
    source.open().expect("source should open");
    assert_eq!(
        source
            .next()
            .expect("first pull should succeed")
            .map(|chunk| chunk.len()),
        Some(1)
    );
    assert!(source.next().expect("second pull should succeed").is_none());
}

#[test]
fn scan_source_splits_across_multiple_chunks() {
    let base = OperatorBase::new(0);
    let chunk_size = base.chunk_size;
    let row_count = chunk_size * 2 + 7;
    let buffer: Vec<Vec<Value>> = (0..row_count)
        .map(|i| vec![Value::BigInt(i as i64)])
        .collect();
    let mut source = source(SourceOperatorKind::ScanVertices {
        buffer,
        current_index: 0,
        col_names: Vec::new(),
    });
    source.inject_context(base.runtime.as_ref(), config_for(&base));
    source.open().expect("source should open");

    let chunk1 = source
        .next()
        .expect("first pull should succeed")
        .expect("first chunk should be Some");
    assert_eq!(chunk1.len(), chunk_size);

    let chunk2 = source
        .next()
        .expect("second pull should succeed")
        .expect("second chunk should be Some");
    assert_eq!(chunk2.len(), chunk_size);

    let chunk3 = source
        .next()
        .expect("third pull should succeed")
        .expect("third chunk should be Some");
    assert_eq!(chunk3.len(), 7);

    assert!(source.next().expect("fourth pull should succeed").is_none());

    let total: i64 = chunk1
        .rows
        .iter()
        .chain(chunk2.rows.iter())
        .chain(chunk3.rows.iter())
        .map(|row| match &row[0] {
            Value::BigInt(n) => *n,
            _ => 0,
        })
        .sum();
    let expected: i64 = (0..row_count as i64).sum();
    assert_eq!(total, expected);
}

#[test]
fn scan_without_col_names_infers_layout_from_row_width() {
    let base = OperatorBase::new(0);
    let mut source = source(SourceOperatorKind::ScanVertices {
        buffer: vec![vec![Value::BigInt(1), Value::string("a")]],
        current_index: 0,
        col_names: Vec::new(),
    });
    source.inject_context(base.runtime.as_ref(), config_for(&base));
    source.open().expect("source should open");
    let mut chunk = source
        .next()
        .expect("pull should succeed")
        .expect("chunk should be Some");
    assert_eq!(chunk.num_columns(), 2);
    assert_eq!(chunk.col_names(), vec!["c0", "c1"]);
    let col = chunk.get_column(0).expect("column derived from rows");
    assert_eq!(col.len(), 1);
}

#[test]
fn buffered_scan_propagates_memory_budget_errors() {
    let runtime = Arc::new(ExecutionRuntime::new(
        QueryIdentity::default(),
        MemoryBudget::new(0),
        None,
        crate::executor::base::SearchContext::default(),
    ));
    let base = OperatorBase::new(0).with_runtime(Some(runtime));
    let mut source = source(SourceOperatorKind::ScanVertices {
        buffer: vec![vec![Value::string("row")]],
        current_index: 0,
        col_names: Vec::new(),
    });
    source.inject_context(base.runtime.as_ref(), config_for(&base));

    source.open().expect("source should open");
    let error = source
        .next()
        .expect_err("the source must propagate a memory budget error");
    assert!(error.to_string().contains("Memory budget exceeded"));
}

#[test]
fn buffered_source_open_returns_configuration_errors() {
    let mut source = source(SourceOperatorKind::GetVertices {
        storage: None,
        space_name: "test".to_string(),
        tag: "person".to_string(),
        vertex_ids: None,
        cached_ids: Vec::new(),
        projected_properties: Vec::new(),
    });
    let base = OperatorBase::new(0);
    source.inject_context(base.runtime.as_ref(), config_for(&base));

    let error = source.next().expect_err("source without storage must fail");
    assert!(error.to_string().contains("requires storage"));
    assert!(source.close().is_ok());
}

#[test]
fn get_vertices_single_id_returns_none_on_second_call() {
    let mock = crate::storage::MockStorage::new().expect("MockStorage should be created");
    let storage = Arc::new(RwLock::new(mock));
    let mut source = source(SourceOperatorKind::GetVertices {
        storage: Some(storage),
        space_name: "test".to_string(),
        tag: "person".to_string(),
        vertex_ids: Some(vec![Value::string("1")]),
        cached_ids: Vec::new(),
        projected_properties: Vec::new(),
    });
    let runtime = Arc::new(ExecutionRuntime::new(
        QueryIdentity::default(),
        MemoryBudget::new(1024 * 1024),
        None,
        crate::executor::base::SearchContext::default(),
    ));
    let base = OperatorBase::new(0).with_runtime(Some(runtime));
    source.inject_context(base.runtime.as_ref(), config_for(&base));

    source.open().expect("open should succeed");

    let result1 = source.next().expect("first next should succeed");
    assert!(
        result1.is_none(),
        "no vertex in mock, first call returns None"
    );

    let result2 = source.next().expect("second next should succeed");
    assert!(
        result2.is_none(),
        "second call must also return None (regression: do not re-emit)"
    );
}

#[test]
fn get_edges_projected_filters_edge_properties() {
    use linkrs_core::Edge;
    let mock = crate::storage::MockStorage::new().expect("MockStorage should be created");
    let src =
        linkrs_core::types::storage_ids::VertexId::try_from_int64(1).expect("valid vertex id");
    let dst =
        linkrs_core::types::storage_ids::VertexId::try_from_int64(2).expect("valid vertex id");
    mock.set_edges(vec![Edge {
        src,
        dst,
        edge_type: "friend".to_string(),
        ranking: 0,
        props: vec![
            ("degree".into(), Value::Double(0.8)),
            ("since".into(), Value::Int(2020)),
        ]
        .into_iter()
        .collect(),
    }]);
    let storage = Arc::new(RwLock::new(mock));
    let mut source = source(SourceOperatorKind::GetEdges {
        storage: Some(storage),
        space_name: "test".to_string(),
        edge_type: Some("friend".to_string()),
        src: Some("1".to_string()),
        dst: Some("2".to_string()),
        rank: 0,
        projected_properties: vec![Arc::from("degree")],
        cursor: None,
    });
    let runtime = Arc::new(ExecutionRuntime::new(
        QueryIdentity::default(),
        MemoryBudget::new(1024 * 1024),
        None,
        crate::executor::base::SearchContext::default(),
    ));
    let base = OperatorBase::new(0).with_runtime(Some(runtime));
    source.inject_context(base.runtime.as_ref(), config_for(&base));

    source.open().expect("open should succeed");

    let chunk = source
        .next()
        .expect("first next should succeed")
        .expect("edge must be returned");
    assert_eq!(chunk.len(), 1);
    let row = &chunk.rows[0];
    assert_eq!(row.len(), 2, "edge column + one flat property column");
    match &row[0] {
        Value::Edge(edge) => {
            assert_eq!(edge.properties().len(), 1, "only degree must be kept");
            assert_eq!(edge.properties().get("degree"), Some(&Value::Double(0.8)));
        }
        other => panic!("expected Value::Edge, got {:?}", other),
    }
    assert_eq!(row[1], Value::Double(0.8));

    assert!(source.next().expect("second next should succeed").is_none());
}

// ── Reset protocol ──

#[test]
fn buffered_scan_reset_rewinds_the_buffer() {
    let mut source = source(SourceOperatorKind::ScanVertices {
        buffer: vec![
            vec![Value::BigInt(1)],
            vec![Value::BigInt(2)],
            vec![Value::BigInt(3)],
        ],
        current_index: 0,
        col_names: Vec::new(),
    });
    let base = OperatorBase::new(0);
    source.inject_context(base.runtime.as_ref(), config_for(&base));

    source.open().expect("open should succeed");
    let mut all = Vec::new();
    while let Some(chunk) = source.next().expect("first run should succeed") {
        all.extend(chunk.rows);
    }
    assert_eq!(all.len(), 3, "first run emits every row");

    source
        .reset()
        .expect("reset should succeed without fallback");

    let mut again = Vec::new();
    while let Some(chunk) = source.next().expect("second run should succeed") {
        again.extend(chunk.rows);
    }
    assert_eq!(again, all, "reset re-produces the identical stream");
}

#[test]
fn start_source_reset_remits_its_single_row() {
    let runtime = Arc::new(ExecutionRuntime::new(
        QueryIdentity::default(),
        MemoryBudget::new(1024 * 1024),
        None,
        crate::executor::base::SearchContext::default(),
    ));
    let base = OperatorBase::new(0).with_runtime(Some(runtime));
    let mut source = source(SourceOperatorKind::Start);
    source.inject_context(base.runtime.as_ref(), config_for(&base));

    source.open().expect("open should succeed");
    assert!(source.next().expect("pull").is_some());
    assert!(source.next().expect("pull").is_none());

    source.reset().expect("reset should succeed");
    assert!(
        source.next().expect("pull").is_some(),
        "reset un-emits Start"
    );
    assert!(source.next().expect("pull").is_none());
}

#[test]
fn argument_source_reads_the_injected_frame_per_reset() {
    let layout = Arc::new(SlotLayout::from_names(&["id".to_string()]));
    let mut source = SourceOperator::new(SourceOperatorKind::Argument, layout.clone());

    source.open().expect("open should succeed");

    source.frame = Some((layout.clone(), vec![Value::Int(1)]));
    let chunk = source.next().expect("pull").expect("frame row emitted");
    assert_eq!(chunk.rows, vec![vec![Value::Int(1)]]);
    assert!(
        source.next().expect("pull").is_none(),
        "frame is consumed once per run"
    );

    source.reset().expect("reset should succeed");
    source.frame = Some((layout.clone(), vec![Value::Int(2)]));
    let chunk = source.next().expect("pull").expect("new frame row emitted");
    assert_eq!(chunk.rows, vec![vec![Value::Int(2)]]);
}

#[test]
fn storage_backed_source_reset_reopens_and_repulls() {
    use crate::storage::MockStorage;
    use linkrs_core::types::storage_ids::VertexId;
    use linkrs_core::Edge;

    let mock = MockStorage::new().expect("MockStorage should be created");
    mock.set_edges(vec![Edge {
        src: VertexId::try_from_int64(1).expect("valid vertex id"),
        dst: VertexId::try_from_int64(2).expect("valid vertex id"),
        edge_type: "friend".to_string(),
        ranking: 0,
        props: Default::default(),
    }]);
    let storage: Arc<RwLock<dyn crate::storage::QueryStorage>> = Arc::new(RwLock::new(mock));
    let runtime = Arc::new(ExecutionRuntime::new(
        QueryIdentity::default(),
        MemoryBudget::new(1024 * 1024),
        Some(storage.clone()),
        crate::executor::base::SearchContext::default(),
    ));
    let base = OperatorBase::new(0).with_runtime(Some(runtime));
    let mut source = source(SourceOperatorKind::GetEdges {
        storage: Some(storage),
        space_name: "test".to_string(),
        edge_type: Some("friend".to_string()),
        src: Some("1".to_string()),
        dst: Some("2".to_string()),
        rank: 0,
        projected_properties: Vec::new(),
        cursor: None,
    });
    source.inject_context(base.runtime.as_ref(), config_for(&base));

    source.open().expect("open should succeed");
    let first = source.next().expect("pull").expect("edge emitted");
    assert_eq!(first.len(), 1);
    assert!(source.next().expect("pull").is_none());

    source.reset().expect("reset should succeed");
    let second = source
        .next()
        .expect("pull")
        .expect("same edge re-emitted after reset");
    assert_eq!(second.len(), 1);
    assert!(source.next().expect("pull").is_none());
}
