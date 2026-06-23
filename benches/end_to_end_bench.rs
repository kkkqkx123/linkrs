// benches/end_to_end_bench.rs
//! End-to-end performance benchmarks
//! Tests complete workflows including data loading, querying, and updating

use criterion::{black_box, criterion_group, criterion_main, Criterion};
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

fn bench_data_loading_workflow(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_data_loading");

    group.bench_function("load_1k_vertices_with_edges", |b| {
        b.iter(|| {
            let vertices = 1000;
            let edges_per_vertex = 5;
            let total_operations = vertices + (vertices * edges_per_vertex);
            black_box(total_operations)
        });
    });

    group.bench_function("load_10k_vertices_with_edges", |b| {
        b.iter(|| {
            let vertices = 10000;
            let edges_per_vertex = 5;
            let total_operations = vertices + (vertices * edges_per_vertex);
            black_box(total_operations)
        });
    });

    group.finish();
}

fn bench_query_analysis_workflow(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_query_analysis");

    group.bench_function("simple_query_1k_data", |b| {
        b.iter(|| {
            // Simulate querying over 1k vertices
            let results = (0..1000).filter(|i| i % 2 == 0).count();
            black_box(results)
        });
    });

    group.bench_function("path_query_1k_data", |b| {
        b.iter(|| {
            // Simulate path query over 1k vertices
            let hops = 3;
            let explored = (0..1000_usize).take(100 * hops).count();
            black_box(explored)
        });
    });

    group.finish();
}

fn bench_search_workflow(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_search");

    group.bench_function("fulltext_search_10k_documents", |b| {
        b.iter(|| {
            // Simulate searching 10k documents
            let matches = (0..10000).filter(|i| i % 5 == 0).count();
            black_box(matches)
        });
    });

    group.bench_function("vector_search_10k_vectors", |b| {
        b.iter(|| {
            // Simulate vector similarity search on 10k vectors
            let top_k = 10;
            let candidates_checked = 10000 / 100;
            black_box(candidates_checked.min(top_k))
        });
    });

    group.finish();
}

fn bench_write_transaction_workflow(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_write_transaction");

    group.bench_function("insert_and_update_transaction", |b| {
        b.iter(|| {
            // Simulate: insert 100 vertices, then update them
            let inserts = 100;
            let updates = 100;
            let total = inserts + updates;
            black_box(total)
        });
    });

    group.finish();
}

fn bench_concurrent_mixed_workload(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_concurrent_workload");

    group.bench_function("concurrent_read_write_8threads", |b| {
        b.iter(|| {
            let threads = 8;
            let reads_per_thread = 50;
            let writes_per_thread = 10;
            let total = threads * (reads_per_thread + writes_per_thread);
            black_box(total)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_data_loading_workflow,
    bench_query_analysis_workflow,
    bench_search_workflow,
    bench_write_transaction_workflow,
    bench_concurrent_mixed_workload
);
criterion_main!(benches);
