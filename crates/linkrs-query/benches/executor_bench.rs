//! Executor micro-benchmarks: expression evaluation, accumulation paths,
//! and columnar necessity probes.
//!
//! Merged from the former `operator_bench`, `accumulation_bench` and
//! `columnar_necessity_bench`, which shared the same `DataChunk` /
//! `SlotLayout` fixtures. Groups:
//!
//! - `expr_eval`, `filter_throughput`, `column_materialize`: per-chunk
//!   expression evaluation, filtered projection chains, typed-column builds.
//! - `hash_join_build`, `group_by`, `scan_group`: row-materializing operator
//!   paths vs columnar accumulation candidates.
//! - `numeric_promotion`, `row_vs_column_filter`, `wide_single_column_filter`,
//!   `typed_data_chunk_filter`, `selection_propagation_chain`,
//!   `selectivity_propagation`, `null_bitmap`,
//!   `nullable_typed_column_filter`, `autovectorization`: necessity
//!   experiments for the columnar optimization family.
//!
//! Run the SIMD probe twice for comparison:
//!   cargo bench -p linkrs-query --bench executor_bench
//!   RUSTFLAGS="-C target-cpu=native" cargo bench -p linkrs-query --bench executor_bench

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::BinaryOperator;
use linkrs_core::Value;
use linkrs_query::executor::streaming::chunk::DataChunk;
use linkrs_query::executor::streaming::helpers::accumulator_states::AggregateAccumulator;
use linkrs_query::executor::streaming::operators::join_operator::JoinKeyValue;
use linkrs_query::executor::streaming::slot::SlotLayout;

// ── shared fixtures ─────────────────────────────────────────────────────────

fn create_chunk(size: usize) -> DataChunk {
    let layout = Arc::new(SlotLayout::from_names(&[
        "id".into(),
        "name".into(),
        "age".into(),
        "score".into(),
    ]));
    let rows: Vec<Vec<Value>> = (0..size)
        .map(|i| {
            vec![
                Value::BigInt(i as i64),
                Value::string(format!("user_{}", i % 1000)),
                Value::Int((i % 80) as i32),
                Value::Double((i as f64) * 0.1),
            ]
        })
        .collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);
    chunk
}

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
    columns.push(
        (0..size)
            .map(|i| Value::BigInt((i % 100_000) as i64))
            .collect(),
    );
    columns.push(
        (0..size)
            .map(|i| Value::string(format!("user_{}", i % 1000)))
            .collect(),
    );
    columns.push((0..size).map(|i| Value::Int((i % 80) as i32)).collect());
    columns.push((0..size).map(|i| Value::Double((i % 40) as f64)).collect());
    for c in 4..num_cols {
        columns.push(
            (0..size)
                .map(|i| Value::Double(i as f64 * (c as f64) * 0.001))
                .collect(),
        );
    }
    DataChunk::project_columns(columns, layout)
}

const KEYS: usize = 5;

fn create_wide_chunk(size: usize) -> DataChunk {
    let names: Vec<String> = (0..KEYS).map(|i| format!("k{}", i)).collect();
    let layout = Arc::new(SlotLayout::from_names(&names));
    let rows: Vec<Vec<Value>> = (0..size)
        .map(|i| {
            (0..KEYS)
                .map(|k| Value::BigInt(((i % 1000) + k * 7919) as i64))
                .collect()
        })
        .collect();
    DataChunk::new_with_layout(rows, layout)
}

/// Materialize owned `Value` columns for every slot, mirroring the old
/// columnar materialization edge (typed layout when available, row-major
/// clone otherwise).
fn materialize_columns(chunk: &mut DataChunk) -> Vec<Vec<Value>> {
    chunk.build_typed_columns(true);
    (0..chunk.num_columns())
        .filter_map(|slot| chunk.get_column(slot))
        .collect()
}

// ── expression evaluation ───────────────────────────────────────────────────

