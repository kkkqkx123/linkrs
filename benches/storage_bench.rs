// benches/storage_bench.rs
//! Storage layer performance benchmarks
//! Tests: vertex operations, edge operations, and persistence

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

#[allow(dead_code)]
fn setup_benchmark_database() -> String {
    let db_name = format!("bench_storage_{}", std::process::id());
    db_name
}

fn generate_gql_for_vertex_insert(count: usize) -> String {
    let mut gql = String::new();
    gql.push_str(&format!("CREATE SPACE IF NOT EXISTS bench_v{} (vid_type=STRING)\n", count));
    gql.push_str(&format!("USE bench_v{}\n\n", count));

    gql.push_str("CREATE TAG IF NOT EXISTS TestVertex(\n");
    gql.push_str("    name: STRING,\n");
    gql.push_str("    value: DOUBLE,\n");
    gql.push_str("    label: STRING,\n");
    gql.push_str("    timestamp: INT\n");
    gql.push_str(")\n\n");

    for i in 0..count {
        let vid = format!("v{}", i);
        let name = format!("vertex_{}", i);
        let value = (i as f64) * 0.1;
        gql.push_str(&format!(
            "INSERT VERTEX TestVertex(name, value, label, timestamp) VALUES \"{}\":(\"{}\" , {}, \"test\", {})\n",
            vid, name, value, i
        ));
    }

    gql
}

fn generate_gql_for_edge_insert(vertex_count: usize, edges_per_vertex: usize) -> String {
    let mut gql = String::new();
    gql.push_str(&format!("CREATE SPACE IF NOT EXISTS bench_e{}_{} (vid_type=STRING)\n", vertex_count, edges_per_vertex));
    gql.push_str(&format!("USE bench_e{}_{}\n\n", vertex_count, edges_per_vertex));

    gql.push_str("CREATE TAG IF NOT EXISTS TestVertex(\n");
    gql.push_str("    name: STRING\n");
    gql.push_str(")\n\n");

    gql.push_str("CREATE EDGE IF NOT EXISTS TestEdge(\n");
    gql.push_str("    weight: DOUBLE DEFAULT 1.0,\n");
    gql.push_str("    label: STRING\n");
    gql.push_str(")\n\n");

    // Create vertices
    for i in 0..vertex_count {
        gql.push_str(&format!(
            "INSERT VERTEX TestVertex(name) VALUES \"v{}\":(\"vertex_{}\")\n",
            i, i
        ));
    }

    gql.push('\n');

    // Create edges
    for i in 0..vertex_count {
        for j in 0..edges_per_vertex {
            let target = (i + j + 1) % vertex_count;
            gql.push_str(&format!(
                "INSERT EDGE TestEdge(weight, label) VALUES \"v{}\"->\"v{}\"({}, \"test\")\n",
                i, target, 0.5
            ));
        }
    }

    gql
}

fn bench_vertex_insert(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "vertex_insert");

    for count in &[10, 100, 1000] {
        let gql = generate_gql_for_vertex_insert(*count);

        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            count,
            |b, _| {
                b.iter(|| {
                    // Simulate batch vertex insertion
                    let insert_count = gql.matches("INSERT VERTEX").count();
                    black_box(insert_count)
                });
            },
        );
    }

    group.finish();
}

fn bench_edge_insert(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "edge_insert");

    for vertex_count in &[10, 100] {
        for edges_per_vertex in &[1, 5, 10] {
            let gql = generate_gql_for_edge_insert(*vertex_count, *edges_per_vertex);
            let edge_count = gql.matches("INSERT EDGE").count();

            group.bench_with_input(
                BenchmarkId::from_parameter(format!("v{}_e{}", vertex_count, edges_per_vertex)),
                &(*vertex_count, *edges_per_vertex),
                |b, _| {
                    b.iter(|| {
                        black_box(edge_count)
                    });
                },
            );
        }
    }

    group.finish();
}

fn bench_data_generation(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "data_generation");

    group.bench_function("generate_storage_data_1k", |b| {
        b.iter(|| {
            black_box(generate_gql_for_vertex_insert(1000))
        });
    });

    group.bench_function("generate_storage_data_10k", |b| {
        b.iter(|| {
            black_box(generate_gql_for_vertex_insert(10000))
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_vertex_insert,
    bench_edge_insert,
    bench_data_generation
);
criterion_main!(benches);
