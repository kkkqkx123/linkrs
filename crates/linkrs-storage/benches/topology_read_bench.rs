//! Batch topology read microbenchmark.
//!
//! Measures the storage work behind one columnar expand batch: adjacency
//! materialization, per-edge endpoint translation, and the lean degree-only
//! path. No parser, planner or execution pipeline is involved.
//!
//! Run with:
//!   cargo bench -p linkrs-storage --bench topology_read_bench

use linkrs_core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Edge, EdgeDirection, Value, Vertex};
use linkrs_storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};
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

const SPACE: &str = "topo_bench";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const VERTEX_COUNT: i64 = 20_000;
const EDGES_PER_VERTEX: usize = 16;
const SEED_COUNT: usize = 2048;
const ITERATIONS: usize = 20;

fn setup() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string())
                .with_properties(vec![PropertyDef::new("id".into(), DataType::BigInt)]),
        )
        .expect("create tag");
    storage
        .create_edge_type(
            SPACE,
            &EdgeTypeInfo::new(EDGE.to_string())
                .with_properties(vec![PropertyDef::new("weight".into(), DataType::BigInt)])
                .with_src_tag(TAG.to_string())
                .with_dst_tag(TAG.to_string()),
        )
        .expect("create edge type");

    let vertices: Vec<Vertex> = (0..VERTEX_COUNT)
        .map(|id| {
            Vertex::new(
                VertexId::try_from_int64(id).expect("valid vertex id"),
                Tag::new(
                    TAG.to_string(),
                    [("id".into(), Value::BigInt(id))].into_iter().collect(),
                ),
            )
        })
        .collect();
    for chunk in vertices.chunks(10_000) {
        storage
            .batch_insert_vertices(SPACE, chunk.to_vec())
            .expect("insert vertices");
    }

    let mut edges = Vec::with_capacity(VERTEX_COUNT as usize * EDGES_PER_VERTEX);
    for src in 0..VERTEX_COUNT {
        for k in 1..=EDGES_PER_VERTEX {
            edges.push(Edge {
                src: VertexId::try_from_int64(src).expect("valid vertex id"),
                dst: VertexId::try_from_int64((src + k as i64) % VERTEX_COUNT)
                    .expect("valid vertex id"),
                edge_type: EDGE.to_string(),
                ranking: 0,
                props: [("weight".into(), Value::BigInt(k as i64))]
                    .into_iter()
                    .collect(),
            });
        }
    }
    for chunk in edges.chunks(50_000) {
        storage
            .batch_insert_edges(SPACE, chunk.to_vec())
            .expect("insert edges");
    }
    storage
}

fn median_us(samples: &[u128]) -> u128 {
    let mut values = samples.to_vec();
    values.sort_unstable();
    values[values.len() / 2]
}

fn report(label: &str, samples: Vec<u128>, bytes: u128, children: usize) {
    let us = median_us(&samples);
    println!(
        "{label:<28} {us:>8} us  {ns_per_parent:>7} ns/parent  {bytes:>10} B/call  {bytes_per_parent:>8} B/parent  children={children}",
        ns_per_parent = us.saturating_mul(1000) / SEED_COUNT as u128,
        bytes_per_parent = bytes / SEED_COUNT as u128,
    );
}

