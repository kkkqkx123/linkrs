//! Commit-path contention: staging-commit hold time plus gate-wait share.
//!
//! Merged from the former `edge_group_commit_bench` and `write_gate_bench`,
//! which measured two sides of the same serialized commit path:
//!
//! 1. Staging leg: one atomic `commit_staging_batch` across owner-group
//!    distributions (uniform versus single-group skewed) and batch sizes,
//!    reporting the group-split plan width, the write-skew snapshot and the
//!    hottest owner groups alongside commit wall time. The commit itself stays
//!    one serialized pass under the single-writer discipline; this leg only
//!    records how long that pass holds the table lock, so callers can size
//!    their explicit chunks from measured data.
//! 2. Gate leg: the share of auto-commit write time spent waiting on the
//!    global `AutoCommitWriteGate` when N threads write concurrently.
//!
//! Decision gate: while the per-batch commit time scales with batch size and
//! the gate-wait share stays small, group-parallel applies are not justified.
//! Reservation sizing stays at the fixed packed density target; the
//! reserve-versus-rebuild trade-off is covered by `csr_perf_bench`.
//!
//! Capacity contract, frozen for all callers: one batch is one atomic unit;
//! a batch holds the table lock for the whole apply with prefix rollback on
//! failure; batches of tens of thousands of entries are correct but callers
//! with very large fanouts chunk explicitly and treat each chunk as its own
//! atomic unit.
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench -p linkrs-storage --bench commit_gate_bench
//!
//! Machine requirement: >= 8 cores for the gate leg. Record CPU model and
//! core count for reproducibility.

use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Barrier;
use std::time::{Duration, Instant};

use linkrs_core::types::{EdgeStrategy, PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Value, Vertex};
use linkrs_storage::edge::edge_table::config::EdgeTableConfig;
use linkrs_storage::edge::{EdgeSchema, EdgeStore, RecordForm};
use linkrs_storage::{GraphStorage, StorageOperationContextOps, StorageSchemaOps, StorageWriter};

// ── staging leg ─────────────────────────────────────────────────────────────

/// Batch sizes for the scaling leg (commit hold-time growth).
const SIZES: [usize; 3] = [5_000, 10_000, 20_000];
/// Measurement repetitions per configuration (median reported).
const ITERATIONS: usize = 3;
/// Skewed leg reuses one owner group; uniform spreads across this many rows.
const UNIFORM_SPAN: u32 = 200_000;

fn make_table() -> EdgeStore {
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "bench".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: Vec::new(),
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::Columnar,
    };
    EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("bench table builds")
}

fn uniform_keys(n: usize) -> Vec<(u32, u32)> {
    (0..n)
        .map(|i| {
            (
                ((i as u64 * 7919) % UNIFORM_SPAN as u64) as u32,
                ((i as u64 * 104_729 + 7) % UNIFORM_SPAN as u64) as u32,
            )
        })
        .collect()
}

fn skewed_keys(n: usize) -> Vec<(u32, u32)> {
    (0..n)
        .map(|i| {
            (
                (i % 4000) as u32,
                ((i as u64 * 104_729 + 7) % UNIFORM_SPAN as u64) as u32,
            )
        })
        .collect()
}

struct CommitSample {
    commit_ms: f64,
    plan_groups: usize,
    skew_groups: usize,
    skew_total: u64,
    skew_max: u64,
    skew: f64,
}

fn measure_commit(keys: &[(u32, u32)]) -> CommitSample {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let table = make_table();
        // Distribution check first: the owner partition of the raw keys must
        // agree with the staging plan of the buffered batch, since an
        // insert-only batch routes inserts exactly once through both.
        let partition = table.partition_inserts_by_owner(keys);
        let mut batch = EdgeStore::staging_batch();
        for (src, dst) in keys {
            batch.stage_insert(*src, *dst, 0, &[], 100);
        }
        let plan = table.staging_group_plan(&batch);
        assert_eq!(
            partition.len(),
            plan.len(),
            "owner partition and staging plan must agree on group width"
        );
        let mut table = table;
        let start = Instant::now();
        let applied = table
            .commit_staging_batch(batch)
            .expect("bench batch commits");
        let commit_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(applied, keys.len(), "one batch commits atomically");
        let (skew_groups, skew_total, skew_max, skew) = table.write_contention_snapshot();
        black_box(applied);
        samples.push(CommitSample {
            commit_ms,
            plan_groups: plan.len(),
            skew_groups,
            skew_total,
            skew_max,
            skew,
        });
    }
    samples.sort_by(|a, b| a.commit_ms.partial_cmp(&b.commit_ms).expect("finite"));
    samples.swap_remove(ITERATIONS / 2)
}

