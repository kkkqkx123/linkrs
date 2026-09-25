//! Edge point-write latency baseline: single commits versus batched commits.
//!
//! Compares one edge per commit (`insert_edge` in a loop) against one commit
//! for the whole batch (`batch_insert_edges`), each on an in-memory storage
//! (no WAL, no fsync; CPU side only) and on a persistent storage (full WAL +
//! commit fsync). The gap between the memory and persistent columns attributes
//! WAL fsync; the gap between single and batch attributes per-commit staging,
//! reservation and owner/index maintenance amortization.
//!
//! Decision gate for concurrency work: while the persistent single-commit
//! per-edge cost dominates the memory cost, vertex-level locks or shard-level
//! parallel writes are not justified; callers should batch instead. The edge
//! table stays serialized by design; large fanouts already chunk explicitly.
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench edge_point_write_bench

use std::collections::HashMap;
use std::hint::black_box;
use std::time::Instant;

use tempfile::TempDir;

use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::storage::{GraphStorage, StorageSchemaOps, StorageWriter};

const SPACE: &str = "edge_point_write";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const EDGE_COUNT: usize = 1000;
/// Measurement repetitions per configuration (median reported).
const ITERATIONS: usize = 3;

fn new_storage(persistent: bool) -> (GraphStorage, Option<TempDir>) {
    let (mut storage, dir) = if persistent {
        let dir = TempDir::new().expect("temp directory");
        let storage =
            GraphStorage::new_with_path(dir.path().to_path_buf()).expect("persistent storage");
        (storage, Some(dir))
    } else {
        (GraphStorage::new().expect("storage init"), None)
    };
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("create tag");
    storage
        .create_edge_type(
            SPACE,
            &EdgeTypeInfo::new(EDGE.to_string())
                .with_src_tag(TAG.to_string())
                .with_dst_tag(TAG.to_string()),
        )
        .expect("create edge type");
    (storage, dir)
}

fn build_vertices(count: usize) -> Vec<Vertex> {
    (0..count)
        .map(|i| {
            Vertex::new(
                VertexId::try_from_int64(i as i64).expect("valid vertex id"),
                Tag::new(
                    TAG.to_string(),
                    [("value".to_string(), Value::BigInt(i as i64))]
                        .into_iter()
                        .collect(),
                ),
            )
        })
        .collect()
}

fn build_edges(count: usize) -> Vec<Edge> {
    (0..count)
        .map(|i| Edge {
            src: VertexId::try_from_int64(i as i64).expect("valid vertex id"),
            dst: VertexId::try_from_int64((i as i64 + 1) % count as i64).expect("valid vertex id"),
            edge_type: EDGE.to_string(),
            ranking: 0,
            props: HashMap::new(),
        })
        .collect()
}

fn measure_median(mut sample: impl FnMut() -> f64) -> f64 {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        samples.push(sample());
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    samples[ITERATIONS / 2]
}

fn measure_single(persistent: bool, edges: &[Edge]) -> f64 {
    measure_median(|| {
        let (mut storage, _dir) = new_storage(persistent);
        storage
            .batch_insert_vertices(SPACE, build_vertices(edges.len()))
            .expect("setup vertices");
        let start = Instant::now();
        for edge in edges {
            storage
                .insert_edge(SPACE, edge.clone())
                .expect("single edge");
        }
        black_box(());
        start.elapsed().as_secs_f64() * 1000.0
    })
}

fn measure_batch(persistent: bool, edges: &[Edge]) -> f64 {
    measure_median(|| {
        let (mut storage, _dir) = new_storage(persistent);
        storage
            .batch_insert_vertices(SPACE, build_vertices(edges.len()))
            .expect("setup vertices");
        let start = Instant::now();
        storage
            .batch_insert_edges(SPACE, edges.to_vec())
            .expect("batch edges");
        black_box(());
        start.elapsed().as_secs_f64() * 1000.0
    })
}

fn main() {
    println!("== edge point-write latency baseline ==");
    println!(
        "machine: {} ({} visible cores)",
        machine_name(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    println!(
        "edges = {}, iterations = {} (median)",
        EDGE_COUNT, ITERATIONS
    );

    let edges = build_edges(EDGE_COUNT);
    let single_mem = measure_single(false, &edges);
    let single_wal = measure_single(true, &edges);
    let batch_mem = measure_batch(false, &edges);
    let batch_wal = measure_batch(true, &edges);

    println!(
        "\n{:>12} | {:>12} | {:>12} | {:>12}",
        "mode", "total ms", "per-edge us", "vs single"
    );
    let rows = [
        ("single/mem", single_mem),
        ("single/wal", single_wal),
        ("batch/mem", batch_mem),
        ("batch/wal", batch_wal),
    ];
    for (mode, total_ms) in rows {
        let per_edge_us = total_ms * 1000.0 / EDGE_COUNT as f64;
        let speedup = if total_ms > 0.0 {
            single_wal / total_ms
        } else {
            1.0
        };
        println!(
            "{:>12} | {:>12.2} | {:>12.2} | {:>11.2}x",
            mode, total_ms, per_edge_us, speedup
        );
    }

    let wal_share_single = if single_wal > 0.0 {
        1.0 - single_mem / single_wal
    } else {
        0.0
    };
    let wal_share_batch = if batch_wal > 0.0 {
        1.0 - batch_mem / batch_wal
    } else {
        0.0
    };
    println!(
        "\nWAL/fsync share: single={:.1}% batch={:.1}%",
        wal_share_single * 100.0,
        wal_share_batch * 100.0
    );
    println!(
        "batch amortization (wal): {:.1}x per-edge cheaper than single commits",
        single_wal / batch_wal.max(f64::MIN_POSITIVE)
    );
    println!(
        "\nresult: WAL/fsync share above 60% means point-write latency is durability-bound; \
        do not add vertex-level locks or shard-parallel writes, prefer caller batching"
    );
}

fn machine_name() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|info| {
            info.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}
