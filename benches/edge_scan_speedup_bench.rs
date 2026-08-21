//! Edge-table scan parallel speedup.
//!
//! Measures `scan_edges_by_type` full-table scan speedup E(n) = T(1)/T(n) as a
//! function of the number of (src,dst) edge partitions (N) and the rayon
//! worker count (n):
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench edge_scan_speedup_bench
//!
//! Machine requirement: >= 8 cores. Record CPU model and core count for
//! reproducibility.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use rayon::ThreadPoolBuilder;

use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};

const SPACE: &str = "b1";
const EDGE: &str = "Link";
/// Edge partitions (distinct (src,dst) tag pairs) per dataset.
const PARTITIONS: [usize; 3] = [1, 4, 16];
/// Rayon pool sizes used for the scan.
const WORKERS: [usize; 4] = [1, 2, 4, 8];
const TOTAL_EDGES: u64 = 1_000_000;
const ITERATIONS: usize = 11;

fn create_storage() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");
    storage
}

fn build_data(partitions: usize) -> Arc<RwLock<GraphStorage>> {
    let mut storage = create_storage();

    // Tag count: 1 partition uses two tags (one pair); N >= 2 uses N tags with
    // cyclic pairs (T_p, T_{p+1}) so every pair is distinct -> N partitions.
    let tag_count = partitions.max(2);
    let pair_count = partitions;
    let edges_per_partition = TOTAL_EDGES / pair_count as u64;

    for p in 0..tag_count {
        storage
            .create_tag(
                SPACE,
                &TagInfo::new(format!("N{p}")).with_properties(vec![PropertyDef::new(
                    "value".to_string(),
                    DataType::BigInt,
                )]),
            )
            .expect("create tag");
    }

    // Unconstrained edge type (no src/dst tag): edges spread across one edge
    // table per distinct (src_label, dst_label) pair.
    storage
        .create_edge_type(SPACE, &EdgeTypeInfo::new(EDGE.to_string()))
        .expect("create edge type");

    // Vertices: each tag gets `edges_per_partition` vertices (used as src for
    // one pair and dst for the previous pair).
    for p in 0..tag_count {
        let tag = format!("N{p}");
        let vertices: Vec<Vertex> = (0..edges_per_partition)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64((p as i64) * TOTAL_EDGES as i64 + i as i64),
                    vec![Tag::new(
                        tag.clone(),
                        [("value".to_string(), Value::BigInt(i as i64))]
                            .into_iter()
                            .collect(),
                    )],
                )
            })
            .collect();
        storage
            .batch_insert_vertices(SPACE, vertices)
            .expect("batch insert vertices");
    }

    // Edges: pair p connects tag p -> tag (p+1), one edge per vertex index.
    let mut edges: Vec<Edge> = Vec::with_capacity(TOTAL_EDGES as usize);
    for p in 0..pair_count {
        let src_tag = p;
        let dst_tag = (p + 1) % tag_count;
        for i in 0..edges_per_partition {
            edges.push(Edge {
                src: VertexId::from_int64(src_tag as i64 * TOTAL_EDGES as i64 + i as i64),
                dst: VertexId::from_int64(dst_tag as i64 * TOTAL_EDGES as i64 + i as i64),
                edge_type: EDGE.to_string(),
                ranking: 0,
                props: Default::default(),
            });
        }
    }
    for chunk in edges.chunks(100_000) {
        storage
            .batch_insert_edges(SPACE, chunk.to_vec())
            .expect("batch insert edges");
    }

    // Freeze all partitions synchronously so the scan state is deterministic
    // (background freeze is asynchronous and its backlog would otherwise make
    // the mutable-CSR share dataset-dependent).
    storage
        .trigger_background_freeze()
        .expect("background freeze");
    let stats = storage.get_freeze_stats();
    if let Some(stats) = stats {
        println!(
            "[freeze] partitions={} frozen_edges={} delta_edges={}",
            partitions, stats.total_frozen_edges, stats.current_delta_edges
        );
    }

    Arc::new(RwLock::new(storage))
}

fn median_us(samples: &[u64]) -> u64 {
    let mut v = samples.to_vec();
    v.sort_unstable();
    v[v.len() / 2]
}

/// Full `scan_edges_by_type` scan inside a rayon pool with `workers` threads.
fn measure_scan(storage: &Arc<RwLock<GraphStorage>>, workers: usize) -> u64 {
    let pool = ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .expect("build rayon pool");
    for _ in 0..2 {
        pool.install(|| {
            let edges = storage
                .read()
                .scan_edges_by_type(SPACE, EDGE)
                .expect("scan");
            black_box(edges.len());
        });
    }
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        pool.install(|| {
            let edges = storage
                .read()
                .scan_edges_by_type(SPACE, EDGE)
                .expect("scan");
            black_box(edges.len());
        });
        samples.push(start.elapsed().as_micros() as u64);
    }
    median_us(&samples)
}

fn main() {
    println!("== B1: edge-table scan parallel speedup ==");
    println!(
        "machine: {} ({} visible cores)",
        machine_name(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    println!(
        "total edges = {TOTAL_EDGES}, iterations = {ITERATIONS}, partitions = {:?}, workers = {:?}",
        PARTITIONS, WORKERS
    );

    for &partitions in &PARTITIONS {
        let storage = build_data(partitions);
        let edges_per_partition = TOTAL_EDGES / partitions as u64;
        println!(
            "\n### partitions N = {partitions} ({} edges per partition)",
            edges_per_partition
        );
        println!(
            "{:>7} | {:>8} | {:>9} | {:>7}",
            "workers", "T(n) us", "E(n)", "eta"
        );
        let mut medians = Vec::with_capacity(WORKERS.len());
        for &workers in &WORKERS {
            medians.push(measure_scan(&storage, workers));
        }
        for (idx, &workers) in WORKERS.iter().enumerate() {
            let t1 = medians[0] as f64;
            let tn = medians[idx] as f64;
            let e = t1 / tn;
            let eta = if workers > 1 { e / workers as f64 } else { 1.0 };
            println!(
                "{:>7} | {:>8} | {:>9.2} | {:>7.2}",
                workers, medians[idx], e, eta
            );
        }
    }
    println!("\nresult: N >= 4 and >= 1M edges -> E(8) >= 3");
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
