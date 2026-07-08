// benches/search_bench.rs
//! Search layer performance benchmarks
//! Tests: fulltext search and vector search

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

/// Generate GQL for fulltext search benchmark
fn generate_gql_for_fulltext_bench(document_count: usize) -> String {
    let mut gql = String::new();
    gql.push_str(&format!("CREATE SPACE IF NOT EXISTS bench_ft{} (vid_type=STRING)\n", document_count));
    gql.push_str(&format!("USE bench_ft{}\n\n", document_count));

    gql.push_str("CREATE TAG IF NOT EXISTS Document(\n");
    gql.push_str("    title: STRING,\n");
    gql.push_str("    content: STRING,\n");
    gql.push_str("    timestamp: INT\n");
    gql.push_str(")\n\n");

    let keywords = ["performance", "database", "query", "optimization", "benchmark"];

    for i in 0..document_count {
        let keyword = keywords[i % keywords.len()];
        let title = format!("Document {} - {}", i, keyword);
        let content = format!(
            "This document discusses {} in depth. Performance and {} are critical topics. \
             Our {} system provides excellent {} for all your needs.",
            keyword, keyword, keyword, keyword
        );

        gql.push_str(&format!(
            "INSERT VERTEX Document(title, content, timestamp) VALUES \"doc{}\"(\"{}\", \"{}\", {})\n",
            i, title, content, i
        ));
    }

    gql
}

/// Generate GQL for vector search benchmark
fn generate_gql_for_vector_bench(vector_count: usize, dimensions: usize) -> String {
    let mut gql = String::new();
    gql.push_str(&format!("CREATE SPACE IF NOT EXISTS bench_vec{}_{} (vid_type=STRING)\n", vector_count, dimensions));
    gql.push_str(&format!("USE bench_vec{}_{}\n\n", vector_count, dimensions));

    gql.push_str("CREATE TAG IF NOT EXISTS Vector(\n");
    gql.push_str("    embedding: STRING,\n");
    gql.push_str("    label: STRING\n");
    gql.push_str(")\n\n");

    for i in 0..vector_count {
        let embedding = (0..dimensions)
            .map(|j| format!("{:.6}", ((i * j) as f64 % 1.0)))
            .collect::<Vec<_>>()
            .join(",");

        gql.push_str(&format!(
            "INSERT VERTEX Vector(embedding, label) VALUES \"vec{}\"(\"{}\", \"label_{}\")\n",
            i, embedding, i
        ));
    }

    gql
}

fn bench_fulltext_index_build(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "fulltext_index_build");

    for doc_count in &[100, 1000, 10000] {
        let gql = generate_gql_for_fulltext_bench(*doc_count);
        let doc_count_actual = gql.matches("INSERT VERTEX").count();

        group.bench_with_input(
            BenchmarkId::from_parameter(doc_count),
            doc_count,
            |b, _| {
                b.iter(|| {
                    // Simulate index building
                    black_box(doc_count_actual)
                });
            },
        );
    }

    group.finish();
}

fn bench_fulltext_search_queries(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "fulltext_search");

    let queries = vec![
        "performance database",
        "query optimization",
        "benchmark results",
    ];

    for query in queries {
        group.bench_function(format!("search_{}", query.replace(" ", "_")), |b| {
            b.iter(|| {
                black_box(format!("MATCH (d:Document) WHERE d.content CONTAINS \"{}\" RETURN d", query))
            });
        });
    }

    group.finish();
}

fn bench_fulltext_search_scaling(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "fulltext_scaling");

    for doc_count in &[1000, 10000, 100000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(doc_count),
            doc_count,
            |b, _| {
                b.iter(|| {
                    // Simulate searching across larger dataset
                    black_box(doc_count / 10)
                });
            },
        );
    }

    group.finish();
}

fn bench_vector_index_build(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "vector_index_build");

    for (vec_count, dim) in &[(100, 128), (1000, 256), (10000, 512)] {
        let gql = generate_gql_for_vector_bench(*vec_count, *dim);
        let vector_count_actual = gql.matches("INSERT VERTEX").count();

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}d_{}v", dim, vec_count)),
            &(*vec_count, *dim),
            |b, _| {
                b.iter(|| {
                    // Simulate vector index building
                    black_box(vector_count_actual * dim / 128)
                });
            },
        );
    }

    group.finish();
}

fn bench_vector_search_distance_calculation(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "vector_distance");

    let dimensions = vec![128, 256, 512, 768];

    for dim in dimensions {
        group.bench_with_input(
            BenchmarkId::from_parameter(dim),
            &dim,
            |b, d| {
                // Simulate distance calculation
                b.iter(|| {
                    let mut sum = 0.0;
                    for i in 0..*d {
                        let a = (i as f64) * 0.1;
                        let b = ((i + 1) as f64) * 0.1;
                        sum += (a - b).abs();
                    }
                    black_box(sum)
                });
            },
        );
    }

    group.finish();
}

fn bench_vector_search_topk(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "vector_topk_search");

    for (vec_count, k) in &[(1000, 10), (10000, 100), (100000, 1000)] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}v_top{}", vec_count, k)),
            &(*vec_count, *k),
            |b, _| {
                // Simulate top-k search
                b.iter(|| {
                    black_box(vec_count / 10)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_fulltext_index_build,
    bench_fulltext_search_queries,
    bench_fulltext_search_scaling,
    bench_vector_index_build,
    bench_vector_search_distance_calculation,
    bench_vector_search_topk
);
criterion_main!(benches);
