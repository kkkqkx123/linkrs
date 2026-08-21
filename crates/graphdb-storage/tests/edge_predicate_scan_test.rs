//! Edge cursor pushed-predicate correctness.
//!
//! `ScanOptions::predicate` must pre-filter edge rows inside the scan
//! (hot mutable tables and cold segments alike) before offset/limit
//! accounting, and predicate columns outside the projection must still be
//! decoded for evaluation. The query layer keeps the original filter on
//! top, so these are pure performance semantics — but the counts here pin
//! the contract.

use std::sync::Arc;

use graphdb_storage::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb_storage::core::vertex_edge_path::Tag;
use graphdb_storage::core::{DataType, Edge, Value, Vertex};
use graphdb_storage::storage::{
    GraphStorage, RequiredProperty, ScanOptions, ScanPredicate, StorageReader, StorageSchemaOps,
    StorageWriter,
};
use parking_lot::RwLock;

const SPACE: &str = "eps";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const VERTEX_COUNT: i64 = 200;

fn setup_storage() -> Arc<RwLock<GraphStorage>> {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).unwrap();
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .unwrap();
    storage
        .create_edge_type(
            SPACE,
            &EdgeTypeInfo::new(EDGE.to_string())
                .with_src_tag(TAG.to_string())
                .with_dst_tag(TAG.to_string())
                .with_properties(vec![PropertyDef::new(
                    "weight".to_string(),
                    DataType::BigInt,
                )]),
        )
        .unwrap();

    let vertices: Vec<Vertex> = (0..VERTEX_COUNT)
        .map(|i| {
            Vertex::new(
                VertexId::from_int64(i),
                vec![Tag::new(TAG.to_string(), Default::default())],
            )
        })
        .collect();
    storage.batch_insert_vertices(SPACE, vertices).unwrap();

    // Every src gets two edges; the second one carries weight = src * 10,
    // the first carries no property record at all.
    let mut edges = Vec::with_capacity((VERTEX_COUNT * 2) as usize);
    for src in 0..VERTEX_COUNT {
        edges.push(Edge {
            src: VertexId::from_int64(src),
            dst: VertexId::from_int64((src + 1) % VERTEX_COUNT),
            edge_type: EDGE.to_string(),
            ranking: 0,
            props: Default::default(),
        });
        edges.push(Edge {
            src: VertexId::from_int64(src),
            dst: VertexId::from_int64((src + 2) % VERTEX_COUNT),
            edge_type: EDGE.to_string(),
            ranking: 1,
            props: [("weight".to_string(), Value::BigInt(src * 10))]
                .into_iter()
                .collect(),
        });
    }
    storage.batch_insert_edges(SPACE, edges).unwrap();
    Arc::new(RwLock::new(storage))
}

fn drain(storage: &Arc<RwLock<GraphStorage>>, opts: ScanOptions) -> Vec<Edge> {
    let reader = storage.read();
    let mut cursor = reader
        .create_edge_cursor(SPACE, &opts)
        .expect("open edge cursor");
    let mut out = Vec::new();
    loop {
        let batch = cursor.next_batch(64).expect("edge batch");
        if batch.is_empty() {
            break;
        }
        out.extend(batch);
    }
    out
}

#[test]
fn equality_predicate_keeps_only_matching_rows() {
    let storage = setup_storage();

    let mut opts = ScanOptions::new();
    opts.edge_type = Some(EDGE.to_string());
    opts.predicate = Some(vec![ScanPredicate::ColumnEqual {
        column: "weight".to_string(),
        value: Value::BigInt(500),
    }]);
    let rows = drain(&storage, opts);

    assert_eq!(rows.len(), 1, "exactly one edge has weight = 500");
    assert_eq!(
        rows[0].props.get("weight"),
        Some(&Value::BigInt(500)),
        "projected-out predicate columns must not leak into emitted rows"
    );
}

#[test]
fn range_predicate_and_projection_narrowing() {
    let storage = setup_storage();

    let mut opts = ScanOptions::new();
    opts.edge_type = Some(EDGE.to_string());
    // Predicate on a column that is NOT in the projection.
    opts.projection = Some(vec![RequiredProperty::new("unrelated".to_string())]);
    opts.predicate = Some(vec![ScanPredicate::ColumnRange {
        column: "weight".to_string(),
        lower: Some(Value::BigInt(1000)),
        upper: Some(Value::BigInt(1500)),
        include_lower: true,
        include_upper: false,
    }]);
    let rows = drain(&storage, opts);

    assert_eq!(rows.len(), 50, "src in [100, 150) => 50 weighted edges");
    assert!(
        !rows.iter().any(|e| e.props.contains_key("weight")),
        "rows must carry only projected columns"
    );
}

#[test]
fn limit_applies_after_predicate_filtering() {
    let storage = setup_storage();

    let mut opts = ScanOptions::new();
    opts.edge_type = Some(EDGE.to_string());
    opts.limit = Some(7);
    opts.predicate = Some(vec![ScanPredicate::ColumnEqual {
        column: "weight".to_string(),
        value: Value::BigInt(30),
    }]);
    let rows = drain(&storage, opts);

    assert_eq!(rows.len(), 1, "filter runs before limit accounting");
}

#[test]
fn missing_property_never_matches_predicate() {
    let storage = setup_storage();

    let mut opts = ScanOptions::new();
    opts.edge_type = Some(EDGE.to_string());
    opts.limit = Some(10_000);
    opts.predicate = Some(vec![ScanPredicate::ColumnRange {
        column: "weight".to_string(),
        lower: Some(Value::BigInt(i64::MIN)),
        upper: None,
        include_lower: true,
        include_upper: false,
    }]);
    let rows = drain(&storage, opts);

    assert_eq!(rows.len(), VERTEX_COUNT as usize);
}
