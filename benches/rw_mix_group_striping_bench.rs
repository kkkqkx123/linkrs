//! Read-write mixed contention baseline (Section 2 + 3 decision gates).
//!
//! Runs N concurrent point-writers (uniform vs single-group skewed) alongside
//! M fan-out readers, reporting write throughput, read p99 and the
//! skewed/uniform wall ratio. Section 3 (row-level locks) adds a "same
//! vertex vs different rows in same group" leg to compute the Amdahl upper
//! bound for intra-row contention.
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench rw_mix_group_striping_bench

use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Barrier;
use std::time::Instant;

use graphdb::core::types::graph_schema::EdgeDirection;
use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::storage::StorageReader;
use graphdb::storage::{GraphStorage, StorageSchemaOps, StorageWriter};

const SPACE: &str = "rw_mix";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const VERTEX_COUNT: i64 = 200_000;
const STMTS_PER_THREAD: usize = 2_000;
const UNIFORM_SPAN: i64 = 200_000;
const SKEWED_SRC: i64 = 7;
const READERS: usize = 4;
const ITERATIONS: usize = 3;

fn setup_storage() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string()).with_properties(vec![PropertyDef::new(
                "v".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("create tag");
    storage
        .create_edge_type(
            SPACE,
            &EdgeTypeInfo::new(EDGE.to_string())
                .with_src_tag(TAG.to_string())
                .with_dst_tag(dst_tag()),
        )
        .expect("create edge type");
    let verts: Vec<Vertex> = (0..VERTEX_COUNT)
        .map(|i| {
            Vertex::new(
                VertexId::try_from_int64(i).expect("vid"),
                Tag::new(TAG.to_string(), [("v".to_string(), Value::BigInt(i))].into()),
            )
        })
        .collect();
    storage
        .batch_insert_vertices(SPACE, verts)
        .expect("seed vertices");
    storage
}

fn dst_tag() -> String { TAG.to_string() }

fn build_edges(n: usize, uniform: bool) -> Vec<Edge> {
    (0..n)
        .map(|i| {
            let src = if uniform {
                let raw = ((i as i64 * 7919) % UNIFORM_SPAN) + 1;
                raw.min(VERTEX_COUNT - 1)
            } else {
                SKEWED_SRC
            };
            let dst = ((i as i64 * 104_729 + 7) % (VERTEX_COUNT - 1)) + 1;
            Edge {
                src: VertexId::try_from_int64(src).expect("src"),
                dst: VertexId::try_from_int64(dst).expect("dst"),
                edge_type: EDGE.to_string(),
                ranking: 0,
                props: std::collections::HashMap::new(),
            }
        })
        .collect()
}

fn seed_read_edges(storage: &mut GraphStorage) {
    let mut edges = Vec::with_capacity(100);
    for i in 0..100i64 {
        edges.push(Edge {
            src: VertexId::try_from_int64((i % 200) + 1).expect("src"),
            dst: VertexId::try_from_int64((i * 3 + 5) % (VERTEX_COUNT - 1) + 1).expect("dst"),
            edge_type: EDGE.to_string(),
            ranking: 0,
            props: std::collections::HashMap::new(),
        });
    }
    storage.batch_insert_edges(SPACE, edges).expect("seed edges");
}

struct LegResult {
    wall_ms: f64,
    throughput: f64,
    read_p99_us: f64,
    read_ops: u64,
}

fn measure_leg(uniform: bool, writer_threads: usize, run_readers: usize) -> LegResult {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let mut storage = setup_storage();
        seed_read_edges(&mut storage);
        let total_writes = STMTS_PER_THREAD * writer_threads;
        let edges = build_edges(total_writes, uniform);
        let edges_per_thread = edges.len() / writer_threads;
        let barrier = Arc::new(Barrier::new(writer_threads + run_readers));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let storage_read = storage.clone();
        let barrier_read = barrier.clone();
        let stop_read = stop.clone();
        let handles_read: Vec<_> = (0..run_readers)
            .map(|_r| {
                let handle = storage_read.clone();
                let barrier = barrier_read.clone();
                let stop = stop_read.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let mut p99_us: Vec<u64> = Vec::new();
                    let mut ops: u64 = 0;
                    while !stop.load(Ordering::Relaxed) {
                        let vid = VertexId::try_from_int64((ops as i64 * 104_729 + 3) % VERTEX_COUNT)
                            .expect("vid");
                        let t0 = Instant::now();
                        let _ = black_box(
                            handle
                                .neighbor_dst_ids_batch(
                                    SPACE,
                                    &[vid],
                                    EdgeDirection::Out,
                                    &[EDGE.to_string()],
                                )
                                .expect("neighbors"),
                        );
                        let elapsed = t0.elapsed().as_nanos() as u64;
                        p99_us.push(elapsed / 1000);
                        ops += 1;
                        if ops & 0xF == 0 {
                            std::thread::yield_now();
                        }
                    }
                    p99_us.sort();
                    let p99 = if p99_us.is_empty() {
                        0
                    } else {
                        let idx = ((p99_us.len() as f64 * 0.99).ceil() as usize)
                            .min(p99_us.len() - 1);
                        p99_us[idx]
                    };
                    (ops, p99)
                })
            })
            .collect();

        let start = Instant::now();
        let handles_write: Vec<_> = (0..writer_threads)
            .map(|t| {
                let mut handle = storage.clone();
                let barrier = barrier.clone();
                let edge_chunk: Vec<_> = edges
                    [t * edges_per_thread..(t + 1) * edges_per_thread]
                    .to_vec();
                std::thread::spawn(move || {
                    barrier.wait();
                    for e in edge_chunk {
                        handle
                            .insert_edge(SPACE, e)
                            .expect("point write");
                    }
                })
            })
            .collect();

        for h in handles_write {
            h.join().expect("write thread");
        }
        let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
        stop.store(true, Ordering::Relaxed);
        let mut all_read_ops: u64 = 0;
        let mut all_p99: u64 = 0;
        for h in handles_read {
            let (ops, p99) = h.join().expect("read thread");
            all_read_ops += ops;
            if p99 > all_p99 {
                all_p99 = p99;
            }
        }
        samples.push(LegResult {
            wall_ms,
            throughput: total_writes as f64 / (wall_ms / 1000.0),
            read_p99_us: all_p99 as f64,
            read_ops: all_read_ops,
        });
    }
    samples.sort_by(|a, b| a.wall_ms.partial_cmp(&b.wall_ms).expect("finite"));
    samples.swap_remove(ITERATIONS / 2)
}

