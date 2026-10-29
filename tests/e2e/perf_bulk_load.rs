//! Bulk-load performance regression guard.
//!
//! Guards against the O(n²) checkpoint pathology that made persistent bulk
//! ingests pathologically slow: before the bulk-load session
//! ([`linkrs::storage::GraphStorage::begin_bulk_load`]) and the incremental
//! checkpoint gating fix, every `checkpoint_threshold` (10000) WAL bytes
//! forced a **full-dataset** flush — the incremental/shadow-page path was
//! defeated during a bulk load (no baseline on the first checkpoint, and slow
//! checkpoints aged the baseline past `MAX_BASELINE_AGE_MS`, cascading further
//! full rewrites). Measured on the 20100-statement `optimizer_data.gql`:
//!
//! - grouped load, checkpoints active: ~194s total, per-statement cost grew
//!   ~12x across the load (1.32 ms → 16.8 ms);
//! - the same load in memory: ~28s, flat ~1.4 ms/statement (proves the
//!   regression is entirely in the persistence/checkpoint layer).
//!
//! With the bulk-load session the persistent load is expected flat at
//! ~1.4 ms/statement plus one final full checkpoint. The assertions below use
//! generous margins (4x wall-clock, 3x scaling) so they hold on slow runners
//! while still failing on any return of the O(n²) behavior.

use std::time::Instant;

use crate::common::{create_test_db, load_gql_file_grouped};

const DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/e2e/data");
const OPTIMIZER_DATA: &str = "optimizer_data.gql";

/// Wall-clock bound for the 20100-statement bulk load. Linear expectation is
/// ~30-40s on this dataset; the O(n²) regression measured ~194s. The bound
/// sits ~4x above the linear expectation and ~1.5x below the regression.
const TOTAL_SECS_BOUND: f64 = 120.0;

/// Per-statement cost growth allowed between the first and the last load
/// segment. Linear loads stay ≈1x (measured 1.0-1.4x); the O(n²) regression
/// measured ~12x. The 3x bound leaves room for background noise while
/// catching any superlinear checkpoint flush behavior.
const SCALING_RATIO_BOUND: f64 = 3.0;

/// Segment size for the scaling measurement (statements per timed segment).
const SEGMENT: usize = 5000;

fn parse_statements(content: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut buffer = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            if !buffer.is_empty() {
                statements.push(std::mem::take(&mut buffer));
            }
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') || trimmed.starts_with(')') {
            buffer.push(' ');
            buffer.push_str(trimmed);
        } else {
            if !buffer.is_empty() {
                statements.push(std::mem::take(&mut buffer));
            }
            buffer = trimmed.to_string();
        }
    }
    if !buffer.is_empty() {
        statements.push(buffer);
    }
    statements
}

fn is_insert(statement: &str) -> bool {
    statement.trim().to_uppercase().starts_with("INSERT ")
}

#[test]
fn perf_persistent_bulk_load_stays_linear() {
    let mut db = create_test_db();

    // Suspend threshold/time-based checkpoint and flush triggers for the
    // whole ingest; `finish` below anchors everything with one checkpoint.
    let session = db
        .storage()
        .read()
        .begin_bulk_load()
        .expect("begin bulk-load session");
    assert!(session.active(), "bulk-load suspension must be engaged");

    let content =
        std::fs::read_to_string(format!("{}/{}", DATA_DIR, OPTIMIZER_DATA)).expect("read data");
    let statements = parse_statements(&content);
    let mut inserts: Vec<String> = Vec::new();
    for statement in &statements {
        if is_insert(statement) {
            inserts.push(statement.clone());
        } else {
            db.execute_query(statement).expect("non-insert statement");
        }
    }
    assert_eq!(inserts.len(), 20100, "expected the full dataset");

    let total_start = Instant::now();
    let mut segment_ms_per_stmt: Vec<f64> = Vec::new();
    for chunk in inserts.chunks(SEGMENT) {
        let segment_start = Instant::now();
        db.execute_batch_grouped(chunk, 500)
            .expect("bulk-load segment");
        let segment = segment_start.elapsed().as_secs_f64();
        segment_ms_per_stmt.push(segment / chunk.len() as f64 * 1000.0);
    }
    session.finish().expect("final checkpoint");
    let total = total_start.elapsed().as_secs_f64();

    let first = segment_ms_per_stmt[0];
    let last = *segment_ms_per_stmt.last().expect("non-empty segments");
    let ratio = last / first;
    println!(
        "PERF bulk-load: total={total:.2}s first_segment={first:.2}ms/stmt \
         last_segment={last:.2}ms/stmt ratio={ratio:.2}x"
    );

    assert!(
        total < TOTAL_SECS_BOUND,
        "bulk load took {total:.1}s (bound {TOTAL_SECS_BOUND}s); did the O(n²) \
         full-flush checkpoint regression return?"
    );
    assert!(
        ratio <= SCALING_RATIO_BOUND,
        "per-statement cost grew {ratio:.2}x across the load \
         (first {first:.2}ms → last {last:.2}ms, bound {SCALING_RATIO_BOUND}x); \
         superlinear checkpoint/flush behavior is back"
    );

    // The load_bounds invariants also hold on the persistent path.
    let storage = db.storage();
    let guard = storage.read();
    let staged_wal = guard.staged_wal_len();
    assert!(
        staged_wal <= 8,
        "staged WAL must stay bounded across the bulk load, got {staged_wal}"
    );
    let retired = guard.retired_generation_count();
    assert!(
        retired <= 64,
        "retired index generations must stay bounded, got {retired}"
    );
}

#[test]
fn bulk_load_session_finish_runs_final_checkpoint() {
    let mut db = create_test_db();

    let session = db
        .storage()
        .read()
        .begin_bulk_load()
        .expect("begin bulk-load session");
    assert!(session.active(), "suspension must be engaged");

    // Load data with the session open. Whether checkpoint triggers stay
    // suspended for the whole session is covered end-to-end by
    // `perf_persistent_bulk_load_stays_linear` (a mid-load full-dataset
    // checkpoint would blow both its wall-clock and scaling bounds);
    // `last_stats` below pins the exactly-one-final-checkpoint contract.
    load_gql_file_grouped(
        &mut db,
        &format!("{}/social_network_data.gql", DATA_DIR),
        500,
    )
    .expect("load social network data");

    let final_stats = session.finish().expect("final checkpoint");
    let stats = final_stats
        .unwrap_or_else(|| panic!("finish must anchor the load with exactly one final checkpoint"));
    assert_eq!(
        stats.trigger_reason,
        linkrs_metrics::CheckpointTriggerReason::Explicit,
        "the final checkpoint must be the session's explicit anchor"
    );
    assert!(
        stats.data_flushed > 0,
        "final checkpoint must have flushed the loaded data"
    );
}
