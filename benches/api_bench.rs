use criterion::{black_box, criterion_group, criterion_main, Criterion};

#[path = "bench_group.rs"]
mod bench_group;
use bench_group::create_benchmark_group;

const SAMPLE_SIZE: usize = 100;

fn bench_json_serialization(c: &mut Criterion) {
    use graphdb_core::types::VertexId;
    use graphdb_core::vertex_edge_path::Tag;
    use graphdb_core::{Value, Vertex};

    let mut group = create_benchmark_group(c, "json_serialization", SAMPLE_SIZE);

    let vertex = Vertex::new(
        VertexId::try_from_int64(42).expect("valid vertex id"),
        Tag::new(
            "Node".to_string(),
            vec![
                ("name".into(), Value::string("test_node")),
                ("value".into(), Value::BigInt(100)),
            ]
            .into_iter()
            .collect(),
        ),
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
                    VertexId::try_from_int64(i).expect("valid vertex id"),
                    Tag::new(
                        "Node".to_string(),
                        vec![
                            ("name".into(), Value::string(format!("node_{}", i))),
                            ("value".into(), Value::BigInt(i)),
                        ]
                        .into_iter()
                        .collect(),
                    ),
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

    let mut group = create_benchmark_group(c, "json_deserialization", SAMPLE_SIZE);

    let vertex = Vertex::new(
        VertexId::try_from_int64(42).expect("valid vertex id"),
        Tag::new(
            "Node".to_string(),
            vec![
                ("name".into(), Value::string("test_node")),
                ("value".into(), Value::BigInt(100)),
            ]
            .into_iter()
            .collect(),
        ),
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
