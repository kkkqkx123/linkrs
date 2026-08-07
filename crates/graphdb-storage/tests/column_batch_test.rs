//! A1 column-block path correctness: `next_column_batch` must produce rows
//! identical to the row-based `next_flat_batch` path across projections,
//! nulls, pushed predicates, and partition ranges.

use std::sync::Arc;

use graphdb_storage::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb_storage::core::vertex_edge_path::Tag;
use graphdb_storage::core::{DataType, StorageError, Value, Vertex};
use graphdb_storage::storage::{
    open_vertex_scan, GraphStorage, RequiredProperty, ScanOptions, ScanPredicate,
    StorageSchemaOps, StorageWriter, VertexColumnBatch,
};
use parking_lot::RwLock;

fn setup_storage(nullable: bool) -> Arc<RwLock<GraphStorage>> {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new("t".to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).unwrap();
    storage
        .create_tag(
            "t",
            &TagInfo::new("Node".to_string()).with_properties(vec![
                PropertyDef::new("value".to_string(), DataType::BigInt).with_nullable(nullable),
                PropertyDef::new("group".to_string(), DataType::BigInt).with_nullable(nullable),
                PropertyDef::new("name".to_string(), DataType::String).with_nullable(nullable),
            ]),
        )
        .unwrap();
    storage
        .create_edge_type(
            "t",
            &EdgeTypeInfo::new("Link".to_string())
                .with_src_tag("Node".to_string())
                .with_dst_tag("Node".to_string()),
        )
        .unwrap();
    let mut vertices = Vec::new();
    for i in 0..500i64 {
        let mut props = vec![
            ("value".to_string(), Value::BigInt(i)),
            ("group".to_string(), Value::BigInt(i % 7)),
            ("name".to_string(), Value::string(format!("node_{i}"))),
        ];
        if nullable && i % 3 == 0 {
            props[0] = (
                "value".to_string(),
                Value::Null(graphdb_storage::core::value::NullType::Null),
            );
        }
        vertices.push(Vertex::new(
            VertexId::from_int64(i),
            vec![Tag::new("Node".to_string(), props.into_iter().collect())],
        ));
    }
    storage.batch_insert_vertices("t", vertices).unwrap();
    Arc::new(RwLock::new(storage))
}

fn options(projection: Vec<&str>, range: Option<std::ops::Range<i64>>) -> ScanOptions {
    let mut opts = ScanOptions::new();
    if !projection.is_empty() {
        opts.projection = Some(
            projection
                .into_iter()
                .map(|n| RequiredProperty::new(n.to_string()))
                .collect(),
        );
    }
    if let Some(r) = range {
        opts.vertex_id_range = Some(r);
    }
    opts.column_block_mode = true;
    opts
}

fn drain_columns(storage: &Arc<RwLock<GraphStorage>>, opts: &ScanOptions) -> Vec<Vec<Value>> {
    let prop_names: Vec<String> = opts
        .projection
        .as_ref()
        .map(|p| p.iter().map(|rp| rp.name.clone()).collect())
        .unwrap_or_default();
    let mut cursor = open_vertex_scan(storage, "t", opts).expect("open cursor");
    let mut out = Vec::new();
    loop {
        let batch = cursor
            .next_column_batch(&prop_names, 128)
            .expect("column batch");
        if batch.is_empty() {
            break;
        }
        for row in 0..batch.len() {
            let mut rec = vec![Value::from(batch.vids[row])];
            for col in &batch.columns {
                rec.push(
                    col.values
                        .value_at(row)
                        .unwrap_or(Value::Null(graphdb_storage::core::value::NullType::Null)),
                );
            }
            out.push(rec);
        }
    }
    out
}

fn drain_rows(storage: &Arc<RwLock<GraphStorage>>, opts: &ScanOptions) -> Vec<Vec<Value>> {
    // Canonical column order for normalization (matches the schema).
    let all_columns = ["value".to_string(), "group".to_string(), "name".to_string()];
    let columns: Vec<String> = match opts.projection.as_ref() {
        Some(projection) => projection.iter().map(|rp| rp.name.clone()).collect(),
        None => all_columns.to_vec(),
    };
    let mut cursor = open_vertex_scan(storage, "t", opts).expect("open cursor");
    let mut out = Vec::new();
    loop {
        let batch = cursor.next_flat_batch(128).expect("flat batch");
        if batch.is_empty() {
            break;
        }
        for rec in batch {
            let mut row = vec![Value::from(rec.vid)];
            for name in &columns {
                row.push(
                    rec.props
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, v)| v.clone())
                        .unwrap_or(Value::Null(graphdb_storage::core::value::NullType::Null)),
                );
            }
            out.push(row);
        }
    }
    out
}

