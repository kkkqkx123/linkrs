//! Batch-import bottleneck attribution.
//!
//! Splits `batch_insert_vertices` / `batch_insert_edges` time into a CPU-side
//! component (schema validation + allocation + shard locks + CSR insertion)
//! and an I/O-side component (WAL append + commit fsync):
//!
//! Method: the same batch is executed on an in-memory storage (no WAL, no
//! fsync; measures the CPU side only) and on a persistent storage
//! (`GraphStorage::new_with_path`, full WAL + Sync-durability commit).
//! CPU share = T(in-memory) / T(persistent).
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench import_bench

use std::hint::black_box;
use std::time::Instant;

use tempfile::TempDir;

use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::storage::{GraphStorage, StorageSchemaOps, StorageWriter};

const SPACE: &str = "b3";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const BATCH_SIZES: [usize; 3] = [10_000, 100_000, 1_000_000];
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
                VertexId::from_int64(i as i64),
                vec![Tag::new(
                    TAG.to_string(),
                    [("value".to_string(), Value::BigInt(i as i64))]
                        .into_iter()
                        .collect(),
                )],
            )
        })
        .collect()
}

fn build_edges(count: usize) -> Vec<Edge> {
    (0..count)
        .map(|i| Edge {
            src: VertexId::from_int64(i as i64),
            dst: VertexId::from_int64((i as i64 + 1) % count as i64),
            edge_type: EDGE.to_string(),
            ranking: 0,
            props: Default::default(),
        })
        .collect()
}

fn measure<F>(mut setup: F) -> f64
where
    F: FnMut() -> f64,
{
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        samples.push(setup());
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    samples[ITERATIONS / 2]
}

fn main() {
    println!("== B3: batch-import bottleneck attribution ==");
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

    // ── vertices ────────────────────────────────────────────────────────────
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
        let cpu_share = if wal_ms > 0.0 { cpu_ms / wal_ms } else { 1.0 };
        println!(
            "{:>10} | {:>10.2} | {:>10.2} | {:>8.1}% | {:>8.1}%",
            size,
            cpu_ms,
            wal_ms,
            cpu_share * 100.0,
            (1.0 - cpu_share) * 100.0
        );
    }

    // ── edges ───────────────────────────────────────────────────────────────
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
        let cpu_share = if wal_ms > 0.0 { cpu_ms / wal_ms } else { 1.0 };
        println!(
            "{:>10} | {:>10.2} | {:>10.2} | {:>8.1}% | {:>8.1}%",
            size,
            cpu_ms,
            wal_ms,
            cpu_share * 100.0,
            (1.0 - cpu_share) * 100.0
        );
    }

    println!(
        "\nresult: CPU-side share < 40% -> import parallelization not justified"
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
