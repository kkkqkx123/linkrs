//! Multi-client write-write conflict benchmark for the transaction layer
//! real contention on a shared TransactionManager.
//!
//! Measures commit throughput and abort rate as the write-set overlap ratio
//! and client count change. Anchors the commit critical-section / linear
//! scan hotspots in certify.rs. Debug-mode friendly.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use linkrs_core::types::VertexId;
use linkrs_transaction::manager::TransactionManager;
use linkrs_transaction::types::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[path = "results_report.rs"]
mod report;
use report::write_results_report;

const WRITE_SET_SIZE: usize = 20;
const KEY_SPACE: usize = 200;

fn record_writes(mgr: &TransactionManager, txn: TransactionId, seed: usize, overlap: f64) {
    let ctx = mgr.get_context(txn).expect("context");
    for i in 0..WRITE_SET_SIZE {
        // With overlap=0 the keys are disjoint from any other client's set;
        // with overlap=1 every client touches the same key range.
        let key = if overlap >= 1.0 {
            i % KEY_SPACE
        } else {
            ((seed * 7919 + i * 104729) % ((KEY_SPACE as f64 * (1.0 - overlap)) as usize)).max(i)
        };
        ctx.record_vertex_write(VertexId::try_from_int64(key as i64).expect("valid vertex id"));
    }
}

/// Run `rounds` rounds with `clients` concurrent transactions per round and
/// the given write-set overlap ratio. Returns (committed, aborted, elapsed).
fn run_contention_rounds(
    mgr: &Arc<TransactionManager>,
    clients: usize,
    overlap: f64,
    rounds: usize,
) -> (usize, usize, Duration) {
    let start = Instant::now();
    let mut committed = 0usize;
    let mut aborted = 0usize;
    for round in 0..rounds {
        let mut handles = Vec::with_capacity(clients);
        for client in 0..clients {
            let mgr = Arc::clone(mgr);
            handles.push(std::thread::spawn(move || {
                let txn = mgr
                    .begin_insert_transaction(TransactionOptions::default())
                    .expect("begin");
                record_writes(&mgr, txn, round * 100 + client, overlap);
                let _ = mgr.check_write_set_conflict(txn);
                mgr.commit_transaction(txn).is_ok()
            }));
        }
        for h in handles {
            if h.join().expect("join") {
                committed += 1;
            } else {
                aborted += 1;
            }
        }
    }
    (committed, aborted, start.elapsed())
}

fn bench_conflict(c: &mut Criterion) {
    let mut report =
        String::from("multi-client write-write conflict (transaction layer, bench profile)\n\n");
    report.push_str(&format!(
        "write-set size: {} keys, key space: {}\n\n",
        WRITE_SET_SIZE, KEY_SPACE
    ));
    report.push_str("| clients | overlap | committed | aborted | abort rate | elapsed |\n|---|---|---|---|---|---|\n");

    for overlap in &[0.0f64, 0.5, 1.0] {
        for clients in &[1usize, 4, 8] {
            let mgr = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));
            let (committed, aborted, elapsed) = run_contention_rounds(&mgr, *clients, *overlap, 20);
            let total = committed + aborted;
            let rate = if total > 0 {
                aborted as f64 / total as f64
            } else {
                0.0
            };
            report.push_str(&format!(
                "| {} | {:.0}% | {} | {} | {:.1}% | {:.2} ms |\n",
                clients,
                overlap * 100.0,
                committed,
                aborted,
                rate * 100.0,
                elapsed.as_secs_f64() * 1000.0
            ));

            // Criterion: single mixed round as a timed op.
            let group_name = format!("contention_c{}_ov{}", clients, (overlap * 100.0) as u32);
            let mut group = c.benchmark_group(&group_name);
            group.measurement_time(Duration::from_secs(2));
            group.sample_size(10);
            group.warm_up_time(Duration::from_millis(300));
            let mgr2 = Arc::clone(&mgr);
            group.bench_function("round", |b| {
                b.iter(|| {
                    let (c0, a0, _) = run_contention_rounds(&mgr2, *clients, *overlap, 1);
                    black_box((c0, a0));
                });
            });
            group.finish();
        }
    }

    let path = write_results_report("txn_conflict_bench", "results.txt", &report);
    println!("report written to {}", path.display());
}

criterion_group!(benches, bench_conflict);
criterion_main!(benches);
