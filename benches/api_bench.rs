use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, Criterion};

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

fn bench_json_serialization(c: &mut Criterion) {
    use graphdb_core::types::VertexId;
    use graphdb_core::vertex_edge_path::Tag;
    use graphdb_core::{Value, Vertex};

    let mut group = create_benchmark_group(c, "json_serialization");

    let vertex = Vertex::new(
        VertexId::from_int64(42),
        vec![Tag::new(
            "Node".to_string(),
            vec![
                ("name".to_string(), Value::string("test_node")),
                ("value".to_string(), Value::BigInt(100)),
            ]
            .into_iter()
            .collect(),
        )],
    );

    group.bench_function("serialize_vertex", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&vertex).unwrap();
            black_box(json)
        });
    });

    group.bench_function("serialize_100_vertices", |b| {
        let vertices: Vec<Vertex> = (0..100)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64(i),
                    vec![Tag::new(
                        "Node".to_string(),
                        vec![
                            ("name".to_string(), Value::string(format!("node_{}", i))),
                            ("value".to_string(), Value::BigInt(i)),
                        ]
                        .into_iter()
                        .collect(),
                    )],
                )
            })
            .collect();
        b.iter(|| {
            let json = serde_json::to_string(&vertices).unwrap();
            black_box(json)
        });
    });

    group.finish();
}

fn bench_json_deserialization(c: &mut Criterion) {
    use graphdb_core::types::VertexId;
    use graphdb_core::vertex_edge_path::Tag;
    use graphdb_core::{Value, Vertex};

    let mut group = create_benchmark_group(c, "json_deserialization");

    let vertex = Vertex::new(
        VertexId::from_int64(42),
        vec![Tag::new(
            "Node".to_string(),
            vec![
                ("name".to_string(), Value::string("test_node")),
                ("value".to_string(), Value::BigInt(100)),
            ]
            .into_iter()
            .collect(),
        )],
    );
    let json = serde_json::to_string(&vertex).unwrap();

    group.bench_function("deserialize_vertex", |b| {
        b.iter(|| {
            let v: Vertex = serde_json::from_str(&json).unwrap();
            black_box(v)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_json_serialization,
    bench_json_deserialization,
);
criterion_main!(benches);
