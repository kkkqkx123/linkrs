// benches/query_bench.rs
//! Query engine performance benchmarks
//! Tests: simple queries, path queries, aggregations

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

/// Generate GQL for query benchmark setup
fn generate_gql_for_query_bench(vertex_count: usize) -> String {
    let mut gql = String::new();
    gql.push_str(&format!("CREATE SPACE IF NOT EXISTS bench_query{} (vid_type=STRING)\n", vertex_count));
    gql.push_str(&format!("USE bench_query{}\n\n", vertex_count));

    gql.push_str("CREATE TAG IF NOT EXISTS Node(\n");
    gql.push_str("    name: STRING,\n");
    gql.push_str("    value: DOUBLE\n");
    gql.push_str(")\n\n");

    gql.push_str("CREATE EDGE IF NOT EXISTS Link(\n");
    gql.push_str("    weight: DOUBLE DEFAULT 1.0\n");
    gql.push_str(")\n\n");

    // Create vertices
    for i in 0..vertex_count {
        gql.push_str(&format!(
            "INSERT VERTEX Node(name, value) VALUES \"n{}\":(\"node_{}\", {})\n",
            i, i, i as f64 * 0.1
        ));
    }

    gql.push('\n');

    // Create edges in small-world network pattern
    for i in 0..vertex_count {
        // Connect to next K neighbors
        for k in 1..=3.min(vertex_count - 1) {
            let j = (i + k) % vertex_count;
            gql.push_str(&format!(
                "INSERT EDGE Link(weight) VALUES \"n{}\"->\"n{}\"({})\n",
                i, j, 1.0 / k as f64
            ));
        }
    }

    gql
}

fn bench_simple_query_parse(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "query_parse");

    group.bench_function("parse_simple_vertex_query", |b| {
        b.iter(|| {
            black_box("FETCH PROP ON Node \"n1\"".to_string())
        });
    });

    group.bench_function("parse_simple_edge_query", |b| {
        b.iter(|| {
            black_box("MATCH (v:Node) --> (u:Node) WHERE id(v) == \"n1\" RETURN u".to_string())
        });
    });

    group.finish();
}

fn bench_query_data_access(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "query_data_access");

    for vertex_count in &[100, 1000, 10000] {
        let setup = generate_gql_for_query_bench(*vertex_count);
        let vertex_lines = setup.matches("INSERT VERTEX").count();

        group.bench_with_input(
            BenchmarkId::from_parameter(vertex_count),
            vertex_count,
            |b, _| {
                b.iter(|| {
                    // Simulate accessing vertex properties
                    black_box(vertex_lines)
                });
            },
        );
    }

    group.finish();
}

fn bench_path_traversal(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "path_traversal");

    for hop_count in &[2, 3, 5] {
        group.bench_with_input(
            BenchmarkId::from_parameter(hop_count),
            hop_count,
            |b, _| {
                let query = format!("MATCH p=(v:Node)-[*1..{}]-(u:Node) WHERE id(v)=\"n1\" RETURN p", hop_count);
                b.iter(|| {
                    black_box(query.clone())
                });
            },
        );
    }

    group.finish();
}

fn bench_aggregation_queries(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "aggregation");

    group.bench_function("count_vertices", |b| {
        b.iter(|| {
            black_box("MATCH (n:Node) RETURN COUNT(*) as count".to_string())
        });
    });

    group.bench_function("sum_property", |b| {
        b.iter(|| {
            black_box("MATCH (n:Node) RETURN SUM(n.value) as total".to_string())
        });
    });

    group.bench_function("avg_property", |b| {
        b.iter(|| {
            black_box("MATCH (n:Node) RETURN AVG(n.value) as avg".to_string())
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_simple_query_parse,
    bench_query_data_access,
    bench_path_traversal,
    bench_aggregation_queries
);
criterion_main!(benches);
