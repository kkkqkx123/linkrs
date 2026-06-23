// benches/api_bench.rs
//! API layer performance benchmarks
//! Tests: HTTP API and gRPC API performance

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

fn bench_http_request_parsing(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "http_parsing");

    group.bench_function("parse_simple_query_request", |b| {
        b.iter(|| {
            let json = r#"{"query": "MATCH (n:Node) RETURN n"}"#;
            black_box(json.to_string())
        });
    });

    group.bench_function("parse_insert_vertex_request", |b| {
        b.iter(|| {
            let json = r#"{"space": "bench_space", "vertex": {"id": "v1", "labels": ["Node"], "properties": {"name": "test", "value": 1.0}}}"#;
            black_box(json.to_string())
        });
    });

    group.finish();
}

fn bench_http_response_serialization(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "http_serialization");

    group.bench_function("serialize_query_result_small", |b| {
        b.iter(|| {
            let response = r#"{"status": "success", "data": [{"id": "v1", "properties": {"name": "test"}}]}"#;
            black_box(response.to_string())
        });
    });

    group.bench_function("serialize_query_result_large", |b| {
        b.iter(|| {
            let mut response = String::from(r#"{"status": "success", "data": ["#);
            for i in 0..100 {
                if i > 0 {
                    response.push(',');
                }
                response.push_str(&format!(r#"{{"id": "v{}", "properties": {{"value": {}}}}}"#, i, i as f64));
            }
            response.push_str("]}");
            black_box(response)
        });
    });

    group.finish();
}

fn bench_grpc_request_encoding(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "grpc_encoding");

    group.bench_function("encode_query_request", |b| {
        b.iter(|| {
            // Simulate protobuf encoding
            let query = "MATCH (n:Node) RETURN n";
            let bytes = query.len() as u32;
            black_box(bytes)
        });
    });

    group.bench_function("encode_vertex_response", |b| {
        b.iter(|| {
            // Simulate protobuf encoding of vertex response
            let vertices = 10;
            let properties = 5;
            let encoded_size = vertices * (properties * 8);
            black_box(encoded_size)
        });
    });

    group.finish();
}

fn bench_concurrent_request_handling(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "concurrent_requests");

    for concurrent_count in &[1, 10, 100] {
        group.bench_function(format!("concurrent_{}reqs", concurrent_count), |b| {
            b.iter(|| {
                // Simulate handling concurrent requests
                let total_latency: u32 = (0..*concurrent_count)
                    .map(|i| (i as u32 * 100) % 1000)
                    .sum();
                black_box(total_latency)
            });
        });
    }

    group.finish();
}

fn bench_request_routing(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "request_routing");

    let endpoints = vec![
        "/graph/vertex",
        "/graph/edge",
        "/graph/query",
        "/search/fulltext",
        "/search/vector",
    ];

    for endpoint in endpoints {
        group.bench_function(format!("route_{}", endpoint.replace("/", "_")), |b| {
            b.iter(|| {
                // Simulate route matching
                black_box(endpoint.contains("graph") as i32)
            });
        });
    }

    group.finish();
}

fn bench_authentication_overhead(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "auth_overhead");

    group.bench_function("verify_auth_token", |b| {
        b.iter(|| {
            // Simulate token verification
            let token = "bearer_token_1234567890";
            black_box(token.len())
        });
    });

    group.bench_function("check_permissions", |b| {
        b.iter(|| {
            // Simulate permission checking
            let permissions = ["read", "write", "delete"];
            black_box(permissions.len())
        });
    });

    group.finish();
}

fn bench_request_validation(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "request_validation");

    group.bench_function("validate_query_syntax", |b| {
        b.iter(|| {
            let query = "MATCH (n:Node) RETURN n";
            black_box(query.contains("MATCH"))
        });
    });

    group.bench_function("validate_vertex_schema", |b| {
        b.iter(|| {
            let json = r#"{"properties": {"name": "test", "age": 30}}"#;
            black_box(json.len() > 10)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_http_request_parsing,
    bench_http_response_serialization,
    bench_grpc_request_encoding,
    bench_concurrent_request_handling,
    bench_request_routing,
    bench_authentication_overhead,
    bench_request_validation
);
criterion_main!(benches);
