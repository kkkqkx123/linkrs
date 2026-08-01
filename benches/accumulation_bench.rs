//! Decision-gate benchmarks for columnar accumulation (Phase B).
//!
//! Measures the three gates defined in
//! `docs/analysis/query/columnar_options_compare_analysis.md` §7:
//!
//! 1. `hash_join_build` — HashJoin build side: current row double-clone
//!    (`build_side_hash` bucket + `all_right_rows`) vs columnar accumulation
//!    with the full `insert_chunk` cost (materialize + index + column
//!    extend, no per-row clones).
//! 2. `group_by` — GroupBy in-memory path: current `all_rows` + per-group
//!    row collection + per-group rescan, vs streaming accumulator map
//!    (`AggregateAccumulator`, one pass, no row materialization).
//! 3. `scan_group` — scan→group without row materialization: rows transposed
//!    from columnar input then grouped, vs columns fed directly into
//!    accumulators.
//!
//! Baseline implementations mirror the operator code paths faithfully:
//! - hash join: `operators/join_operator/hash_join.rs` build loop
//!   (materialize columns, key fast path via `try_column_value`, double clone).
//! - group by: `operators/blocking.rs` in-memory aggregation phase
//!   (all_rows collect → `HashMap<Vec<Value>, Vec<Vec<Value>>>` → rescan).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use graphdb_query::core::Value;
use graphdb_query::query::executor::streaming::chunk::DataChunk;
use graphdb_query::query::executor::streaming::helpers::accumulator_states::AggregateAccumulator;
use graphdb_query::query::executor::streaming::operators::join_operator::JoinKeyValue;
use graphdb_query::query::executor::streaming::slot::SlotLayout;

const ROW_SIZES: [usize; 2] = [100_000, 1_000_000];

fn create_row_chunk(size: usize, num_cols: usize) -> DataChunk {
    let names: Vec<String> = (0..num_cols).map(|i| format!("c{}", i)).collect();
    let layout = Arc::new(SlotLayout::from_names(&names));
    let rows: Vec<Vec<Value>> = (0..size)
        .map(|i| {
            let mut row = vec![
                Value::BigInt((i % 100_000) as i64),
                Value::string(format!("user_{}", i % 1000)),
                Value::Int((i % 80) as i32),
                Value::Double((i % 40) as f64),
            ];
            for c in 4..num_cols {
                row.push(Value::Double((i * c) as f64 * 0.001));
            }
            row
        })
        .collect();
    DataChunk::new_with_layout(rows, layout)
}

fn create_column_chunk(size: usize, num_cols: usize) -> DataChunk {
    let names: Vec<String> = (0..num_cols).map(|i| format!("c{}", i)).collect();
    let layout = Arc::new(SlotLayout::from_names(&names));
    let mut columns: Vec<Vec<Value>> = Vec::with_capacity(num_cols);
    columns.push((0..size).map(|i| Value::BigInt((i % 100_000) as i64)).collect());
    columns.push((0..size).map(|i| Value::string(format!("user_{}", i % 1000))).collect());
    columns.push((0..size).map(|i| Value::Int((i % 80) as i32)).collect());
    columns.push((0..size).map(|i| Value::Double((i % 40) as f64)).collect());
    for c in 4..num_cols {
        columns.push((0..size).map(|i| Value::Double(i as f64 * (c as f64) * 0.001)).collect());
    }
    DataChunk::from_columns(columns, layout)
}

// ───────────────────────── Gate 1: HashJoin build ─────────────────────────

/// Current operator pattern (hash_join.rs build loop): key from materialized
/// columns, row cloned twice (bucket + all_right_rows).
fn hash_join_build_rows(chunk: &mut DataChunk) -> usize {
    chunk.materialize_columns();
    let cols = chunk.columns.as_deref().unwrap();
    let mut build_side_hash: HashMap<JoinKeyValue, Vec<Vec<Value>>> = HashMap::new();
    let mut all_right_rows: Vec<Vec<Value>> = Vec::new();
    for (row_idx, row) in chunk.rows.iter().enumerate() {
        let key = JoinKeyValue::from(cols[0][row_idx].clone());
        build_side_hash.entry(key).or_default().push(row.clone());
        all_right_rows.push(row.clone());
    }
    build_side_hash.len() + all_right_rows.len()
}

