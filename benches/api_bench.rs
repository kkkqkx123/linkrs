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

#[cfg(feature = "embedded")]
fn bench_json_serialization(c: &mut Criterion) {
    use graphdb_storage::core::types::VertexId;
    use graphdb_storage::core::vertex_edge_path::Tag;
    use graphdb_storage::core::{Value, Vertex};

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

#[cfg(not(feature = "embedded"))]
fn bench_json_serialization(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "json_serialization");
    group.bench_function("placeholder", |b| b.iter(|| black_box("{}")));
    group.finish();
}

#[cfg(feature = "embedded")]
fn bench_json_deserialization(c: &mut Criterion) {
    use graphdb_storage::core::types::VertexId;
    use graphdb_storage::core::vertex_edge_path::Tag;
    use graphdb_storage::core::{Value, Vertex};

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

#[cfg(not(feature = "embedded"))]
fn bench_json_deserialization(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "json_deserialization");
    group.bench_function("placeholder", |b| b.iter(|| black_box(0)));
    group.finish();
}

#[cfg(feature = "embedded")]
fn bench_database_operations(c: &mut Criterion) {
    use graphdb_api::api::embedded::database::GraphDatabase;

    let db = GraphDatabase::open_in_memory().expect("open in memory");
    let session = db.session();
    let mut session = session.write();

    session
        .execute("CREATE SPACE bench (vid_type=INT64)")
        .expect("create space");
    session.use_space("bench").expect("use space");
    session
        .execute("CREATE TAG Node(name STRING, value INT64)")
        .expect("create tag");

    let mut group = create_benchmark_group(c, "database_ops");
    group.sample_size(50);

    group.bench_function("insert_vertex", |b| {
        b.iter(|| {
            let result = session
                .execute("INSERT VERTEX Node(name, value) VALUES (1)(\"node_1\", 100)")
                .expect("insert");
            black_box(result.len());
        });
    });

    group.finish();
}

#[cfg(not(feature = "embedded"))]
fn bench_database_operations(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "database_ops");
    group.sample_size(50);
    group.bench_function("placeholder", |b| b.iter(|| black_box(0)));
    group.finish();
}

#[cfg(feature = "embedded")]
fn bench_transaction_api(c: &mut Criterion) {
    use graphdb_api::api::embedded::database::GraphDatabase;

    let db = GraphDatabase::open_in_memory().expect("open in memory");
    let session = db.session();
    let mut session = session.write();

    session
        .execute("CREATE SPACE bench_txn (vid_type=INT64)")
        .expect("create space");
    session.use_space("bench_txn").expect("use space");
    session
        .execute("CREATE TAG T(name STRING)")
        .expect("create tag");

    let mut group = create_benchmark_group(c, "transaction_api");
    group.sample_size(50);

    group.bench_function("begin_commit", |b| {
        b.iter(|| {
            let txn = session.begin_transaction().expect("begin");
            session
                .execute("INSERT VERTEX T(name) VALUES (99)(\"txn_test\")")
                .expect("insert");
            session.commit_transaction(txn).expect("commit");
            black_box(())
        });
    });

    group.finish();
}

#[cfg(not(feature = "embedded"))]
fn bench_transaction_api(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "transaction_api");
    group.sample_size(50);
    group.bench_function("placeholder", |b| b.iter(|| black_box(0)));
    group.finish();
}

criterion_group!(
    benches,
    bench_json_serialization,
    bench_json_deserialization,
    bench_database_operations,
    bench_transaction_api,
);
criterion_main!(benches);
