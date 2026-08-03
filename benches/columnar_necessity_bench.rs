//! Necessity experiments for the columnar optimization family.
//!
//! Each benchmark group answers one question from
//! `docs/plan/columnar-necessity-verification-design.md`:
//! - row_vs_column_filter: is a typed column layout faster than Vec<Value> for
//!   single-column predicates? (Q1: typed columnar chunk)
//! - wide_single_column_filter: does column pruning beat full-row scans on wide
//!   tables (cache behavior)? (Q1 / Q6)
//! - null_bitmap: how does a validity bitmap compare to Option-style encoding
//!   across null densities? (Q3)
//! - autovectorization: scalar vs unrolled loops under `-C target-cpu=native`
//!   (Q2: SIMD headroom probe)
//! - selectivity_propagation: take_indices materialization vs index pass-through
//!   across operator boundaries (Q4)
//!
//! Run twice for the SIMD probe:
//!   cargo bench --bench columnar_necessity_bench
//!   RUSTFLAGS="-C target-cpu=native" cargo bench --bench columnar_necessity_bench

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use graphdb_query::core::Value;
use graphdb_query::query::executor::streaming::chunk::DataChunk;
use graphdb_query::query::executor::streaming::slot::SlotLayout;
use std::sync::Arc;
use std::time::Duration;

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
                        &graphdb_query::core::types::expr::Expression::Binary {
                            left: Box::new(
                                graphdb_query::core::types::expr::Expression::Variable("k0".into()),
                            ),
                            op: graphdb_query::core::types::operators::BinaryOperator::GreaterThan,
                            right: Box::new(
                                graphdb_query::core::types::expr::Expression::Literal(
                                    Value::BigInt(500),
                                ),
                            ),
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
        let bits: Vec<u64> = (0..n).map(|i| if (i as f64) / (n as f64) < null_rate { 0 } else { 1 }).collect();
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
    let mut chunks = values.chunks_exact(4);
    for c in &mut chunks {
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
    for x in chunks.remainder() {
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
    group.bench_function("unrolled4", |b| b.iter(|| black_box(filter_unrolled4(&cols))));
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

        group.bench_function(BenchmarkId::new("take_indices_materialize", selectivity), |b| {
            b.iter_batched(
                || create_wide_chunk(n),
                |mut chunk| {
                    let out = chunk.take_indices(&indices);
                    black_box(out.len());
                },
                criterion::BatchSize::SmallInput,
            )
        });

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

criterion_group!(
    benches,
    bench_row_vs_column_filter,
    bench_wide_single_column_filter,
    bench_null_bitmap,
    bench_autovectorization,
    bench_selectivity_propagation,
);
criterion_main!(benches);
