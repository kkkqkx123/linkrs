use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use linkrs_core::types::VertexId;
use linkrs_transaction::manager::TransactionManager;
use linkrs_transaction::mvcc::VersionManager;
use linkrs_transaction::types::*;

fn bench_transaction_create_commit(c: &mut Criterion) {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let mut group = c.benchmark_group("transaction_ops");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);
    group.warm_up_time(Duration::from_secs(1));

    group.bench_function("begin_read", |b| {
        b.iter(|| {
            let txn = manager
                .begin_read_transaction(TransactionOptions::default())
                .unwrap();
            manager.commit_transaction(txn).unwrap();
            black_box(txn);
        });
    });

    group.bench_function("begin_write", |b| {
        b.iter(|| {
            let txn = manager
                .begin_insert_transaction(TransactionOptions::default())
                .unwrap();
            manager.commit_transaction(txn).unwrap();
            black_box(txn);
        });
    });

    group.finish();
}

fn bench_write_set_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("write_set");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);
    group.warm_up_time(Duration::from_secs(1));

    for size in &[10, 100, 1000] {
        group.bench_with_input(BenchmarkId::new("build", size), size, |b, &size| {
            b.iter(|| {
                let mut ws = WriteSet::new();
                for i in 0..size {
                    ws.record_vertex(VertexId::try_from_int64(i as i64).expect("valid vertex id"));
                }
                black_box(ws);
            });
        });
    }

    for size in &[10, 100, 1000] {
        group.bench_with_input(
            BenchmarkId::new("conflict_check", size),
            size,
            |b, &size| {
                let mut ws1 = WriteSet::new();
                let mut ws2 = WriteSet::new();
                for i in 0..size {
                    ws1.record_vertex(VertexId::try_from_int64(i as i64).expect("valid vertex id"));
                    ws2.record_vertex(
                        VertexId::try_from_int64((i + size) as i64).expect("valid vertex id"),
                    );
                }
                ws2.record_vertex(VertexId::try_from_int64(0).expect("valid vertex id"));
                b.iter(|| {
                    black_box(ws1.has_conflict_with(&ws2));
                });
            },
        );
    }

    group.finish();
}

fn bench_mvcc_version_management(c: &mut Criterion) {
    let vm = VersionManager::new();

    let mut group = c.benchmark_group("mvcc");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);
    group.warm_up_time(Duration::from_secs(1));

    group.bench_function("acquire_read_ts", |b| {
        b.iter(|| {
            let ts = vm.acquire_read_timestamp().unwrap();
            vm.release_read_timestamp_at(ts);
            black_box(ts);
        });
    });

    group.bench_function("acquire_write_ts", |b| {
        b.iter(|| {
            let ts = vm.acquire_insert_timestamp().unwrap();
            vm.commit_ordered(ts).expect("ordered commit");
            black_box(ts);
        });
    });

    group.finish();
}

fn bench_certification_fast_paths(c: &mut Criterion) {
    let mut group = c.benchmark_group("certification_fast_paths");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(100);

    let manager = TransactionManager::new(TransactionManagerConfig::default());

    group.bench_function("certification_read_only", |b| {
        b.iter(|| {
            let txn_id = manager
                .begin_read_transaction(TransactionOptions::default())
                .unwrap();
            manager.check_write_set_conflict(txn_id).unwrap();
            black_box(());
            manager.commit_transaction(txn_id).unwrap();
        });
    });

    group.bench_function("certification_single_writer", |b| {
        let mut cfg = TransactionManagerConfig::default();
        cfg.txn_config.concurrency_mode = linkrs_transaction::types::ConcurrencyMode::SingleWriter;
        let sw_manager = TransactionManager::new(cfg);
        b.iter(|| {
            let txn_id = sw_manager
                .begin_insert_transaction(TransactionOptions::default())
                .unwrap();
            sw_manager.check_write_set_conflict(txn_id).unwrap();
            black_box(());
            sw_manager.commit_transaction(txn_id).unwrap();
        });
    });

    group.bench_function("certification_empty_write_set", |b| {
        b.iter(|| {
            let txn_id = manager
                .begin_insert_transaction(TransactionOptions::default())
                .unwrap();
            manager.check_write_set_conflict(txn_id).unwrap();
            black_box(());
            manager.commit_transaction(txn_id).unwrap();
        });
    });

    group.finish();
}

fn bench_snapshot_tracker(c: &mut Criterion) {
    use linkrs_transaction::SnapshotTracker;
    let mut group = c.benchmark_group("snapshot_tracker");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(100);

    group.bench_function("min_active_snapshot", |b| {
        let tracker = SnapshotTracker::new();
        for i in 0..100 {
            tracker.add_snapshot(i * 10).unwrap();
        }
        b.iter(|| black_box(tracker.min_active_snapshot()));
    });

    group.bench_function("contains_fast_negative", |b| {
        let tracker = SnapshotTracker::new();
        for i in 0..100 {
            tracker.add_snapshot(i * 10).unwrap();
        }
        b.iter(|| black_box(tracker.contains_snapshot(999999)));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_transaction_create_commit,
    bench_write_set_operations,
    bench_mvcc_version_management,
    bench_certification_fast_paths,
    bench_snapshot_tracker,
);
criterion_main!(benches);
