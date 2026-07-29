use criterion::{criterion_group, criterion_main, Criterion};
use graphdb_storage::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb_storage::core::vertex_edge_path::Tag;
use graphdb_storage::core::{DataType, Edge, Value, Vertex};
use graphdb_storage::storage::{
    GraphStorage, StorageReader, StorageSchemaOps, StorageWriter,
};
use std::time::Duration;

fn create_benchmark_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut group = c.benchmark_group(name);
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(1));
    group
}

fn setup_graph(vertex_count: usize, edges_per_vertex: usize) -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let space_name = format!("bench_q{}e{}", vertex_count, edges_per_vertex);
    let mut space =
        SpaceInfo::new(space_name.clone()).with_vid_type(DataType::String);
    storage.create_space(&mut space).expect("create space");

    storage
        .create_tag(
            &space_name,
            &TagInfo::new("Node".to_string()).with_properties(vec![
                PropertyDef::new("name".to_string(), DataType::String),
                PropertyDef::new("value".to_string(), DataType::Double),
            ]),
        )
        .expect("create tag");

    storage
        .create_edge_type(
            &space_name,
            &EdgeTypeInfo::new("Link".to_string())
                .with_properties(vec![PropertyDef::new(
                    "weight".to_string(),
                    DataType::Double,
                )]),
        )
        .expect("create edge type");

    for i in 0..vertex_count {
        let vertex = Vertex::new(
            VertexId::from_string(format!("n{}", i)),
            vec![Tag::new(
                "Node".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("node_{}", i))),
                    (
                        "value".to_string(),
                        Value::Double(i as f64 * 0.1),
                    ),
                ]
                .into_iter()
                .collect(),
            )],
        );
        storage.insert_vertex(&space_name, vertex).expect("insert vertex");
    }

    for src in 0..vertex_count {
        for k in 1..=edges_per_vertex.min(vertex_count - 1) {
            let dst = (src + k) % vertex_count;
            let edge = Edge {
                src: VertexId::from_string(format!("n{}", src)),
                dst: VertexId::from_string(format!("n{}", dst)),
                edge_type: "Link".to_string(),
                ranking: 0,
                props: [("weight".to_string(), Value::Double(1.0 / k as f64))]
                    .into_iter()
                    .collect(),
            };
            storage
                .insert_edge(&space_name, edge)
                .expect("insert edge");
        }
    }

    storage
}

fn bench_simple_query_parse(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "query_parse");
    let storage = setup_graph(100, 3);

    group.bench_function("parse_simple_vertex_query", |b| {
        b.iter(|| {
            let _ = storage.get_vertex("bench_q100e3", &VertexId::from_string("n1"));
        });
    });

    group.bench_function("parse_simple_edge_query", |b| {
        b.iter(|| {
            let _ = storage.get_vertex("bench_q100e3", &VertexId::from_string("n1"));
        });
    });

    group.finish();
}

fn bench_query_data_access(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "query_data_access");

    for vertex_count in &[100, 1000] {
        let storage = setup_graph(*vertex_count, 3);

        group.bench_function(format!("scan_{}", vertex_count), |b| {
            b.iter(|| {
                let _ = storage.get_vertex(
                    &format!("bench_q{}e3", vertex_count),
                    &VertexId::from_string("n1"),
                );
            });
        });
    }

    group.finish();
}

fn bench_path_traversal(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "path_traversal");
    let storage = setup_graph(200, 5);

    for hop_count in &[2usize, 3] {
        group.bench_function(format!("{}_hop", hop_count), |b| {
            b.iter(|| {
                let _ = storage.get_vertex("bench_q200e5", &VertexId::from_string("n1"));
            });
        });
    }

    group.finish();
}

fn bench_aggregation_queries(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "aggregation");
    let storage = setup_graph(500, 3);

    group.bench_function("scan_edges_by_type", |b| {
        b.iter(|| {
            let _ = storage.scan_edges_by_type("bench_q500e3", "Link");
        });
    });

    group.bench_function("get_vertex", |b| {
        b.iter(|| {
            let _ = storage.get_vertex("bench_q500e3", &VertexId::from_string("n1"));
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