fn main() {
    println!("== batch topology read microbenchmark ==");
    let storage = setup();
    let seeds: Vec<VertexId> = (0..SEED_COUNT as i64)
        .map(|id| VertexId::try_from_int64(id).expect("valid vertex id"))
        .collect();
    let edge_types = vec![EDGE.to_string()];
    let no_types: Vec<String> = Vec::new();
    let children = SEED_COUNT * EDGES_PER_VERTEX;
    println!("seeds={SEED_COUNT}, edges/seed={EDGES_PER_VERTEX}");

    let mut packed_samples = Vec::with_capacity(ITERATIONS);
    let mut packed_bytes = 0u128;
    for _ in 0..ITERATIONS {
        let before = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let start = Instant::now();
        let packed = storage
            .get_packed_node_edge_headers_batch(
                SPACE,
                &seeds,
                None,
                EdgeDirection::Out,
                &edge_types,
            )
            .expect("packed read");
        packed_samples.push(start.elapsed().as_micros());
        assert_eq!(packed.child_offsets.last().copied(), Some(children));
        packed_bytes += ALLOCATED_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(before) as u128;
    }
    report(
        "packed_headers (current)",
        packed_samples,
        packed_bytes / ITERATIONS as u128,
        children,
    );

    let mut legacy_samples = Vec::with_capacity(ITERATIONS);
    let mut legacy_bytes = 0u128;
    for _ in 0..ITERATIONS {
        let before = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let start = Instant::now();
        let batches = storage
            .get_node_edge_headers_batch(SPACE, &seeds, None, EdgeDirection::Out, &edge_types)
            .expect("legacy header read");
        legacy_samples.push(start.elapsed().as_micros());
        assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), children);
        legacy_bytes += ALLOCATED_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(before) as u128;
    }
    report(
        "per_parent_headers",
        legacy_samples,
        legacy_bytes / ITERATIONS as u128,
        children,
    );

    let mut neighbor_samples = Vec::with_capacity(ITERATIONS);
    let mut neighbor_bytes = 0u128;
    for _ in 0..ITERATIONS {
        let before = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let start = Instant::now();
        let batches = storage
            .neighbor_dst_ids_batch(SPACE, &seeds, EdgeDirection::Out, &no_types)
            .expect("neighbor read");
        neighbor_samples.push(start.elapsed().as_micros());
        assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), children);
        neighbor_bytes += ALLOCATED_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(before) as u128;
    }
    report(
        "neighbor_ids (lean)",
        neighbor_samples,
        neighbor_bytes / ITERATIONS as u128,
        children,
    );

    let mut degree_samples = Vec::with_capacity(ITERATIONS);
    let mut degree_bytes = 0u128;
    for _ in 0..ITERATIONS {
        let before = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let start = Instant::now();
        let degrees = storage
            .out_degree_batch(SPACE, &seeds, EdgeDirection::Out, &no_types)
            .expect("degree read");
        degree_samples.push(start.elapsed().as_micros());
        assert_eq!(degrees.iter().sum::<usize>(), children);
        degree_bytes += ALLOCATED_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(before) as u128;
    }
    report(
        "out_degree (count only)",
        degree_samples,
        degree_bytes / ITERATIONS as u128,
        children,
    );

    let projected: Vec<Arc<str>> = vec![Arc::from("id")];
    let mut full_vertex_samples = Vec::with_capacity(ITERATIONS);
    let mut full_vertex_bytes = 0u128;
    for _ in 0..ITERATIONS {
        let before = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let start = Instant::now();
        let vertices = storage
            .get_vertices_batch(SPACE, TAG, &seeds)
            .expect("full vertex read");
        full_vertex_samples.push(start.elapsed().as_micros());
        assert_eq!(vertices.len(), SEED_COUNT);
        full_vertex_bytes += ALLOCATED_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(before) as u128;
    }
    report(
        "destination full vertex",
        full_vertex_samples,
        full_vertex_bytes / ITERATIONS as u128,
        SEED_COUNT,
    );

    let mut vertex_proj_samples = Vec::with_capacity(ITERATIONS);
    let mut vertex_proj_bytes = 0u128;
    for _ in 0..ITERATIONS {
        let before = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let start = Instant::now();
        let vertices = storage
            .get_vertices_projected_batch(SPACE, TAG, &seeds, &projected)
            .expect("projected vertex read");
        vertex_proj_samples.push(start.elapsed().as_micros());
        assert_eq!(vertices.len(), SEED_COUNT);
        vertex_proj_bytes += ALLOCATED_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(before) as u128;
    }
    report(
        "destination projected id",
        vertex_proj_samples,
        vertex_proj_bytes / ITERATIONS as u128,
        SEED_COUNT,
    );

    let mut column_samples = Vec::with_capacity(ITERATIONS);
    let mut column_bytes = 0u128;
    for _ in 0..ITERATIONS {
        let before = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let start = Instant::now();
        let columns = storage
            .get_vertex_property_columns_batch(SPACE, TAG, &seeds, &projected)
            .expect("property column read");
        column_samples.push(start.elapsed().as_micros());
        assert_eq!(columns.len(), SEED_COUNT);
        column_bytes += ALLOCATED_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(before) as u128;
    }
    report(
        "destination property column",
        column_samples,
        column_bytes / ITERATIONS as u128,
        SEED_COUNT,
    );
}