#[test]
fn column_batch_matches_row_path_all_columns() {
    let storage = setup_storage(false);
    let opts = options(vec![], None);
    assert_eq!(drain_columns(&storage, &opts), drain_rows(&storage, &opts));
}

#[test]
fn column_batch_matches_row_path_projection() {
    let storage = setup_storage(false);
    let opts = options(vec!["value", "name"], None);
    assert_eq!(drain_columns(&storage, &opts), drain_rows(&storage, &opts));
}

#[test]
fn column_batch_matches_row_path_with_nulls() {
    let storage = setup_storage(true);
    let opts = options(vec![], None);
    assert_eq!(drain_columns(&storage, &opts), drain_rows(&storage, &opts));
}

#[test]
fn column_batch_matches_row_path_with_range() {
    let storage = setup_storage(false);
    let opts = options(vec![], Some(100..300));
    assert_eq!(drain_columns(&storage, &opts), drain_rows(&storage, &opts));
}

#[test]
fn column_batch_matches_row_path_with_predicate() {
    let storage = setup_storage(false);
    let mut opts = options(vec![], None);
    opts.predicate = Some(vec![ScanPredicate::ColumnRange {
        column: "value".to_string(),
        lower: None,
        upper: Some(Value::BigInt(200)),
        include_lower: false,
        include_upper: false,
    }]);
    assert_eq!(drain_columns(&storage, &opts), drain_rows(&storage, &opts));
}

#[test]
fn column_batch_selective_predicate_does_not_end_scan_early() {
    // Very selective predicate: most rows are filtered, and some windows are
    // entirely filtered out. The scan must still return every matching row.
    let storage = setup_storage(false);
    let mut opts = options(vec![], None);
    opts.predicate = Some(vec![ScanPredicate::ColumnEqual {
        column: "value".to_string(),
        value: Value::BigInt(7),
    }]);
    let columns = drain_columns(&storage, &opts);
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0][0], Value::from(VertexId::from_int64(7)));
}

#[test]
fn column_batch_limit_with_predicate() {
    // Limit applies to the returned (predicate-filtered) rows, so a window
    // whose rows are all filtered does not consume the limit budget.
    let storage = setup_storage(false);
    let mut opts = options(vec![], None);
    opts.predicate = Some(vec![ScanPredicate::ColumnRange {
        column: "value".to_string(),
        lower: Some(Value::BigInt(10)),
        upper: Some(Value::BigInt(500)),
        include_lower: true,
        include_upper: false,
    }]);
    opts.limit = Some(20);
    let mut cursor = open_vertex_scan(&storage, "t", &opts).expect("open cursor");
    let mut total = 0usize;
    loop {
        let batch = cursor.next_column_batch(&[], 128).expect("batch");
        if batch.is_empty() {
            break;
        }
        total += batch.len();
    }
    assert_eq!(total, 20);
}

#[test]
fn column_batch_typed_columns_are_scalar() {
    let storage = setup_storage(false);
    let opts = options(vec!["value", "group"], None);
    let mut cursor = open_vertex_scan(&storage, "t", &opts).expect("open cursor");
    let batch = cursor
        .next_column_batch(&["value".to_string(), "group".to_string()], 64)
        .expect("batch");
    assert_eq!(batch.len(), 64);
    assert_eq!(batch.columns.len(), 2);
    assert!(matches!(
        batch.columns[0].values,
        graphdb_storage::storage::ColumnValues::I64 { .. }
    ));
    assert!(matches!(
        batch.columns[1].values,
        graphdb_storage::storage::ColumnValues::I64 { .. }
    ));
    let _ = batch;
}

#[test]
fn column_batch_exhaustion_and_limit() {
    let storage = setup_storage(false);
    let mut opts = options(vec![], None);
    opts.limit = Some(250);
    let mut cursor = open_vertex_scan(&storage, "t", &opts).expect("open cursor");
    let mut total = 0usize;
    loop {
        let batch = cursor.next_column_batch(&[], 128).expect("batch");
        if batch.is_empty() {
            break;
        }
        total += batch.len();
    }
    assert_eq!(total, 250);
}

#[allow(dead_code)]
fn _silence_unused() -> Option<StorageError> {
    None
}

#[allow(dead_code)]
fn _unused(_: VertexColumnBatch) {}
