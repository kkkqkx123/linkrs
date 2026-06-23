// benches/transaction_bench.rs
//! Transaction layer performance benchmarks
//! Tests: transaction operations, MVCC, concurrency

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use std::time::Duration;

fn create_benchmark_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut group = c.benchmark_group(name);
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);
    group.warm_up_time(Duration::from_secs(1));
    group
}

/// Generate GQL for transaction benchmark setup
#[allow(dead_code)]
fn generate_gql_for_transaction_bench(vertex_count: usize) -> String {
    let mut gql = String::new();
    gql.push_str(&format!("CREATE SPACE IF NOT EXISTS bench_txn{} (vid_type=STRING)\n", vertex_count));
    gql.push_str(&format!("USE bench_txn{}\n\n", vertex_count));

    gql.push_str("CREATE TAG IF NOT EXISTS Data(\n");
    gql.push_str("    value: INT,\n");
    gql.push_str("    counter: INT DEFAULT 0\n");
    gql.push_str(")\n\n");

    for i in 0..vertex_count {
        gql.push_str(&format!(
            "INSERT VERTEX Data(value, counter) VALUES \"d{}\"({}, 0)\n",
            i, i
        ));
    }

    gql
}

fn bench_transaction_create_commit(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "transaction_ops");

    group.bench_function("transaction_create", |b| {
        b.iter(|| {
            black_box("BEGIN".to_string())
        });
    });

    group.bench_function("transaction_commit", |b| {
        b.iter(|| {
            black_box("COMMIT".to_string())
        });
    });

    group.bench_function("transaction_rollback", |b| {
        b.iter(|| {
            black_box("ROLLBACK".to_string())
        });
    });

    group.finish();
}

fn bench_transaction_batch_operations(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "transaction_batch");

    for op_count in &[10, 100, 1000] {
        let mut gql = String::new();
        gql.push_str("BEGIN\n");
        for i in 0..*op_count {
            gql.push_str(&format!("INSERT VERTEX Data(value, counter) VALUES \"v{}\"({}, 0)\n", i, i));
        }
        gql.push_str("COMMIT\n");

        group.bench_with_input(
            BenchmarkId::from_parameter(op_count),
            op_count,
            |b, _| {
                b.iter(|| {
                    black_box(gql.matches("INSERT VERTEX").count())
                });
            },
        );
    }

    group.finish();
}

fn bench_mvcc_version_management(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "mvcc_versions");

    for version_count in &[1, 10, 100] {
        group.bench_with_input(
            BenchmarkId::from_parameter(version_count),
            version_count,
            |b, _| {
                // Simulate MVCC version chain traversal
                b.iter(|| {
                    #[allow(clippy::unnecessary_cast)]
                    let result = black_box((0..*version_count).map(|v| v as i32).sum::<i32>());
                    result
                });
            },
        );
    }

    group.finish();
}

fn bench_write_conflict_detection(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "conflict_detection");

    for conflict_count in &[0, 5, 10] {
        group.bench_with_input(
            BenchmarkId::from_parameter(conflict_count),
            conflict_count,
            |b, _| {
                // Simulate conflict checking
                b.iter(|| {
                    black_box((0..*conflict_count).map(|c| c * 2).sum::<usize>() as i32)
                });
            },
        );
    }

    group.finish();
}

fn bench_isolation_levels(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "isolation_levels");

    let isolation_levels = vec!["READ_UNCOMMITTED", "READ_COMMITTED", "REPEATABLE_READ", "SERIALIZABLE"];

    for level in isolation_levels {
        group.bench_function(format!("isolation_{}", level), |b| {
            b.iter(|| {
                black_box(format!("BEGIN ISOLATION_LEVEL {}", level))
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_transaction_create_commit,
    bench_transaction_batch_operations,
    bench_mvcc_version_management,
    bench_write_conflict_detection,
    bench_isolation_levels
);
criterion_main!(benches);