fn bench_expression_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("expr_eval");

    for chunk_size in &[128usize, 512, 1024, 4096] {
        // Simple comparison: id > 50
        let simple_pred = Expression::Binary {
            left: Box::new(Expression::Variable("id".into())),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(Value::BigInt(50))),
        };

        // Compound comparison: age > 18 AND score > 5.0
        let compound_pred = Expression::Binary {
            left: Box::new(Expression::Binary {
                left: Box::new(Expression::Variable("age".into())),
                op: BinaryOperator::GreaterThan,
                right: Box::new(Expression::Literal(Value::Int(18))),
            }),
            op: BinaryOperator::And,
            right: Box::new(Expression::Binary {
                left: Box::new(Expression::Variable("score".into())),
                op: BinaryOperator::GreaterThan,
                right: Box::new(Expression::Literal(Value::Double(5.0))),
            }),
        };

        // Single column project: name
        let project_single = [Expression::Variable("name".into())];

        // Multi column project: name, age, score
        let project_multi = vec![
            Expression::Variable("name".into()),
            Expression::Variable("age".into()),
            Expression::Variable("score".into()),
        ];

        group.bench_function(BenchmarkId::new("simple_predicate", chunk_size), |b| {
            b.iter_batched(
                || create_chunk(*chunk_size),
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&simple_pred, None);
                },
                BatchSize::SmallInput,
            )
        });

        group.bench_function(BenchmarkId::new("compound_predicate", chunk_size), |b| {
            b.iter_batched(
                || create_chunk(*chunk_size),
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&compound_pred, None);
                },
                BatchSize::SmallInput,
            )
        });

        group.bench_function(BenchmarkId::new("project_single", chunk_size), |b| {
            b.iter_batched(
                || create_chunk(*chunk_size),
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&project_single[0], None);
                },
                BatchSize::SmallInput,
            )
        });

        group.bench_function(BenchmarkId::new("project_multi_each", chunk_size), |b| {
            b.iter_batched(
                || create_chunk(*chunk_size),
                |mut chunk| {
                    for expr in &project_multi {
                        let _ = chunk.evaluate_expression(expr, None);
                    }
                },
                BatchSize::SmallInput,
            )
        });

        group.bench_function(
            BenchmarkId::new("project_multi_expressions", chunk_size),
            |b| {
                b.iter_batched(
                    || create_chunk(*chunk_size),
                    |mut chunk| {
                        let _ = chunk.evaluate_expressions(&project_multi, None);
                    },
                    BatchSize::SmallInput,
                )
            },
        );
    }

    group.finish();
}

fn bench_filter_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("filter_throughput");

    for chunk_size in &[128usize, 512, 1024, 4096] {
        // Low selectivity: id > 9999999 (almost no rows pass)
        let low_sel = Expression::Binary {
            left: Box::new(Expression::Variable("id".into())),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(Value::BigInt(9999999))),
        };

        // Medium selectivity: age > 18
        let med_sel = Expression::Binary {
            left: Box::new(Expression::Variable("age".into())),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(Value::Int(18))),
        };

        // High selectivity: id >= 0 (all rows pass)
        let high_sel = Expression::Binary {
            left: Box::new(Expression::Variable("id".into())),
            op: BinaryOperator::GreaterThanOrEqual,
            right: Box::new(Expression::Literal(Value::BigInt(0))),
        };

        for (sel_name, pred) in [("low", &low_sel), ("medium", &med_sel), ("high", &high_sel)] {
            group.bench_function(
                BenchmarkId::new(format!("filter_{}", sel_name), chunk_size),
                |b| {
                    b.iter_batched(
                        || create_chunk(*chunk_size),
                        |mut chunk| {
                            let results = chunk.evaluate_expression(pred, None).unwrap();
                            let selected: Vec<usize> = results
                                .into_iter()
                                .enumerate()
                                .filter_map(|(i, v)| matches!(&v, Value::Bool(true)).then_some(i))
                                .collect();
                            let _ = chunk.take_indices(&selected);
                        },
                        BatchSize::SmallInput,
                    )
                },
            );
        }
    }

    group.finish();
}

