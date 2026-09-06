//! Necessity experiments for the columnar optimization family.
//!
//! Each benchmark group answers one question about the columnar
//! optimization family:
//! - row_vs_column_filter: is a typed column layout faster than Vec<Value> for
//!   single-column predicates? (typed columnar chunk)
//! - wide_single_column_filter: does column pruning beat full-row scans on wide
//!   tables (cache behavior)?
//! - null_bitmap: how does a validity bitmap compare to Option-style encoding
//!   across null densities?
//! - autovectorization: scalar vs unrolled loops under `-C target-cpu=native`
//!   (SIMD headroom probe)
//! - selectivity_propagation: take_indices materialization vs index pass-through
//!   across operator boundaries
//!
//! Run twice for the SIMD probe:
//!   cargo bench --bench columnar_necessity_bench
//!   RUSTFLAGS="-C target-cpu=native" cargo bench --bench columnar_necessity_bench

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use graphdb_core::Value;
use graphdb_query::executor::streaming::chunk::DataChunk;
use graphdb_query::executor::streaming::slot::SlotLayout;
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

/// Mixed-kind numeric expressions: batch numeric promotion (typed columns)
/// vs. per-row Value evaluation (no typed columns).
///
/// Before the promotion work, every mixed I32/I64/F64 expression fell back to
/// the per-row Value path; this group quantifies the throughput of the batch
/// paths (`numeric_i64_view` / `numeric_f64_view` promotion in `typed.rs`).
fn bench_numeric_promotion(c: &mut Criterion) {
    use graphdb_core::types::expr::Expression;
    use graphdb_core::types::operators::BinaryOperator;

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
                        &graphdb_core::types::expr::Expression::Binary {
                            left: Box::new(graphdb_core::types::expr::Expression::Variable(
                                "k0".into(),
                            )),
                            op: graphdb_core::types::operators::BinaryOperator::GreaterThan,
                            right: Box::new(graphdb_core::types::expr::Expression::Literal(
                                Value::BigInt(500),
                            )),
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

    let predicate = graphdb_core::types::expr::Expression::Binary {
        left: Box::new(graphdb_core::types::expr::Expression::Variable("k0".into())),
        op: graphdb_core::types::operators::BinaryOperator::GreaterThan,
        right: Box::new(graphdb_core::types::expr::Expression::Literal(
            Value::BigInt(500),
        )),
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
    use graphdb_core::types::expr::Expression;

    let mut group = c.benchmark_group("selection_propagation_chain");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(30);

    let n = 16384usize;
    let predicate = Expression::Binary {
        left: Box::new(Expression::Variable("k0".into())),
        op: graphdb_core::types::operators::BinaryOperator::LessThan,
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
    use graphdb_core::types::expr::Expression;
    use graphdb_core::types::operators::BinaryOperator;
    use graphdb_core::value::NullType;

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
    bench_row_vs_column_filter,
    bench_wide_single_column_filter,
    bench_null_bitmap,
    bench_autovectorization,
    bench_selectivity_propagation,
    bench_typed_data_chunk_filter,
    bench_selection_chain,
    bench_numeric_promotion,
    bench_nullable_typed_column_filter,
);
criterion_main!(benches);
