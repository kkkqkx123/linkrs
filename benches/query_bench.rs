use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use graphdb_storage::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb_storage::core::vertex_edge_path::Tag;
use graphdb_storage::core::{DataType, Edge, Value, Vertex};
use graphdb_storage::storage::{
    GraphStorage, ScanOptions, StorageReader, StorageSchemaOps, StorageWriter,
};
use std::collections::HashMap;
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

fn setup_large_graph(vertex_count: u64, edges_per_vertex: usize) -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let space_name = format!("large_q{}e{}", vertex_count, edges_per_vertex);
    let mut space = SpaceInfo::new(space_name.clone()).with_vid_type(DataType::BigInt);
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

    for i in 0..vertex_count as i64 {
        let vertex = Vertex::new(
            VertexId::from_int64(i),
            vec![Tag::new(
                "Node".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("node_{}", i))),
                    ("value".to_string(), Value::Double(i as f64 * 0.1)),
                ]
                .into_iter()
                .collect(),
            )],
        );
        storage.insert_vertex(&space_name, vertex).expect("insert vertex");
    }

    let max_edges = edges_per_vertex.min((vertex_count as usize).saturating_sub(1));
    for src in 0..vertex_count as i64 {
        for k in 1..=max_edges as i64 {
            let dst = (src + k) % vertex_count as i64;
            let edge = Edge {
                src: VertexId::from_int64(src),
                dst: VertexId::from_int64(dst),
                edge_type: "Link".to_string(),
                ranking: 0,
                props: HashMap::new(),
            };
            storage.insert_edge(&space_name, edge).expect("insert edge");
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

fn bench_large_vertex_scan(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "large_vertex_scan");
    for &vertex_count in &[10_000u64, 100_000] {
        let space_name = format!("large_q{}e3", vertex_count);
        let storage = setup_large_graph(vertex_count, 3);
        group.throughput(Throughput::Elements(vertex_count));
        group.bench_function(BenchmarkId::from_parameter(vertex_count), |b| {
            b.iter(|| {
                let mut cursor = storage
                    .create_vertex_cursor(
                        &space_name,
                        &ScanOptions::new()
                            .with_offset(0)
                            .with_limit(vertex_count as usize),
                    )
                    .expect("vertex cursor");
                let mut count = 0usize;
                while !cursor.next_batch(256).expect("cursor batch").is_empty() {
                    count += 1;
                }
                black_box(count);
            });
        });
    }
    group.finish();
}

fn bench_large_count_operations(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "large_count");
    for &vertex_count in &[10_000u64, 100_000] {
        let space_name = format!("large_q{}e3", vertex_count);
        let storage = setup_large_graph(vertex_count, 3);
        group.bench_function(BenchmarkId::new("count_vertices", vertex_count), |b| {
            b.iter(|| {
                let n = storage.count_vertices_by_tag(&space_name, "Node").expect("count");
                black_box(n);
            });
        });
        group.bench_function(BenchmarkId::new("count_edges", vertex_count), |b| {
            b.iter(|| {
                let n = storage.count_edges_by_type(&space_name, "Link").expect("count");
                black_box(n);
            });
        });
    }
    group.finish();
}

fn bench_large_edge_density(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "large_edge_density");
    for &(label, edges_per_vertex) in &[("sparse_1k_x3", 3usize), ("dense_1k_x50", 50)] {
        let space_name = format!("large_q{}e{}", 1_000u64, edges_per_vertex);
        let storage = setup_large_graph(1_000, edges_per_vertex);
        group.bench_function(format!("scan_edges_{}", label), |b| {
            b.iter(|| {
                let edges = storage.scan_edges_by_type(&space_name, "Link").expect("scan");
                black_box(edges.len());
            });
        });
        group.bench_function(format!("get_node_edges_{}", label), |b| {
            b.iter(|| {
                let edges = storage
                    .get_node_edges(
                        &space_name,
                        &VertexId::from_int64(0),
                        graphdb_storage::core::EdgeDirection::Out,
                    )
                    .expect("get edges");
                black_box(edges.len());
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_simple_query_parse,
    bench_query_data_access,
    bench_path_traversal,
    bench_aggregation_queries,
    bench_large_vertex_scan,
    bench_large_count_operations,
    bench_large_edge_density,
);
criterion_main!(benches);