fn bench_materialize_columns(c: &mut Criterion) {
    let mut group = c.benchmark_group("column_materialize");

    for chunk_size in &[128usize, 512, 1024, 4096] {
        group.bench_function(BenchmarkId::new("materialize", chunk_size), |b| {
            b.iter_batched(
                || create_chunk(*chunk_size),
                |mut chunk| {
                    chunk.build_typed_columns(true);
                },
                BatchSize::SmallInput,
            )
        });

        group.bench_function(BenchmarkId::new("get_column", chunk_size), |b| {
            b.iter_batched(
                || {
                    let mut chunk = create_chunk(*chunk_size);
                    chunk.build_typed_columns(true);
                    chunk
                },
                |mut chunk| {
                    let _ = chunk.get_column(0);
                    let _ = chunk.get_column(1);
                    let _ = chunk.get_column(2);
                },
                BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

// ── accumulation paths ──────────────────────────────────────────────────────

const ROW_SIZES: [usize; 2] = [100_000, 1_000_000];

/// Current operator pattern (hash_join.rs build loop): key from materialized
/// columns, row cloned twice (bucket + all_right_rows).
fn hash_join_build_rows(chunk: &mut DataChunk) -> usize {
    let cols = materialize_columns(chunk);
    let mut build_side_hash: HashMap<JoinKeyValue, Vec<Vec<Value>>> = HashMap::new();
    let mut all_right_rows: Vec<Vec<Value>> = Vec::new();
    for (row_idx, row) in chunk.rows.iter().enumerate() {
        let key = JoinKeyValue::from(cols[0][row_idx].clone());
        build_side_hash.entry(key).or_default().push(row.clone());
        all_right_rows.push(row.clone());
    }
    build_side_hash.len() + all_right_rows.len()
}

/// Columnar candidate mirroring `HashJoinBuildSide::insert_chunk`: columns are
/// materialized, the key → row index map is built, and the materialized
/// columns are moved into the accumulation store (extended across chunks).
/// The target is pre-seeded with one prior chunk so the measured path is the
/// cross-chunk `extend` (per-value copy), not just the first-chunk move.
fn hash_join_build_columns(
    chunk: &mut DataChunk,
    target: &mut Vec<Vec<Value>>,
    base: usize,
) -> usize {
    let cols = materialize_columns(chunk);
    let mut build_index: HashMap<JoinKeyValue, Vec<u32>> = HashMap::new();
    for (row_idx, key) in cols[0].iter().enumerate() {
        let key = JoinKeyValue::from(key.clone());
        build_index
            .entry(key)
            .or_default()
            .push((base + row_idx) as u32);
    }
    if target.is_empty() {
        *target = cols;
    } else {
        for (t, s) in target.iter_mut().zip(cols) {
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
    let target = materialize_columns(&mut first);
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
                |(mut chunk, mut target)| {
                    black_box(hash_join_build_columns(&mut chunk, &mut target, *size))
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

/// Current in-memory path (blocking.rs): collect all_rows, group rows by key
/// with per-group row storage, then rescan each group for the aggregate.
fn group_by_rows(chunk: &mut DataChunk, key_cols: &[usize], value_col: usize) -> (usize, f64) {
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
            vec![
                AggregateAccumulator::Sum(0.0),
                AggregateAccumulator::Count(0),
            ]
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

    for (key_name, key_cols) in [
        ("1_key", vec![2usize]),
        ("3_key", vec![2usize, 3usize, 1usize]),
    ] {
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

/// Baseline: columnar input (storage batch output) transposed into rows,
/// then grouped via per-group row collection (current group path).
fn scan_group_transpose(chunk: &mut DataChunk, key_col: usize, value_col: usize) -> (usize, f64) {
    let cols = materialize_columns(chunk);
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
    let cols = materialize_columns(chunk);
    let mut acc_map: HashMap<Value, AggregateAccumulator> = HashMap::new();
    let mut sum = 0.0;
    for (row_idx, _) in cols[0].iter().enumerate() {
        let key = cols[key_col][row_idx].clone();
        let acc = acc_map
            .entry(key)
            .or_insert_with(|| AggregateAccumulator::Sum(0.0));
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

// ── columnar necessity probes ───────────────────────────────────────────────

/// Mixed-kind numeric expressions: batch numeric promotion (typed columns)
/// vs. per-row Value evaluation (no typed columns).
///
/// Before the promotion work, every mixed I32/I64/F64 expression fell back to
/// the per-row Value path; this group quantifies the throughput of the batch
/// paths (`numeric_i64_view` / `numeric_f64_view` promotion in `typed.rs`).
fn bench_numeric_promotion(c: &mut Criterion) {
    let mut group = c.benchmark_group("numeric_promotion");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(30);

    // BigInt column + Int(32) literal: promoted to i64.
    let mixed_add = Expression::Binary {
        left: Box::new(Expression::Variable("k0".into())),
        op: BinaryOperator::Add,
        right: Box::new(Expression::Literal(Value::Int(7))),
    };
    let mixed_cmp = Expression::Binary {
        left: Box::new(Expression::Variable("k0".into())),
        op: BinaryOperator::LessThan,
        right: Box::new(Expression::Literal(Value::Int(500))),
    };
    // Int(32) column + Double literal: promoted to f64.
    let int_double_add = Expression::Binary {
        left: Box::new(Expression::Variable("k0".into())),
        op: BinaryOperator::Add,
        right: Box::new(Expression::Literal(Value::Double(0.5))),
    };

    let i32_layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
    let make_i32_chunk = |size: usize| {
        let rows: Vec<Vec<Value>> = (0..size)
            .map(|i| vec![Value::Int((i % 1000) as i32)])
            .collect();
        DataChunk::new_with_layout(rows, i32_layout.clone())
    };

    for n in [4096usize, 65536] {
        group.bench_function(BenchmarkId::new("batch_mixed_add", n), |b| {
            b.iter_batched(
                || {
                    let mut chunk = create_wide_chunk(n);
                    chunk.build_typed_columns(true);
                    chunk
                },
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&mixed_add, None);
                },
                criterion::BatchSize::SmallInput,
            )
        });
        group.bench_function(BenchmarkId::new("per_row_mixed_add", n), |b| {
            b.iter_batched(
                || create_wide_chunk(n),
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&mixed_add, None);
                },
                criterion::BatchSize::SmallInput,
            )
        });

        group.bench_function(BenchmarkId::new("batch_mixed_cmp", n), |b| {
            b.iter_batched(
                || {
                    let mut chunk = create_wide_chunk(n);
                    chunk.build_typed_columns(true);
                    chunk
                },
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&mixed_cmp, None);
                },
                criterion::BatchSize::SmallInput,
            )
        });
        group.bench_function(BenchmarkId::new("per_row_mixed_cmp", n), |b| {
            b.iter_batched(
                || create_wide_chunk(n),
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&mixed_cmp, None);
                },
                criterion::BatchSize::SmallInput,
            )
        });

        group.bench_function(BenchmarkId::new("batch_int_double_add", n), |b| {
            b.iter_batched(
                || {
                    let mut chunk = make_i32_chunk(n);
                    chunk.build_typed_columns(true);
                    chunk
                },
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&int_double_add, None);
                },
                criterion::BatchSize::SmallInput,
            )
        });
        group.bench_function(BenchmarkId::new("per_row_int_double_add", n), |b| {
            b.iter_batched(
                || make_i32_chunk(n),
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&int_double_add, None);
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

fn bench_row_vs_column_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("row_vs_column_filter");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(30);

    for n in [64usize, 1024, 16384, 262144] {
        let rows: Vec<Value> = (0..n).map(|i| Value::BigInt((i % 1000) as i64)).collect();
        group.bench_function(BenchmarkId::new("row_value", n), |b| {
            b.iter(|| {
                let mut count = 0usize;
                for v in &rows {
                    if let Value::BigInt(x) = v {
                        if *x > 500 {
                            count += 1;
                        }
                    }
                }
                black_box(count);
            })
        });

        let cols: Vec<i64> = (0..n).map(|i| (i % 1000) as i64).collect();
        group.bench_function(BenchmarkId::new("column_i64", n), |b| {
            b.iter(|| {
                let mut count = 0usize;
                for x in &cols {
                    if *x > 500 {
                        count += 1;
                    }
                }
                black_box(count);
            })
        });
    }
    group.finish();
}

fn bench_wide_single_column_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("wide_single_column_filter");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(30);

    for n in [1024usize, 16384, 262144] {
        group.bench_function(BenchmarkId::new("full_row_scan", n), |b| {
            b.iter_batched(
                || create_wide_chunk(n),
                |mut chunk| {
                    let _ = chunk.evaluate_expression(
                        &Expression::Binary {
                            left: Box::new(Expression::Variable("k0".into())),
                            op: BinaryOperator::GreaterThan,
                            right: Box::new(Expression::Literal(Value::BigInt(500))),
                        },
                        None,
                    );
                },
                criterion::BatchSize::SmallInput,
            )
        });

        let cols: Vec<i64> = (0..n).map(|i| (i % 1000) as i64).collect();
        group.bench_function(BenchmarkId::new("column_pruned", n), |b| {
            b.iter(|| {
                let mut count = 0usize;
                for x in &cols {
                    if *x > 500 {
                        count += 1;
                    }
                }
                black_box(count);
            })
        });
    }
    group.finish();
}

/// Real DataChunk filter over the typed column layout.
///
/// Builds a chunk exactly as source operators do (rows + `build_typed_columns`)
/// and evaluates a single-column predicate through `evaluate_expression`,
/// comparing the typed batch path against the row-major path.
fn bench_typed_data_chunk_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("typed_data_chunk_filter");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(30);

    let predicate = Expression::Binary {
        left: Box::new(Expression::Variable("k0".into())),
        op: BinaryOperator::GreaterThan,
        right: Box::new(Expression::Literal(Value::BigInt(500))),
    };

    for n in [4096usize, 16384, 65536] {
        group.bench_function(BenchmarkId::new("typed_chunk", n), |b| {
            b.iter_batched(
                || {
                    let mut chunk = create_wide_chunk(n);
                    chunk.build_typed_columns(true);
                    chunk
                },
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&predicate, None);
                },
                criterion::BatchSize::SmallInput,
            )
        });
        group.bench_function(BenchmarkId::new("row_chunk", n), |b| {
            b.iter_batched(
                || create_wide_chunk(n),
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&predicate, None);
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

/// End-to-end Filter→Project chain: materialized (`take_indices`) vs.
/// selection-vector propagation (1% selectivity).
fn bench_selection_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("selection_propagation_chain");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(30);

    let n = 16384usize;
    let predicate = Expression::Binary {
        left: Box::new(Expression::Variable("k0".into())),
        op: BinaryOperator::LessThan,
        right: Box::new(Expression::Literal(Value::BigInt(163))), // ~1% of 0..16384
    };
    let project = Expression::Variable("k1".into());

    group.bench_function("materialized_chain", |b| {
        b.iter_batched(
            || {
                let mut chunk = create_wide_chunk(n);
                chunk.build_typed_columns(true);
                chunk
            },
            |mut chunk| {
                let results = chunk.evaluate_expression(&predicate, None).unwrap();
                let selected: Vec<usize> = results
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| {
                        if matches!(v, Value::Bool(true)) {
                            Some(i)
                        } else {
                            None
                        }
                    })
                    .collect();
                let mut filtered = chunk.take_indices(&selected);
                let _ = filtered.evaluate_expression(&project, None).unwrap();
                black_box(filtered.len());
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.bench_function("selection_chain", |b| {
        b.iter_batched(
            || {
                let mut chunk = create_wide_chunk(n);
                chunk.build_typed_columns(true);
                chunk
            },
            |mut chunk| {
                let results = chunk.evaluate_expression(&predicate, None).unwrap();
                let selected: Vec<usize> = results
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| {
                        if matches!(v, Value::Bool(true)) {
                            Some(i)
                        } else {
                            None
                        }
                    })
                    .collect();
                // Selection vector travels downstream without row moves;
                // the next operator reads only the visible rows.
                let chunk = chunk.with_selection(selected);
                let slot = chunk.get_layout().slot_id("k1").expect("k1 slot");
                let mut acc = 0usize;
                for idx in chunk.visible_indices() {
                    if let Some(Value::BigInt(v)) = chunk.get_typed_by_slot(idx, slot) {
                        acc = acc.wrapping_add(v as usize);
                    }
                }
                black_box(acc);
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_null_bitmap(c: &mut Criterion) {
    let mut group = c.benchmark_group("null_bitmap");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(30);

    let n = 262144usize;
    for null_rate in [0.0f64, 0.01, 0.3, 0.8] {
        let options: Vec<Option<i64>> = (0..n)
            .map(|i| {
                if (i as f64) / (n as f64) < null_rate {
                    None
                } else {
                    Some((i % 1000) as i64)
                }
            })
            .collect();
        group.bench_function(BenchmarkId::new("option_enum", null_rate), |b| {
            b.iter(|| {
                let mut count = 0usize;
                for x in options.iter().flatten() {
                    if *x > 500 {
                        count += 1;
                    }
                }
                black_box(count);
            })
        });

        let values: Vec<i64> = (0..n)
            .map(|i| {
                if (i as f64) / (n as f64) < null_rate {
                    0
                } else {
                    (i % 1000) as i64
                }
            })
            .collect();
        let bits: Vec<u64> = (0..n)
            .map(|i| {
                if (i as f64) / (n as f64) < null_rate {
                    0
                } else {
                    1
                }
            })
            .collect();
        group.bench_function(BenchmarkId::new("bitmap_2vec", null_rate), |b| {
            b.iter(|| {
                let mut count = 0usize;
                for (i, x) in values.iter().enumerate() {
                    if bits[i / 64] & (1u64 << (i % 64)) != 0 && *x > 500 {
                        count += 1;
                    }
                }
                black_box(count);
            })
        });
    }
    group.finish();
}

#[inline(never)]
fn filter_scalar(values: &[i64]) -> usize {
    let mut count = 0usize;
    for x in values {
        if *x > 500 {
            count += 1;
        }
    }
    count
}

#[inline(never)]
fn filter_unrolled4(values: &[i64]) -> usize {
    let mut count = 0usize;
    let chunks = values.as_chunks::<4>();
    for c in chunks.0 {
        if c[0] > 500 {
            count += 1;
        }
        if c[1] > 500 {
            count += 1;
        }
        if c[2] > 500 {
            count += 1;
        }
        if c[3] > 500 {
            count += 1;
        }
    }
    for x in chunks.1 {
        if *x > 500 {
            count += 1;
        }
    }
    count
}

fn bench_autovectorization(c: &mut Criterion) {
    let mut group = c.benchmark_group("autovectorization");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(30);

    let n = 262144usize;
    let cols: Vec<i64> = (0..n).map(|i| (i % 1000) as i64).collect();
    group.bench_function("scalar", |b| b.iter(|| black_box(filter_scalar(&cols))));
    group.bench_function("unrolled4", |b| {
        b.iter(|| black_box(filter_unrolled4(&cols)))
    });
    group.finish();
}

fn bench_selectivity_propagation(c: &mut Criterion) {
    let mut group = c.benchmark_group("selectivity_propagation");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(30);

    let n = 16384usize;
    for selectivity in [0.01f64, 0.1, 0.5] {
        let pass_count = (n as f64 * selectivity) as usize;
        let indices: Vec<usize> = (0..pass_count).collect();

        group.bench_function(
            BenchmarkId::new("take_indices_materialize", selectivity),
            |b| {
                b.iter_batched(
                    || create_wide_chunk(n),
                    |mut chunk| {
                        let out = chunk.take_indices(&indices);
                        black_box(out.len());
                    },
                    criterion::BatchSize::SmallInput,
                )
            },
        );

        let rows: Vec<Vec<Value>> = (0..n)
            .map(|i| vec![Value::BigInt((i % 1000) as i64)])
            .collect();
        group.bench_function(BenchmarkId::new("indices_passthrough", selectivity), |b| {
            b.iter(|| {
                let mut acc = 0usize;
                for &idx in &indices {
                    if let Value::BigInt(x) = rows[idx][0] {
                        acc = acc.wrapping_add(x as usize);
                    }
                }
                black_box(acc);
            })
        });
    }
    group.finish();
}

/// NULL-bearing typed columns vs Fallback: does the validity bitmap keep
/// low-NULL-density columns on the typed fast path (Q3 follow-up)?
fn bench_nullable_typed_column_filter(c: &mut Criterion) {
    use linkrs_core::value::NullType;

    let mut group = c.benchmark_group("nullable_typed_column_filter");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(30);

    let n = 262144usize;
    let predicate = Expression::Binary {
        left: Box::new(Expression::Variable("k0".into())),
        op: BinaryOperator::GreaterThan,
        right: Box::new(Expression::Literal(Value::BigInt(500))),
    };

    for null_rate in [0.0f64, 0.01, 0.1, 0.3, 0.8] {
        let make_chunk = |typed: bool| {
            let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
            let rows: Vec<Vec<Value>> = (0..n)
                .map(|i| {
                    if (i as f64) / (n as f64) < null_rate {
                        vec![Value::Null(NullType::Null)]
                    } else {
                        vec![Value::BigInt((i % 1000) as i64)]
                    }
                })
                .collect();
            let mut chunk = DataChunk::new_with_layout(rows, layout);
            chunk.build_typed_columns(typed);
            chunk
        };

        group.bench_function(BenchmarkId::new("nullable_typed", null_rate), |b| {
            b.iter_batched(
                || make_chunk(true),
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&predicate, None);
                },
                criterion::BatchSize::SmallInput,
            )
        });

        group.bench_function(BenchmarkId::new("fallback", null_rate), |b| {
            b.iter_batched(
                || make_chunk(false),
                |mut chunk| {
                    let _ = chunk.evaluate_expression(&predicate, None);
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_expression_eval,
    bench_filter_throughput,
    bench_materialize_columns,
    bench_hash_join_build,
    bench_group_by,
    bench_scan_group,
    bench_row_vs_column_filter,
    bench_wide_single_column_filter,
    bench_typed_data_chunk_filter,
    bench_selection_chain,
    bench_null_bitmap,
    bench_autovectorization,
    bench_selectivity_propagation,
    bench_numeric_promotion,
    bench_nullable_typed_column_filter,
);
criterion_main!(benches);
