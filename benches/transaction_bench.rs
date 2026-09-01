use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use graphdb::core::types::VertexId;
use graphdb::transaction::manager::TransactionManager;
use graphdb::transaction::mvcc::VersionManager;
use graphdb::transaction::types::*;

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
                    ws.record_vertex(VertexId::from_int64(i as i64));
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
                    ws1.record_vertex(VertexId::from_int64(i as i64));
                    ws2.record_vertex(VertexId::from_int64((i + size) as i64));
                }
                ws2.record_vertex(VertexId::from_int64(0));
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
            vm.commit_write_timestamp(ts);
            black_box(ts);
        });
    });

    group.finish();
}

fn bench_conflict_detection(c: &mut Criterion) {
    let manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    let mut group = c.benchmark_group("conflict_detection");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);
    group.warm_up_time(Duration::from_secs(1));

    for vertex_count in &[1, 10, 100] {
        group.bench_with_input(
            BenchmarkId::from_parameter(vertex_count),
            vertex_count,
            |b, &count| {
                let mgr = Arc::clone(&manager);
                b.iter(|| {
                    let txn_a = mgr
                        .begin_insert_transaction(TransactionOptions::default())
                        .unwrap();
                    {
                        let ctx = mgr.get_context(txn_a).unwrap();
                        for i in 0..count {
                            ctx.record_vertex_write(VertexId::from_int64(i as i64));
                        }
                    }
                    let _ = mgr.check_write_set_conflict(txn_a);
                    let txn_b = mgr
                        .begin_insert_transaction(TransactionOptions::default())
                        .unwrap();
                    {
                        let ctx = mgr.get_context(txn_b).unwrap();
                        for i in 0..count {
                            ctx.record_vertex_write(VertexId::from_int64((i + count) as i64));
                        }
                    }
                    let _ = black_box(mgr.check_write_set_conflict(txn_b));
                    mgr.abort_transaction(txn_a).unwrap();
                    mgr.abort_transaction(txn_b).unwrap();
                });
            },
        );
    }

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
        cfg.txn_config.concurrency_mode =
            graphdb::transaction::types::ConcurrencyMode::SingleWriter;
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
    use graphdb::transaction::SnapshotTracker;
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
    bench_conflict_detection,
    bench_certification_fast_paths,
    bench_snapshot_tracker,
);
criterion_main!(benches);
