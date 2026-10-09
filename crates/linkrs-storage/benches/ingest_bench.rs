//! Storage ingest bottleneck attribution.
//!
//! Merged from the former `import_bench` and `edge_point_write_bench`, which
//! shared the same memory-vs-persistent attribution method on the same batch
//! entry points. Three legs:
//!
//! 1. `batch_insert_vertices` CPU-side vs WAL-side split across batch sizes.
//! 2. `batch_insert_edges` CPU-side vs WAL-side split (vertex setup excluded).
//! 3. Single-commit vs batched-commit per-edge cost, memory and persistent.
//!
//! Method: the same batch runs on an in-memory storage (no WAL, no fsync;
//! CPU side only) and on a persistent storage (`GraphStorage::new_with_path`,
//! full WAL + Sync-durability commit). CPU share = T(in-memory) / T(persistent).
//!
//! Decision gates: while the CPU-side share stays small, import
//! parallelization is not justified; while the persistent single-commit
//! per-edge cost dominates the memory cost, callers should batch instead of
//! asking for vertex-level locks or shard-parallel writes.
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench -p linkrs-storage --bench ingest_bench

use std::collections::HashMap;
use std::hint::black_box;
use std::time::Instant;

use tempfile::TempDir;

use linkrs_core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Edge, Value, Vertex};
use linkrs_storage::{GraphStorage, StorageSchemaOps, StorageWriter};

const SPACE: &str = "ingest";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const BATCH_SIZES: [usize; 3] = [10_000, 100_000, 1_000_000];
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
                    [("value".into(), Value::BigInt(i as i64))]
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

fn measure(mut sample: impl FnMut() -> f64) -> f64 {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        samples.push(sample());
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    samples[ITERATIONS / 2]
}

fn measure_single(persistent: bool, edges: &[Edge]) -> f64 {
    measure(|| {
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
    measure(|| {
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

fn cpu_share(cpu_ms: f64, wal_ms: f64) -> f64 {
    if wal_ms > 0.0 {
        cpu_ms / wal_ms
    } else {
        1.0
    }
}

fn main() {
    println!("== storage ingest bottleneck attribution ==");
    println!(
        "machine: {} ({} visible cores)",
        machine_name(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    println!(
        "batch sizes = {:?}, iterations = {} (median)",
        BATCH_SIZES, ITERATIONS
    );

    // ── leg 1: vertices ─────────────────────────────────────────────────────
    println!("\n### batch_insert_vertices");
    println!(
        "{:>10} | {:>10} | {:>10} | {:>9} | {:>9}",
        "batch", "T_cpu ms", "T_wal ms", "CPU share", "WAL share"
    );
    for &size in &BATCH_SIZES {
        let vertices = build_vertices(size);
        let cpu_ms = measure(|| {
            let (mut storage, _dir) = new_storage(false);
            let start = Instant::now();
            storage
                .batch_insert_vertices(SPACE, vertices.clone())
                .expect("batch vertices");
            black_box(());
            start.elapsed().as_secs_f64() * 1000.0
        });
        let wal_ms = measure(|| {
            let (mut storage, _dir) = new_storage(true);
            let start = Instant::now();
            storage
                .batch_insert_vertices(SPACE, vertices.clone())
                .expect("batch vertices");
            black_box(());
            start.elapsed().as_secs_f64() * 1000.0
        });
        let share = cpu_share(cpu_ms, wal_ms);
        println!(
            "{:>10} | {:>10.2} | {:>10.2} | {:>8.1}% | {:>8.1}%",
            size,
            cpu_ms,
            wal_ms,
            share * 100.0,
            (1.0 - share) * 100.0
        );
    }

    // ── leg 2: edges ────────────────────────────────────────────────────────
    println!("\n### batch_insert_edges (vertex setup excluded)");
    println!(
        "{:>10} | {:>10} | {:>10} | {:>9} | {:>9}",
        "batch", "T_cpu ms", "T_wal ms", "CPU share", "WAL share"
    );
    for &size in &BATCH_SIZES {
        let edges = build_edges(size);
        let cpu_ms = measure(|| {
            let (mut storage, _dir) = new_storage(false);
            storage
                .batch_insert_vertices(SPACE, build_vertices(size))
                .expect("setup vertices");
            let start = Instant::now();
            storage
                .batch_insert_edges(SPACE, edges.clone())
                .expect("batch edges");
            black_box(());
            start.elapsed().as_secs_f64() * 1000.0
        });
        let wal_ms = measure(|| {
            let (mut storage, _dir) = new_storage(true);
            storage
                .batch_insert_vertices(SPACE, build_vertices(size))
                .expect("setup vertices");
            let start = Instant::now();
            storage
                .batch_insert_edges(SPACE, edges.clone())
                .expect("batch edges");
            black_box(());
            start.elapsed().as_secs_f64() * 1000.0
        });
        let share = cpu_share(cpu_ms, wal_ms);
        println!(
            "{:>10} | {:>10.2} | {:>10.2} | {:>8.1}% | {:>8.1}%",
            size,
            cpu_ms,
            wal_ms,
            share * 100.0,
            (1.0 - share) * 100.0
        );
    }

    // ── leg 3: single vs batched commits ────────────────────────────────────
    println!("\n### single-commit vs batched-commit per-edge cost");
    let edges = build_edges(EDGE_COUNT);
    let single_mem = measure_single(false, &edges);
    let single_wal = measure_single(true, &edges);
    let batch_mem = measure_batch(false, &edges);
    let batch_wal = measure_batch(true, &edges);

    println!(
        "{:>12} | {:>12} | {:>12} | {:>12}",
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

    println!(
        "\nresult: CPU-side share < 40% -> import parallelization not justified; \
        WAL/fsync share above 60% means point-write latency is durability-bound, \
        prefer caller batching over vertex-level locks or shard-parallel writes"
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