/// Columnar candidate mirroring `HashJoinBuildSide::insert_chunk`: rows are
/// transposed via `materialize_columns`, the key → row index map is built, and
/// the chunk columns are moved into the accumulation store (extended across
/// chunks). The target is pre-seeded with one prior chunk so the measured
/// path is the cross-chunk `extend` (per-value copy), not just the
/// first-chunk move.
fn hash_join_build_columns(
    chunk: &mut DataChunk,
    target: &mut Vec<Vec<Value>>,
    base: usize,
) -> usize {
    chunk.materialize_columns();
    let cols = chunk.columns.as_deref().unwrap();
    let mut build_index: HashMap<JoinKeyValue, Vec<u32>> = HashMap::new();
    for (row_idx, _) in cols[0].iter().enumerate() {
        let key = JoinKeyValue::from(cols[0][row_idx].clone());
        build_index.entry(key).or_default().push((base + row_idx) as u32);
    }
    let chunk_cols = chunk.columns.take().unwrap();
    if target.is_empty() {
        *target = chunk_cols;
    } else {
        for (t, s) in target.iter_mut().zip(chunk_cols) {
            t.extend(s);
        }
    }
    build_index.len() + target[0].len()
}

/// Setup for the candidate: seed the accumulation store with one prior chunk
/// (columns only, outside the measurement) and produce the next chunk to
/// insert, matching the operator's steady-state build.
fn seed_build_target(size: usize) -> (DataChunk, Vec<Vec<Value>>) {
    let mut first = create_row_chunk(size, 4);
    first.materialize_columns();
    let target = first.columns.take().unwrap();
    (create_row_chunk(size, 4), target)
}