fn main() {
    println!("== rw-mix contention / group-level striping decision ==");
    println!(
        "machine: {} ({} visible cores)",
        machine_name(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    println!(
        "stmts per writer thread = {STMTS_PER_THREAD}, writers = [2,4], readers = {READERS}, \
         vertices = {VERTEX_COUNT}"
    );
    println!(
        "{:>12} | {:>7} | {:>8} | {:>12} | {:>12} | {:>10} | {:>10}",
        "leg", "writers", "wall ms", "writes/sec", "read_p99 us", "read ops", "skew/uniform"
    );

    let writer_counts = [2, 4];
    for &writers in &writer_counts {
        let uniform = measure_leg(true, writers, READERS);
        let skewed = measure_leg(false, writers, READERS);
        let skew_ratio = if uniform.wall_ms > 0.0 {
            skewed.wall_ms / uniform.wall_ms
        } else {
            1.0
        };
        println!(
            "{:>12} | {:>7} | {:>8.2} | {:>12.0} | {:>12.1} | {:>10} | {:>10}",
            "uniform",
            writers,
            uniform.wall_ms,
            uniform.throughput,
            uniform.read_p99_us,
            uniform.read_ops,
            skew_ratio
        );
        println!(
            "{:>12} | {:>7} | {:>8.2} | {:>12.0} | {:>12.1} | {:>10} | {:>10}",
            "skewed",
            writers,
            skewed.wall_ms,
            skewed.throughput,
            skewed.read_p99_us,
            skewed.read_ops,
            skew_ratio
        );
    }

    println!("\n== Section 3 probe: same-row vs different-row-in-same-group ==");
    let same_row_writers = 2;
    let same_row = measure_leg(false, same_row_writers, 0);

    // Different row within same group: small uniform, owners collapse.
    let mut diff_samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let mut storage = setup_storage();
        seed_read_edges(&mut storage);
        let total = STMTS_PER_THREAD * same_row_writers;
        let edges: Vec<_> = (0..total)
            .map(|i| Edge {
                src: VertexId::try_from_int64(((i as i64 * 11) % 4000).abs() + 1).expect("src"),
                dst: VertexId::try_from_int64(
                    ((i as i64 * 104_729 + 7) % VERTEX_COUNT + 1).abs().max(1),
                )
                .expect("dst"),
                edge_type: EDGE.to_string(),
                ranking: 0,
                props: std::collections::HashMap::new(),
            })
            .collect();
        let edges_per_thread = edges.len() / same_row_writers;
        let barrier = Arc::new(Barrier::new(same_row_writers));
        let start = Instant::now();
        let handles: Vec<_> = (0..same_row_writers)
            .map(|t| {
                let mut handle = storage.clone();
                let barrier = barrier.clone();
                let chunk = edges[t * edges_per_thread..(t + 1) * edges_per_thread].to_vec();
                std::thread::spawn(move || {
                    barrier.wait();
                    for e in chunk {
                        handle
                            .insert_edge(SPACE, e)
                            .expect("point write");
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("thread");
        }
        diff_samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    diff_samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    let diff_wall_ms = diff_samples[ITERATIONS / 2];
    let diff_tput =
        STMTS_PER_THREAD as f64 * same_row_writers as f64 / (diff_wall_ms / 1000.0);

    println!(
        "{:>18} | {:>10} | {:>12}",
        "leg", "wall ms", "writes/sec"
    );
    println!(
        "{:>18} | {:>10.2} | {:>12.0}",
        "same_row", same_row.wall_ms, same_row.throughput
    );
    println!(
        "{:>18} | {:>10.2} | {:>12.0}",
        "diff_row_in_group", diff_wall_ms, diff_tput
    );
    let amdahl_share = if same_row.throughput > 0.0 {
        let speedup = diff_tput / same_row.throughput;
        let n = same_row_writers as f64;
        (speedup - 1.0) / (speedup * (n - 1.0)).max(1e-6)
    } else {
        0.0
    };
    println!(
        "\nSection 3 Amdahl upper bound for row-level locks: {:.1}%",
        amdahl_share * 100.0
    );

    println!("\n-- Decision signals --");
    println!("(a) skewed/uniform wall >= 2.0 AND skewed still dominates -> Section 2 justifiable.");
    println!("(b) Amdahl share >= 50% of residual contention after Section 2 -> Section 3 \
             justifiable.");
    println!("(c) skewed wall ~ uniform -> keep explicit chunking, not striping.");
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

// silence unused import
#[allow(dead_code)]
fn _unused(u: AtomicU64) { let _ = u; }
