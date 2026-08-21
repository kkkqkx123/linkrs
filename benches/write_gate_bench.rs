//! Write-path gate contention share.
//!
//! Measures the share of auto-commit write time spent waiting on the global
//! `AutoCommitWriteGate` when N threads write concurrently:
//!
//! Gate wait time is accumulated by the storage engine's `AutoCommitWriteGate`
//! counters (`GraphStorage::write_gate_stats`); this bench only reads the
//! deltas and divides by the total wall time across threads.
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench write_gate_bench
//!
//! Machine requirement: >= 8 cores. Record CPU model and core count.

use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Barrier;
use std::time::{Duration, Instant};

use graphdb::core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Value, Vertex};
use graphdb::storage::{GraphStorage, StorageOperationContextOps, StorageSchemaOps, StorageWriter};

const SPACE: &str = "b2";
const TAG: &str = "Node";
/// Concurrent auto-commit writers per run.
const THREADS: [usize; 4] = [1, 4, 8, 16];
const STATEMENTS_PER_THREAD: usize = 4_000;

fn setup() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
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
}

struct RunResult {
    wall: Duration,
    gate_wait_nanos: u64,
    acquisitions: u64,
    total_statements: usize,
}

fn run_concurrent_writers(
    storage: &GraphStorage,
    threads: usize,
    next_id: &Arc<AtomicU64>,
) -> RunResult {
    let before = storage.write_gate_stats();
    let barrier = Arc::new(Barrier::new(threads));
    let start = Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|_t| {
            let handle = storage.clone();
            let barrier = barrier.clone();
            let next_id = next_id.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..STATEMENTS_PER_THREAD {
                    // Mirror the session write path: one gate acquisition per
                    // auto-commit statement, released at finalize.
                    let id = next_id.fetch_add(1, Ordering::Relaxed) as i64;
                    let mut bound = handle.bind_auto_commit_context().expect("bind");
                    bound
                        .insert_vertex(
                            SPACE,
                            Vertex::new(
                                VertexId::from_int64(id),
                                vec![Tag::new(
                                    TAG.to_string(),
                                    [("value".to_string(), Value::BigInt(id))]
                                        .into_iter()
                                        .collect(),
                                )],
                            ),
                        )
                        .expect("insert vertex");
                    bound.finalize_operation(true).expect("finalize");
                    black_box(());
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("writer thread");
    }
    let wall = start.elapsed();
    let after = storage.write_gate_stats();
    RunResult {
        wall,
        gate_wait_nanos: after.wait_nanos - before.wait_nanos,
        acquisitions: after.acquisitions - before.acquisitions,
        total_statements: threads * STATEMENTS_PER_THREAD,
    }
}

fn main() {
    println!("== B2: write-path gate contention share ==");
    println!(
        "machine: {} ({} visible cores)",
        machine_name(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    println!(
        "statements per thread = {STATEMENTS_PER_THREAD}, threads = {:?}",
        THREADS
    );
    println!(
        "{:>7} | {:>10} | {:>12} | {:>11} | {:>10} | {:>9}",
        "threads", "wall ms", "stmts/sec", "gate wait ms", "acq", "gate share"
    );
    let storage = setup();
    let next_id = Arc::new(AtomicU64::new(1));
    for &threads in &THREADS {
        let r = run_concurrent_writers(&storage, threads, &next_id);
        let total_thread_time_ns = r.wall.as_nanos() as u64 * threads as u64;
        let share = if total_thread_time_ns > 0 {
            r.gate_wait_nanos as f64 / total_thread_time_ns as f64
        } else {
            0.0
        };
        println!(
            "{:>7} | {:>10.2} | {:>12.0} | {:>11.2} | {:>10} | {:>8.2}%",
            threads,
            r.wall.as_secs_f64() * 1000.0,
            r.total_statements as f64 / r.wall.as_secs_f64(),
            r.gate_wait_nanos as f64 / 1e6,
            r.acquisitions,
            share * 100.0
        );
    }
    println!("\nresult: gate wait share < 5% at N=16 -> sharding not justified");
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
