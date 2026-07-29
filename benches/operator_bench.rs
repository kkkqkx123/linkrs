use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use graphdb_query::core::types::expr::Expression;
use graphdb_query::core::types::operators::BinaryOperator;
use graphdb_query::core::Value;
use graphdb_query::query::executor::streaming::chunk::DataChunk;
use graphdb_query::query::executor::streaming::slot::SlotLayout;
use std::sync::Arc;

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
    DataChunk::new_with_layout(rows, layout)
}

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

        group.bench_function(
            BenchmarkId::new("simple_predicate", chunk_size),
            |b| {
                b.iter_batched(
                    || create_chunk(*chunk_size),
                    |chunk| {
                        let _ = chunk.evaluate_expression(&simple_pred, None);
                    },
                    BatchSize::SmallInput,
                )
            },
        );

        group.bench_function(
            BenchmarkId::new("compound_predicate", chunk_size),
            |b| {
                b.iter_batched(
                    || create_chunk(*chunk_size),
                    |chunk| {
                        let _ = chunk.evaluate_expression(&compound_pred, None);
                    },
                    BatchSize::SmallInput,
                )
            },
        );

        group.bench_function(
            BenchmarkId::new("project_single", chunk_size),
            |b| {
                b.iter_batched(
                    || create_chunk(*chunk_size),
                    |chunk| {
                        let _ = chunk.evaluate_expression(&project_single[0], None);
                    },
                    BatchSize::SmallInput,
                )
            },
        );

        group.bench_function(
            BenchmarkId::new("project_multi_each", chunk_size),
            |b| {
                b.iter_batched(
                    || create_chunk(*chunk_size),
                    |chunk| {
                        for expr in &project_multi {
                            let _ = chunk.evaluate_expression(expr, None);
                        }
                    },
                    BatchSize::SmallInput,
                )
            },
        );

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
                            let results = chunk
                                .evaluate_expression(pred, None)
                                .unwrap();
                            let selected: Vec<usize> = results
                                .into_iter()
                                .enumerate()
                                .filter_map(|(i, v)| {
                                    matches!(&v, Value::Bool(true)).then_some(i)
                                })
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
                    chunk.materialize_columns();
                },
                BatchSize::SmallInput,
            )
        });

        group.bench_function(BenchmarkId::new("get_column", chunk_size), |b| {
            b.iter_batched(
                || {
                    let mut chunk = create_chunk(*chunk_size);
                    chunk.materialize_columns();
                    chunk
                },
                |chunk| {
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

criterion_group!(
    benches,
    bench_expression_eval,
    bench_filter_throughput,
    bench_materialize_columns,
);
criterion_main!(benches);
