//! Storage-only visibility and column decode microbenchmark.
//!
//! Drives the public vertex scan cursor directly: no parser, planner or
//! execution pipeline is involved. Each reported row separates cursor
//! enumeration, repeated visibility checks, column decode and batch assembly
//! with the timing counters carried by `VertexColumnBatch`. Allocation bytes
//! are counted with the same global allocator technique as
//! `topology_read_bench`.
//!
//! Run with:
//!   cargo bench -p linkrs-storage --bench visibility_decode_bench

use linkrs_core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Value, Vertex};
use linkrs_storage::{
    open_vertex_scan, GraphStorage, RequiredProperty, ScanOptions, ScanTimings, ShardSlice,
    StorageSchemaOps, StorageWriter,
};
use parking_lot::RwLock;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

struct CountingAllocator;

static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

const SPACE: &str = "visibility_decode";
const TAG: &str = "Node";
const VERTEX_COUNT: i64 = 20_000;
const PARTITIONS: usize = 10;
const BATCH_SIZE: usize = 2048;
const ITERATIONS: usize = 16;

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
    for chunk in vertices.chunks(10_000) {
        storage
            .batch_insert_vertices(SPACE, chunk.to_vec())
            .expect("insert vertices");
    }
    storage
}

fn median_us(samples: &[u128]) -> u128 {
    let mut values = samples.to_vec();
    values.sort_unstable();
    values[values.len() / 2]
}

fn drain(storage: &Arc<RwLock<GraphStorage>>, options: &ScanOptions) -> (usize, ScanTimings) {
    let mut cursor = open_vertex_scan(storage, SPACE, options).expect("open scan");
    let mut rows = 0usize;
    let mut timings = ScanTimings::default();
    loop {
        let batch = cursor
            .next_column_batch(&[], BATCH_SIZE)
            .expect("next column batch");
        if batch.is_empty() {
            break;
        }
        rows += batch.len();
        timings.add(&batch.timings);
    }
    (rows, timings)
}

fn measure(storage: &Arc<RwLock<GraphStorage>>, label: &str, options: &ScanOptions) {
    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut total_bytes = 0u128;
    let mut total_timings = ScanTimings::default();
    let mut rows = 0usize;
    for _ in 0..ITERATIONS {
        let before = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let start = Instant::now();
        let (drained_rows, timings) = drain(storage, options);
        samples.push(start.elapsed().as_micros());
        total_bytes += ALLOCATED_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(before) as u128;
        total_timings.add(&timings);
        rows = drained_rows;
    }
    let us = median_us(&samples);
    let bytes = total_bytes / ITERATIONS as u128;
    let divide = |value: u64| value / ITERATIONS as u64;
    println!(
        "{label:<26} {us:>7} us  rows={rows:<6} {bytes_per_row:>6} B/row  enumerate={enumerate:>6} us  visibility={visibility:>6} us  decode={decode:>6} us  assemble={assemble:>6} us  predicate={predicate:>6} us",
        bytes_per_row = bytes / rows.max(1) as u128,
        enumerate = divide(total_timings.scan_enumerate_us),
        visibility = divide(total_timings.scan_visibility_us),
        decode = divide(total_timings.scan_decode_us),
        assemble = divide(total_timings.scan_assemble_us),
        predicate = divide(total_timings.scan_predicate_us),
    );
}

fn main() {
    println!("== visibility and column decode microbenchmark ==");
    let storage = Arc::new(RwLock::new(setup()));
    let sliced = ScanOptions {
        topology_only: true,
        projection: Some(Vec::new()),
        tag: Some(TAG.to_string()),
        batch_size: BATCH_SIZE,
        shard_slice: Some(ShardSlice::new(0, PARTITIONS)),
        ..ScanOptions::default()
    };
    println!(
        "vertices={VERTEX_COUNT}, partitions={PARTITIONS}, batch_size={BATCH_SIZE}, slice_rows={}",
        VERTEX_COUNT / PARTITIONS as i64
    );

    measure(&storage, "visibility_recheck", &sliced);

    let projected = ScanOptions {
        topology_only: false,
        projection: Some(vec![RequiredProperty::new("id".to_string())]),
        ..sliced.clone()
    };
    measure(&storage, "visibility_plus_one_column", &projected);

    let full = ScanOptions {
        topology_only: false,
        projection: None,
        ..sliced.clone()
    };
    measure(&storage, "visibility_plus_all_columns", &full);

    let full_table = ScanOptions {
        shard_slice: None,
        ..sliced.clone()
    };
    measure(&storage, "full_table_topology", &full_table);
}
