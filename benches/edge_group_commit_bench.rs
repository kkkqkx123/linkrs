//! Edge staging-commit contention and batch-size baseline.
//!
//! Measures one atomic `commit_staging_batch` across owner-group
//! distributions (uniform versus single-group skewed) and across batch sizes,
//! reporting the group-split plan width, the write-skew snapshot and the
//! hottest owner groups alongside commit wall time. The commit itself stays
//! one serialized pass under the single-writer discipline; this bench only
//! records how long that pass holds the table lock, so callers can size
//! their explicit chunks from measured data.
//!
//! Decision gate: while the per-batch commit time scales with batch size and
//! the gate-wait share measured by `write_gate_bench` stays small,
//! group-parallel applies are not justified and the capacity contract below
//! stands. Reservation sizing stays at the fixed packed density target; the
//! reserve-versus-rebuild trade-off is covered by `csr_perf_bench`.
//!
//! Capacity contract, frozen for all callers: one batch is one atomic unit;
//! a batch holds the table lock for the whole apply with prefix rollback on
//! failure; batches of tens of thousands of entries are correct but callers
//! with very large fanouts chunk explicitly and treat each chunk as its own
//! atomic unit.
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench edge_group_commit_bench

use std::hint::black_box;
use std::time::Instant;

use graphdb::core::types::EdgeStrategy;
use graphdb::storage::edge::edge_table::config::EdgeTableConfig;
use graphdb::storage::edge::{EdgeSchema, EdgeStore};

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
        record_form: graphdb::storage::edge::RecordForm::Columnar,
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

fn main() {
    println!("== edge staging-commit contention and batch-size baseline ==");
    println!(
        "machine: {} ({} visible cores)",
        machine_name(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
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
