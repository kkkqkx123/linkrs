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
//! Scale notes: the 1M single batch is the heaviest write issued here and
//! runs under a WAL-growth watchdog that fails fast with stall evidence
//! instead of hanging constrained machines. Callers on constrained machines
//! should chunk at or below 100k rows per batch; the single-batch path
//! stays as the throughput baseline.
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench -p linkrs-storage --bench ingest_bench

use std::collections::HashMap;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
/// Largest batch size: the only leg that ever approaches resource limits.
/// Smaller legs run unwatched.
const WATCHED_SIZE: usize = 1_000_000;
/// Watchdog polls WAL growth this often and fails the bench when the WAL
/// shows no growth for this long (stalled batch, not slow batch).
const WATCHDOG_POLL: Duration = Duration::from_secs(5);
const WATCHDOG_STALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Stall watchdog for million-row persistent legs.
///
/// The 1M single batch is the heaviest write the bench issues; on a
/// resource-constrained machine it can stall behind memory/IO pressure with
/// no CPU signal. The watchdog polls the active WAL directory size and
/// proves monotonic WAL growth in the log; when growth stalls past
/// [`WATCHDOG_STALL_TIMEOUT`] it prints the stall evidence and exits
/// non-zero instead of hanging CI forever. The measured closure reports
/// each fresh WAL directory through `current_wal` (one TempDir per
/// iteration); a replaced or vanished directory resets the baseline.
struct StallWatchdog {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl StallWatchdog {
    fn start(current_wal: Arc<Mutex<Option<PathBuf>>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let mut baseline: Option<(PathBuf, u64)> = None;
            let mut stalled_since = Instant::now();
            let mut last_report = Instant::now();
            while !flag.load(Ordering::Acquire) {
                std::thread::sleep(WATCHDOG_POLL);
                let dir = current_wal.lock().expect("watchdog state").clone();
                let Some(dir) = dir else {
                    continue;
                };
                let size = dir_size(&dir);
                match &baseline {
                    Some((watched, known)) if *watched == dir && *known == size => {
                        if last_report.elapsed() >= Duration::from_secs(30) {
                            eprintln!(
                                "watchdog: WAL stalled at {size} bytes for {:?} ({})",
                                stalled_since.elapsed(),
                                dir.display()
                            );
                            last_report = Instant::now();
                        }
                        if stalled_since.elapsed() >= WATCHDOG_STALL_TIMEOUT {
                            eprintln!(
                                "watchdog: no WAL growth for {:?} at {} bytes ({}); failing fast instead of hanging",
                                stalled_since.elapsed(),
                                size,
                                dir.display()
                            );
                            std::process::exit(42);
                        }
                    }
                    _ => {
                        baseline = Some((dir, size));
                        stalled_since = Instant::now();
                    }
                }
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn dir_size(dir: &PathBuf) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.clone()];
    while let Some(path) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

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
        // Million-row persistent legs run under the stall watchdog: each
        // iteration reports its fresh WAL directory, and a stall (no WAL
        // growth) fails fast with evidence instead of hanging the bench.
        // On constrained machines prefer caller chunking at or below 100k
        // rows per batch; the single-batch path stays for baseline duty.
        let watched = size == WATCHED_SIZE;
        let current_wal = Arc::new(Mutex::new(None));
        let watchdog = watched.then(|| StallWatchdog::start(Arc::clone(&current_wal)));
        let wal_ms = measure(|| {
            let (mut storage, dir) = new_storage(true);
            if watched {
                if let Some(d) = dir.as_ref() {
                    *current_wal.lock().expect("watchdog state") = Some(d.path().join("wal"));
                }
            }
            let start = Instant::now();
            storage
                .batch_insert_vertices(SPACE, vertices.clone())
                .expect("batch vertices");
            black_box(());
            start.elapsed().as_secs_f64() * 1000.0
        });
        if let Some(watchdog) = watchdog {
            watchdog.stop();
        }
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
        // Same watchdog cover as leg 1: the 1M persistent setup plus the
        // 1M edge batch both run inside the measured closure.
        let watched_edge = size == WATCHED_SIZE;
        let current_wal_edge = Arc::new(Mutex::new(None));
        let watchdog_edge =
            watched_edge.then(|| StallWatchdog::start(Arc::clone(&current_wal_edge)));
        let wal_ms = measure(|| {
            let (mut storage, dir) = new_storage(true);
            if watched_edge {
                if let Some(d) = dir.as_ref() {
                    *current_wal_edge.lock().expect("watchdog state") = Some(d.path().join("wal"));
                }
            }
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
        if let Some(watchdog_edge) = watchdog_edge {
            watchdog_edge.stop();
        }
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
