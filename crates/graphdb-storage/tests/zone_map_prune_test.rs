//! Zone-map chunk pruning correctness.
//!
//! Per-chunk min/max bounds maintained on vertex property columns must
//! never cause a scan to drop matching rows: bounds only widen on writes,
//! so pruning is conservative by construction. These tests pin the visible
//! behavior — pushed range/equality predicates over the columnar path must
//! return exactly the matching rows, including after updates that move
//! values outside previously-recorded chunk bounds.

use std::sync::Arc;

use graphdb_storage::core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb_storage::core::vertex_edge_path::Tag;
use graphdb_storage::core::{DataType, Value, Vertex};
use graphdb_storage::storage::{
    open_vertex_scan, GraphStorage, ScanOptions, ScanPredicate, StorageSchemaOps, StorageWriter,
};
use parking_lot::RwLock;

const SPACE: &str = "zmp";

fn setup_storage() -> Arc<RwLock<GraphStorage>> {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).unwrap();
    storage
        .create_tag(
            SPACE,
            &TagInfo::new("Node".to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .unwrap();

    // value = i for i in [0, 5000): spans multiple 1024-row zones.
    let vertices: Vec<Vertex> = (0..5000i64)
        .map(|i| {
            let props = vec![("value".to_string(), Value::BigInt(i))];
            Vertex::new(
                VertexId::from_int64(i),
                vec![Tag::new("Node".to_string(), props.into_iter().collect())],
            )
        })
        .collect();
    storage.batch_insert_vertices(SPACE, vertices).unwrap();
    Arc::new(RwLock::new(storage))
}

fn drain_with_predicate(
    storage: &Arc<RwLock<GraphStorage>>,
    predicates: Vec<ScanPredicate>,
    limit: Option<usize>,
) -> Vec<i64> {
    let mut opts = ScanOptions::new();
    opts.column_block_mode = true;
    opts.predicate = (!predicates.is_empty()).then_some(predicates);
    opts.limit = limit;
    let mut cursor = open_vertex_scan(storage, SPACE, &opts).expect("open cursor");
    let mut out = Vec::new();
    loop {
        let batch = cursor.next_column_batch(&[], 256).expect("column batch");
        if batch.is_empty() {
            break;
        }
        for row in 0..batch.len() {
            let vid = batch.vids[row].as_int64().expect("int vid");
            out.push(vid);
        }
    }
    out.sort_unstable();
    out
}

#[test]
fn range_outside_all_zone_bounds_returns_nothing() {
    let storage = setup_storage();
    let rows = drain_with_predicate(
        &storage,
        vec![ScanPredicate::ColumnRange {
            column: "value".to_string(),
            lower: Some(Value::BigInt(10_000)),
            upper: None,
            include_lower: true,
            include_upper: false,
        }],
        None,
    );
    assert!(rows.is_empty(), "no row has value >= 10000");
}

#[test]
fn range_spanning_multiple_zones_returns_exact_rows() {
    let storage = setup_storage();
    let rows = drain_with_predicate(
        &storage,
        vec![ScanPredicate::ColumnRange {
            column: "value".to_string(),
            lower: Some(Value::BigInt(1500)),
            upper: Some(Value::BigInt(3500)),
            include_lower: true,
            include_upper: false,
        }],
        None,
    );
    let expected: Vec<i64> = (1500..3500).collect();
    assert_eq!(rows, expected);
}

#[test]
fn equality_across_zone_boundary_finds_all_matches() {
    let storage = setup_storage();
    let rows = drain_with_predicate(
        &storage,
        vec![ScanPredicate::ColumnEqual {
            column: "value".to_string(),
            value: Value::BigInt(4096),
        }],
        None,
    );
    assert_eq!(rows, vec![4096]);
}

#[test]
fn update_widening_bounds_keeps_results_correct() {
    let storage = setup_storage();

    // Move one row far outside every recorded bound.
    let updated = Vertex::new(
        VertexId::from_int64(42),
        vec![Tag::new(
            "Node".to_string(),
            vec![("value".to_string(), Value::BigInt(99_999))]
                .into_iter()
                .collect(),
        )],
    );
    storage
        .write()
        .update_vertex(SPACE, updated)
        .expect("update vertex");

    // A query whose range covers the new extreme must still find it, and a
    // query below the old max must be unaffected.
    let high = drain_with_predicate(
        &storage,
        vec![ScanPredicate::ColumnRange {
            column: "value".to_string(),
            lower: Some(Value::BigInt(50_000)),
            upper: None,
            include_lower: true,
            include_upper: false,
        }],
        None,
    );
    assert_eq!(high, vec![42], "widened bounds must keep the row visible");

    let mid = drain_with_predicate(
        &storage,
        vec![ScanPredicate::ColumnRange {
            column: "value".to_string(),
            lower: Some(Value::BigInt(100)),
            upper: Some(Value::BigInt(200)),
            include_lower: true,
            include_upper: false,
        }],
        None,
    );
    let expected: Vec<i64> = (100..200).collect();
    assert_eq!(mid, expected);
}

#[test]
fn limit_applies_after_zone_pruning() {
    let storage = setup_storage();
    let rows = drain_with_predicate(
        &storage,
        vec![ScanPredicate::ColumnRange {
            column: "value".to_string(),
            lower: Some(Value::BigInt(0)),
            upper: Some(Value::BigInt(100)),
            include_lower: true,
            include_upper: false,
        }],
        Some(10),
    );
    // Limit takes the first matches in scan order (internal-id order), so
    // only the count and value domain can be pinned here.
    assert_eq!(rows.len(), 10);
    assert!(rows.iter().all(|&v| (0..100).contains(&v)));
}

#[test]
fn merged_ranges_tighten_conjunctive_bounds() {
    let predicates = vec![
        ScanPredicate::ColumnRange {
            column: "a".to_string(),
            lower: Some(Value::BigInt(0)),
            upper: Some(Value::BigInt(100)),
            include_lower: true,
            include_upper: true,
        },
        ScanPredicate::ColumnRange {
            column: "a".to_string(),
            lower: Some(Value::BigInt(50)),
            upper: Some(Value::BigInt(200)),
            include_lower: false,
            include_upper: false,
        },
        ScanPredicate::ColumnEqual {
            column: "b".to_string(),
            value: Value::BigInt(7),
        },
    ];
    let ranges = ScanPredicate::merged_ranges(&predicates);
    assert_eq!(ranges.len(), 2);

    let ra = ranges.iter().find(|r| r.column == "a").expect("range a");
    match (&ra.lower, &ra.upper) {
        (Some(l), Some(u)) => {
            assert_eq!(*l, Value::BigInt(50));
            assert!(!ra.include_lower, "exclusive lower tightens inclusive");
            assert_eq!(*u, Value::BigInt(100));
            assert!(ra.include_upper, "inclusive upper stays: 100 < 200-excl");
        }
        _ => panic!("expected both bounds"),
    }

    let rb = ranges.iter().find(|r| r.column == "b").expect("range b");
    assert_eq!(
        (&rb.lower, &rb.upper),
        (&Some(Value::BigInt(7)), &Some(Value::BigInt(7)))
    );

    // Overlap sanity on the tightened range.
    assert!(ra.overlaps(&Value::BigInt(60), &Value::BigInt(90)));
    assert!(!ra.overlaps(&Value::BigInt(200), &Value::BigInt(300)));
    assert!(!ra.overlaps(&Value::BigInt(0), &Value::BigInt(49)));
}