fn bench_hash_join_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_join_build");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(30);

    for size in &ROW_SIZES {
        group.bench_function(BenchmarkId::new("rows_double_clone", size), |b| {
            b.iter_batched(
                || create_row_chunk(*size, 4),
                |mut chunk| black_box(hash_join_build_rows(&mut chunk)),
                BatchSize::SmallInput,
            )
        });
        group.bench_function(BenchmarkId::new("columns_full_cost", size), |b| {
            b.iter_batched(
                || seed_build_target(*size),
                |(mut chunk, mut target)| black_box(hash_join_build_columns(&mut chunk, &mut target, *size)),
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

// ─────────────────────────── Gate 2: GroupBy ───────────────────────────

/// Current in-memory path (blocking.rs): collect all_rows, group rows by key
/// with per-group row storage, then rescan each group for the aggregate.
fn group_by_rows(
    chunk: &mut DataChunk,
    key_cols: &[usize],
    value_col: usize,
) -> (usize, f64) {
    let mut all_rows: Vec<Vec<Value>> = Vec::new();
    for row in std::mem::take(&mut chunk.rows) {
        all_rows.push(row);
    }
    let mut group_map: HashMap<Vec<Value>, Vec<Vec<Value>>> = HashMap::new();
    for row in all_rows.iter().cloned() {
        let key: Vec<Value> = key_cols.iter().map(|&k| row[k].clone()).collect();
        group_map.entry(key).or_default().push(row);
    }
    let mut sum = 0.0;
    for group_rows in group_map.values() {
        for row in group_rows {
            if let Value::Double(d) = row[value_col] {
                sum += d;
            }
        }
    }
    (group_map.len(), sum)
}

/// Columnar candidate: single pass streaming into per-group accumulators;
/// rows are consumed but never stored per group.
fn group_by_accumulator(
    chunk: &mut DataChunk,
    key_cols: &[usize],
    value_col: usize,
) -> (usize, f64) {
    let mut acc_map: HashMap<Vec<Value>, Vec<AggregateAccumulator>> = HashMap::new();
    let mut sum = 0.0;
    for row in std::mem::take(&mut chunk.rows) {
        let key: Vec<Value> = key_cols.iter().map(|&k| row[k].clone()).collect();
        let accs = acc_map.entry(key).or_insert_with(|| {
            vec![AggregateAccumulator::Sum(0.0), AggregateAccumulator::Count(0)]
        });
        accs[0].accumulate(&row[value_col]);
        accs[1].accumulate(&row[value_col]);
        sum += 1.0;
    }
    (acc_map.len(), sum)
}

fn bench_group_by(c: &mut Criterion) {
    let mut group = c.benchmark_group("group_by");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(30);

    for (key_name, key_cols) in [("1_key", vec![2usize]), ("3_key", vec![2usize, 3usize, 1usize])] {
        for size in &ROW_SIZES {
            let keys = key_cols.clone();
            group.bench_function(
                BenchmarkId::new(format!("rows_collect_{}", key_name), size),
                |b| {
                    b.iter_batched(
                        || create_row_chunk(*size, 4),
                        |mut chunk| black_box(group_by_rows(&mut chunk, &keys, 3)),
                        BatchSize::SmallInput,
                    )
                },
            );
            group.bench_function(
                BenchmarkId::new(format!("accumulator_{}", key_name), size),
                |b| {
                    b.iter_batched(
                        || create_row_chunk(*size, 4),
                        |mut chunk| black_box(group_by_accumulator(&mut chunk, &keys, 3)),
                        BatchSize::SmallInput,
                    )
                },
            );
        }
    }
    group.finish();
}

// ─────────────── Gate 3: scan→group without row materialization ───────────────

/// Baseline: columnar input (storage batch output) transposed into rows,
/// then grouped via per-group row collection (current group path).
fn scan_group_transpose(chunk: &mut DataChunk, key_col: usize, value_col: usize) -> (usize, f64) {
    chunk.materialize_columns();
    let cols = chunk.columns.clone().unwrap();
    let num_rows = cols[0].len();
    let mut rows: Vec<Vec<Value>> = Vec::with_capacity(num_rows);
    for row_idx in 0..num_rows {
        let mut row = Vec::with_capacity(cols.len());
        for col in &cols {
            row.push(col[row_idx].clone());
        }
        rows.push(row);
    }
    let mut group_map: HashMap<Vec<Value>, Vec<Vec<Value>>> = HashMap::new();
    for row in rows {
        let key = vec![row[key_col].clone()];
        group_map.entry(key).or_default().push(row);
    }
    let mut sum = 0.0;
    for group_rows in group_map.values() {
        for row in group_rows {
            if let Value::Double(d) = row[value_col] {
                sum += d;
            }
        }
    }
    (group_map.len(), sum)
}

/// Candidate: storage columns fed directly into accumulators; rows never
/// materialized.
fn scan_group_columns(chunk: &mut DataChunk, key_col: usize, value_col: usize) -> (usize, f64) {
    let cols = chunk.columns.as_deref().unwrap();
    let mut acc_map: HashMap<Value, AggregateAccumulator> = HashMap::new();
    let mut sum = 0.0;
    for (row_idx, _) in cols[0].iter().enumerate() {
        let key = cols[key_col][row_idx].clone();
        let acc = acc_map.entry(key).or_insert_with(|| AggregateAccumulator::Sum(0.0));
        acc.accumulate(&cols[value_col][row_idx]);
        sum += 1.0;
    }
    (acc_map.len(), sum)
}

fn bench_scan_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("scan_group");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(30);

    for size in &ROW_SIZES {
        group.bench_function(BenchmarkId::new("transpose_rows", size), |b| {
            b.iter_batched(
                || create_column_chunk(*size, 4),
                |mut chunk| black_box(scan_group_transpose(&mut chunk, 2, 3)),
                BatchSize::SmallInput,
            )
        });
        group.bench_function(BenchmarkId::new("columns_direct", size), |b| {
            b.iter_batched(
                || create_column_chunk(*size, 4),
                |mut chunk| black_box(scan_group_columns(&mut chunk, 2, 3)),
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_hash_join_build,
    bench_group_by,
    bench_scan_group,
);
criterion_main!(benches);
