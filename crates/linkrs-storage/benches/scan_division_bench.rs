//! Vertex scan division microbenchmark.
//!
//! Isolates the storage cursor work used by partitioned scans: enumerate
//! visible topology rows and apply either an external-id range or a shard
//! slice. This is not an end-to-end query benchmark; no parser, planner or
//! execution pipeline is involved.
//!
//! Run with:
//!   cargo bench -p linkrs-storage --bench scan_division_bench

use linkrs_core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Value, Vertex};
use linkrs_storage::{
    open_vertex_scan, GraphStorage, RequiredProperty, ScanOptions, ShardSlice, StorageSchemaOps,
    StorageWriter,
};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

const SPACE: &str = "scan_div";
const TAG: &str = "Node";
const VERTEX_COUNT: i64 = 20_000;
const PARTITIONS: usize = 10;
const BATCH_SIZE: usize = 2048;
const ITERATIONS: usize = 12;

fn setup() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string()).with_properties(vec![
                PropertyDef::new("id".into(), DataType::BigInt),
                PropertyDef::new("value".into(), DataType::Double),
            ]),
        )
        .expect("create tag");
    let vertices: Vec<Vertex> = (0..VERTEX_COUNT)
        .map(|id| {
            Vertex::new(
                VertexId::try_from_int64(id).expect("valid vertex id"),
                Tag::new(
                    TAG.to_string(),
                    [
                        ("id".into(), Value::BigInt(id)),
                        ("value".into(), Value::Double(id as f64)),
                    ]
                    .into_iter()
                    .collect(),
                ),
            )
        })
        .collect();
    storage
        .batch_insert_vertices(SPACE, vertices)
        .expect("insert vertices");
    storage
}

fn median_us(samples: &[u128]) -> u128 {
    let mut values = samples.to_vec();
    values.sort_unstable();
    values[values.len() / 2]
}

fn drain(storage: &Arc<RwLock<GraphStorage>>, options: &ScanOptions) -> (usize, usize) {
    let mut cursor = open_vertex_scan(storage, SPACE, options).expect("open scan");
    let mut rows = 0usize;
    let mut batches = 0usize;
    loop {
        let batch = cursor
            .next_column_batch(&[], BATCH_SIZE)
            .expect("next column batch");
        if batch.is_empty() {
            break;
        }
        rows += batch.len();
        batches += 1;
    }
    (rows, batches)
}

fn measure(storage: &Arc<RwLock<GraphStorage>>, label: &str, options: &ScanOptions) {
    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut rows = 0usize;
    let mut batches = 0usize;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let (drained_rows, drained_batches) = drain(storage, options);
        samples.push(start.elapsed().as_micros());
        rows = drained_rows;
        batches = drained_batches;
    }
    let us = median_us(&samples);
    println!(
        "{label:<22} {us:>8} us  rows={rows:<7} batches={batches:<4} ns/row={}",
        us.saturating_mul(1000) / rows.max(1) as u128
    );
}

fn main() {
    println!("== vertex scan division microbenchmark ==");
    let storage = Arc::new(RwLock::new(setup()));
    let width = VERTEX_COUNT / PARTITIONS as i64;

    println!("vertices={VERTEX_COUNT}, partitions={PARTITIONS}, batch_size={BATCH_SIZE}");

    let full = ScanOptions {
        topology_only: true,
        projection: Some(Vec::new()),
        tag: Some(TAG.to_string()),
        batch_size: BATCH_SIZE,
        ..ScanOptions::default()
    };
    measure(&storage, "full", &full);

    // `ByValueRange`, the division used by row-partitioned scans. Every
    // partition still enumerates the whole table and prunes by external id.
    let ranged = ScanOptions {
        vertex_id_range: Some(0..width),
        ..full.clone()
    };
    measure(&storage, "external_id_range", &ranged);

    // `ByShardSlice`, the alternative division that partitions enumeration
    // itself instead of filtering after a full walk.
    let sliced = ScanOptions {
        shard_slice: Some(ShardSlice::new(0, PARTITIONS)),
        ..full.clone()
    };
    measure(&storage, "shard_slice", &sliced);

    // The anchored traversal scans carry property demand, so topology-only
    // numbers do not explain their scan_read_us. Measure the same shard slice
    // with a projected decode to separate enumeration from property work.
    let projected = ScanOptions {
        topology_only: false,
        projection: Some(vec![RequiredProperty::new("id".to_string())]),
        ..sliced.clone()
    };
    measure(&storage, "shard_slice_projected", &projected);

    let full_decode = ScanOptions {
        topology_only: false,
        projection: None,
        ..sliced.clone()
    };
    measure(&storage, "shard_slice_full_decode", &full_decode);
}