fn make_hot_summary(n: usize, skewed: bool) -> String {
    let keys = if skewed {
        skewed_keys(n)
    } else {
        uniform_keys(n)
    };
    let mut table = make_table();
    let mut batch = EdgeStore::staging_batch();
    for (src, dst) in &keys {
        batch.stage_insert(*src, *dst, 0, &[], 100);
    }
    table
        .commit_staging_batch(batch)
        .expect("bench batch commits");
    let hot = table.hot_groups(1);
    hot.first().map(|(_, c)| c.to_string()).unwrap_or_default()
}

fn run_commit_scale() {
    println!("== edge staging-commit contention and batch-size baseline ==");
    println!(
        "{:>12} | {:>12} | {:>7} | {:>7} | {:>10} | {:>7} | {:>5}",
        "workload", "commit ms", "groups", "total", "max/group", "skew", "hot"
    );
    for &n in &SIZES {
        let keys = uniform_keys(n);
        let s = measure_commit(&keys);
        let hot = make_hot_summary(n, false);
        println!(
            "{:>12} | {:>12.2} | {:>7} | {:>7} | {:>10} | {:>6.2}x | {:>5}",
            format!("uniform/{n}"),
            s.commit_ms,
            s.plan_groups,
            s.skew_total,
            s.skew_max,
            s.skew,
            hot,
        );
        assert_eq!(s.skew_groups, s.plan_groups);
    }
    let skewed = skewed_keys(SIZES[2]);
    let s = measure_commit(&skewed);
    let hot = make_hot_summary(SIZES[2], true);
    println!(
        "{:>12} | {:>12.2} | {:>7} | {:>7} | {:>10} | {:>6.2}x | {:>5}",
        format!("skewed/{}", SIZES[2]),
        s.commit_ms,
        s.plan_groups,
        s.skew_total,
        s.skew_max,
        s.skew,
        hot,
    );
    println!(
        "\nresult: serial commit retained; skew snapshot bounds the sharding decision, \
        batch hold-time bounds caller chunking (see capacity contract in staging.rs)"
    );
}

// ── gate leg ────────────────────────────────────────────────────────────────

const GATE_SPACE: &str = "b2";
const GATE_TAG: &str = "Node";
/// Concurrent auto-commit writers per run.
const THREADS: [usize; 4] = [1, 4, 8, 16];
const STATEMENTS_PER_THREAD: usize = 4_000;

fn gate_setup() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(GATE_SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");
    storage
        .create_tag(
            GATE_SPACE,
            &TagInfo::new(GATE_TAG.to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("create tag");
    storage
}

struct GateRunResult {
    wall: Duration,
    gate_wait_nanos: u64,
    acquisitions: u64,
    total_statements: usize,
}

fn run_concurrent_writers(
    storage: &GraphStorage,
    threads: usize,
    next_id: &Arc<AtomicU64>,
) -> GateRunResult {
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
                            GATE_SPACE,
                            Vertex::new(
                                VertexId::try_from_int64(id).expect("valid vertex id"),
                                Tag::new(
                                    GATE_TAG.to_string(),
                                    [("value".into(), Value::BigInt(id))].into_iter().collect(),
                                ),
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
    GateRunResult {
        wall,
        gate_wait_nanos: after.wait_nanos - before.wait_nanos,
        acquisitions: after.acquisitions - before.acquisitions,
        total_statements: threads * STATEMENTS_PER_THREAD,
    }
}

fn run_gate_share() {
    println!("== write-path gate contention share ==");
    println!(
        "statements per thread = {STATEMENTS_PER_THREAD}, threads = {:?}",
        THREADS
    );
    println!(
        "{:>7} | {:>10} | {:>12} | {:>11} | {:>10} | {:>9}",
        "threads", "wall ms", "stmts/sec", "gate wait ms", "acq", "gate share"
    );
    let storage = gate_setup();
    let next_id = Arc::new(AtomicU64::new(1));
    let mut share_at_max = 0.0;
    for &threads in &THREADS {
        let r = run_concurrent_writers(&storage, threads, &next_id);
        let total_thread_time_ns = r.wall.as_nanos() as u64 * threads as u64;
        let share = if total_thread_time_ns > 0 {
            r.gate_wait_nanos as f64 / total_thread_time_ns as f64
        } else {
            0.0
        };
        if threads == THREADS[THREADS.len() - 1] {
            share_at_max = share;
        }
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
    if share_at_max < 0.05 {
        println!(
            "\nresult: gate wait share {:.2}% at N={} (< 5%) -> sharding not justified",
            share_at_max * 100.0,
            THREADS[THREADS.len() - 1]
        );
    } else {
        println!(
            "\nresult: gate wait share {:.2}% at N={} (>= 5%) -> review sharding",
            share_at_max * 100.0,
            THREADS[THREADS.len() - 1]
        );
    }
}

fn main() {
    println!(
        "machine: {} ({} visible cores)",
        machine_name(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    run_commit_scale();
    println!();
    run_gate_share();
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
