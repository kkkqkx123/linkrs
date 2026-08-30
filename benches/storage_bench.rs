use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use graphdb_core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb_core::vertex_edge_path::Tag;
use graphdb_core::{DataType, Edge, Value, Vertex};
use graphdb_storage::{
    GraphStorage, ScanOptions, StoragePersistenceOps, StorageReader, StorageSchemaOps,
    StorageWriter,
};
use std::collections::HashMap;
use std::time::Duration;
use tempfile::TempDir;

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

fn benchmark_storage_with_schema() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage should initialize");
    let mut space = SpaceInfo::new("bench".to_string()).with_vid_type(DataType::BigInt);
    storage
        .create_space(&mut space)
        .expect("space should be created");
    storage
        .create_tag(
            "bench",
            &TagInfo::new("Node".to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("tag should be created");
    storage
}

fn build_vertices(vertex_count: u64) -> GraphStorage {
    let mut storage = benchmark_storage_with_schema();
    for id in 0..vertex_count as i64 {
        storage
            .insert_vertex(
                "bench",
                Vertex::new(
                    VertexId::from_int64(id),
                    vec![Tag::new(
                        "Node".to_string(),
                        [("value".to_string(), Value::BigInt(id))]
                            .into_iter()
                            .collect(),
                    )],
                ),
            )
            .expect("vertex insert");
    }
    storage
}

fn build_vertices_with_edges(vertex_count: u64, edges_per_vertex: usize) -> GraphStorage {
    let mut storage = build_vertices(vertex_count);
    storage
        .create_edge_type(
            "bench",
            &EdgeTypeInfo::new("Link".to_string())
                .with_src_tag("Node".to_string())
                .with_dst_tag("Node".to_string()),
        )
        .expect("edge type should be created");
    let max_edges = edges_per_vertex.min((vertex_count as usize).saturating_sub(1));
    for src in 0..vertex_count as i64 {
        for k in 1..=max_edges {
            let dst = (src + k as i64) % vertex_count as i64;
            storage
                .insert_edge(
                    "bench",
                    Edge {
                        src: VertexId::from_int64(src),
                        dst: VertexId::from_int64(dst),
                        edge_type: "Link".to_string(),
                        ranking: 0,
                        props: HashMap::new(),
                    },
                )
                .expect("edge insert");
        }
    }
    storage
}

fn bench_real_vertex_insert(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "storage_vertex_insert");
    let mut storage = benchmark_storage_with_schema();
    let mut next_id = 0i64;
    group.bench_function("single", |b| {
        b.iter(|| {
            let id = next_id;
            next_id += 1;
            let vertex = Vertex::new(
                VertexId::from_int64(id),
                vec![Tag::new(
                    "Node".to_string(),
                    [("value".to_string(), Value::BigInt(id))]
                        .into_iter()
                        .collect(),
                )],
            );
            black_box(
                storage
                    .insert_vertex("bench", vertex)
                    .expect("vertex insert"),
            );
        });
    });
    group.finish();
}

fn bench_bulk_vertex_insert(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "storage_bulk_vertex_insert");
    for &size in &[1_000u64, 10_000] {
        group.throughput(Throughput::Elements(size));
        group.bench_function(BenchmarkId::from_parameter(size), |b| {
            b.iter_batched(
                || {
                    let storage = benchmark_storage_with_schema();
                    let vertices: Vec<_> = (0..size as i64)
                        .map(|i| {
                            Vertex::new(
                                VertexId::from_int64(i),
                                vec![Tag::new(
                                    "Node".to_string(),
                                    [("value".to_string(), Value::BigInt(i))]
                                        .into_iter()
                                        .collect(),
                                )],
                            )
                        })
                        .collect();
                    (storage, vertices)
                },
                |(mut storage, vertices)| {
                    storage.batch_insert_vertices("bench", vertices).unwrap();
                },
                criterion::BatchSize::NumIterations(1),
            )
        });
    }
    group.finish();
}

fn bench_real_edge_insert(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "storage_edge_insert");
    let mut storage = benchmark_storage_with_schema();
    storage
        .create_edge_type(
            "bench",
            &EdgeTypeInfo::new("Link".to_string())
                .with_src_tag("Node".to_string())
                .with_dst_tag("Node".to_string()),
        )
        .expect("edge type should be created");
    for id in 0..100i64 {
        storage
            .insert_vertex(
                "bench",
                Vertex::new(
                    VertexId::from_int64(id),
                    vec![Tag::new(
                        "Node".to_string(),
                        [("value".to_string(), Value::BigInt(id))]
                            .into_iter()
                            .collect(),
                    )],
                ),
            )
            .expect("vertex insert");
    }
    let mut next_id = 0i64;
    group.bench_function("single", |b| {
        b.iter(|| {
            let src = next_id % 100;
            let dst = (src + 1) % 100;
            next_id += 1;
            storage
                .insert_edge(
                    "bench",
                    Edge {
                        src: VertexId::from_int64(src),
                        dst: VertexId::from_int64(dst),
                        edge_type: "Link".to_string(),
                        ranking: next_id,
                        props: HashMap::new(),
                    },
                )
                .expect("edge insert");
            black_box(());
        });
    });
    group.finish();
}

fn bench_edge_insert_density(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "storage_edge_insert_density");
    for &(label, vertex_count, edges_per_vertex) in &[
        ("sparse_1k_x3", 1_000u64, 3usize),
        ("dense_1k_x100", 1_000, 100),
    ] {
        let edge_count =
            vertex_count as usize * edges_per_vertex.min((vertex_count as usize).saturating_sub(1));
        group.throughput(Throughput::Elements(edge_count as u64));
        group.bench_function(BenchmarkId::new(label, vertex_count), |b| {
            b.iter_batched(
                || {
                    let mut storage = build_vertices(vertex_count);
                    storage
                        .create_edge_type(
                            "bench",
                            &EdgeTypeInfo::new("Link".to_string())
                                .with_src_tag("Node".to_string())
                                .with_dst_tag("Node".to_string()),
                        )
                        .expect("edge type should be created");
                    let max_edges = edges_per_vertex.min((vertex_count as usize).saturating_sub(1));
                    let edges: Vec<_> = (0..vertex_count as i64)
                        .flat_map(|src| {
                            (1..=max_edges as i64).map(move |k| {
                                let dst = (src + k) % vertex_count as i64;
                                Edge {
                                    src: VertexId::from_int64(src),
                                    dst: VertexId::from_int64(dst),
                                    edge_type: "Link".to_string(),
                                    ranking: 0,
                                    props: HashMap::new(),
                                }
                            })
                        })
                        .collect();
                    (storage, edges)
                },
                |(mut storage, edges)| {
                    for e in edges {
                        storage.insert_edge("bench", e).unwrap();
                    }
                },
                criterion::BatchSize::NumIterations(1),
            )
        });
    }
    group.finish();
}

fn bench_real_cursor_scan(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "storage_cursor_scan");
    let mut storage = benchmark_storage_with_schema();
    for id in 0..10_000i64 {
        storage
            .insert_vertex(
                "bench",
                Vertex::new(
                    VertexId::from_int64(id),
                    vec![Tag::new(
                        "Node".to_string(),
                        [("value".to_string(), Value::BigInt(id))]
                            .into_iter()
                            .collect(),
                    )],
                ),
            )
            .expect("vertex insert");
    }
    group.bench_function("lazy_10k", |b| {
        b.iter(|| {
            let mut cursor = storage
                .create_vertex_cursor(
                    "bench",
                    &ScanOptions::new().with_offset(1_000).with_limit(1_000),
                )
                .expect("cursor should open");
            let mut count = 0usize;
            while !cursor.next_batch(256).expect("cursor batch").is_empty() {
                count += 1;
            }
            black_box(count);
        });
    });
    group.bench_function("lazy_10k_projected", |b| {
        b.iter(|| {
            let mut cursor = storage
                .create_vertex_cursor(
                    "bench",
                    &ScanOptions::new()
                        .with_offset(1_000)
                        .with_limit(1_000)
                        .with_projection_named(vec!["value".to_string()]),
                )
                .expect("projected cursor should open");
            let mut count = 0usize;
            while !cursor.next_batch(256).expect("cursor batch").is_empty() {
                count += 1;
            }
            black_box(count);
        });
    });
    group.finish();
}

fn bench_scaled_cursor_scan(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "storage_scaled_cursor_scan");
    for &vertex_count in &[10_000u64, 100_000] {
        let name = format!("full_scan_{}", vertex_count);
        let storage = build_vertices(vertex_count);
        group.throughput(Throughput::Elements(vertex_count));
        group.bench_function(&name, |b| {
            b.iter(|| {
                let mut cursor = storage
                    .create_vertex_cursor(
                        "bench",
                        &ScanOptions::new()
                            .with_offset(0)
                            .with_limit(vertex_count as usize),
                    )
                    .expect("cursor should open");
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

fn bench_scan_all_vertices(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "storage_scan_all_vertices");
    for &vertex_count in &[10_000u64, 100_000] {
        let storage = build_vertices(vertex_count);
        group.throughput(Throughput::Elements(vertex_count));
        group.bench_function(BenchmarkId::from_parameter(vertex_count), |b| {
            b.iter(|| {
                let vertices = storage.scan_vertices("bench").expect("scan");
                black_box(vertices.len());
            });
        });
    }
    group.finish();
}

fn bench_real_checkpoint(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "storage_checkpoint");
    let root = TempDir::new().expect("temp directory");
    let mut storage = GraphStorage::new_with_path(root.path().to_path_buf())
        .expect("persistent storage should initialize");
    let mut space = SpaceInfo::new("bench".to_string()).with_vid_type(DataType::BigInt);
    storage
        .create_space(&mut space)
        .expect("space should be created");
    storage
        .create_tag(
            "bench",
            &TagInfo::new("Node".to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("tag should be created");
    for id in 0..1_000i64 {
        storage
            .insert_vertex(
                "bench",
                Vertex::new(
                    VertexId::from_int64(id),
                    vec![Tag::new(
                        "Node".to_string(),
                        [("value".to_string(), Value::BigInt(id))]
                            .into_iter()
                            .collect(),
                    )],
                ),
            )
            .expect("vertex insert");
    }
    group.bench_function("1k_vertices", |b| {
        b.iter(|| {
            black_box(
                storage
                    .create_checkpoint()
                    .expect("checkpoint should succeed"),
            );
        });
    });
    group.finish();
}

fn bench_scaled_checkpoint(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "storage_scaled_checkpoint");
    for &vertex_count in &[10_000u64, 100_000] {
        let root = TempDir::new().expect("temp directory");
        let mut storage = GraphStorage::new_with_path(root.path().to_path_buf())
            .expect("persistent storage should initialize");
        {
            let mut space = SpaceInfo::new("bench".to_string()).with_vid_type(DataType::BigInt);
            storage
                .create_space(&mut space)
                .expect("space should be created");
        }
        storage
            .create_tag(
                "bench",
                &TagInfo::new("Node".to_string()).with_properties(vec![PropertyDef::new(
                    "value".to_string(),
                    DataType::BigInt,
                )]),
            )
            .expect("tag should be created");
        for id in 0..vertex_count as i64 {
            storage
                .insert_vertex(
                    "bench",
                    Vertex::new(
                        VertexId::from_int64(id),
                        vec![Tag::new(
                            "Node".to_string(),
                            [("value".to_string(), Value::BigInt(id))]
                                .into_iter()
                                .collect(),
                        )],
                    ),
                )
                .expect("vertex insert");
        }
        group.throughput(Throughput::Elements(vertex_count));
        group.bench_function(BenchmarkId::from_parameter(vertex_count), |b| {
            b.iter(|| {
                black_box(
                    storage
                        .create_checkpoint()
                        .expect("checkpoint should succeed"),
                );
            });
        });
    }
    group.finish();
}

fn bench_scaled_graph_operations(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "storage_scaled_mixed");
    for &(label, vertex_count, edges_per_vertex) in
        &[("1k_x3", 1_000u64, 3usize), ("10k_x3", 10_000, 3)]
    {
        let storage = build_vertices_with_edges(vertex_count, edges_per_vertex);
        group.bench_function(format!("scan_edges_{}", label), |b| {
            b.iter(|| {
                let edges = storage
                    .scan_edges_by_type("bench", "Link")
                    .expect("scan edges");
                black_box(edges.len());
            });
        });
        group.bench_function(format!("count_vertices_{}", label), |b| {
            b.iter(|| {
                let count = storage
                    .count_vertices_by_tag("bench", "Node")
                    .expect("count");
                black_box(count);
            });
        });
        group.bench_function(format!("count_edges_{}", label), |b| {
            b.iter(|| {
                let count = storage.count_edges_by_type("bench", "Link").expect("count");
                black_box(count);
            });
        });
    }
    group.finish();
}

fn bench_sparse_id_insert_throughput(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "csr_sparse_id_insert");
    for &(label, high_ids) in &[
        ("dense_100k", &[][..]),
        ("sparse_1M", &[1_000_000i64][..]),
        ("sparse_4M", &[1_000_000i64, 2_000_000, 4_000_000][..]),
    ] {
        let n = 100_000u64;
        let total_edges = n + high_ids.len() as u64;
        group.throughput(Throughput::Elements(total_edges));
        group.bench_function(BenchmarkId::new("insert_edges", label), |b| {
            b.iter_batched(
                || {
                    let mut storage = benchmark_storage_with_schema();
                    storage
                        .create_edge_type(
                            "bench",
                            &EdgeTypeInfo::new("Link".to_string())
                                .with_src_tag("Node".to_string())
                                .with_dst_tag("Node".to_string()),
                        )
                        .expect("edge type should be created");
                    for id in 0..n as i64 {
                        storage
                            .insert_vertex(
                                "bench",
                                Vertex::new(
                                    VertexId::from_int64(id),
                                    vec![Tag::new(
                                        "Node".to_string(),
                                        [("value".to_string(), Value::BigInt(id))]
                                            .into_iter()
                                            .collect(),
                                    )],
                                ),
                            )
                            .expect("vertex insert");
                    }
                    for &high in high_ids {
                        storage
                            .insert_vertex(
                                "bench",
                                Vertex::new(
                                    VertexId::from_int64(high),
                                    vec![Tag::new(
                                        "Node".to_string(),
                                        [("value".to_string(), Value::BigInt(high))]
                                            .into_iter()
                                            .collect(),
                                    )],
                                ),
                            )
                            .expect("vertex insert");
                    }
                    storage
                },
                |mut storage| {
                    for src in 0..n as i64 {
                        let dst = (src + 1) % n as i64;
                        storage
                            .insert_edge(
                                "bench",
                                Edge {
                                    src: VertexId::from_int64(src),
                                    dst: VertexId::from_int64(dst),
                                    edge_type: "Link".to_string(),
                                    ranking: 0,
                                    props: HashMap::new(),
                                },
                            )
                            .expect("edge insert");
                    }
                    for &high in high_ids {
                        storage
                            .insert_edge(
                                "bench",
                                Edge {
                                    src: VertexId::from_int64(high),
                                    dst: VertexId::from_int64(0),
                                    edge_type: "Link".to_string(),
                                    ranking: 0,
                                    props: HashMap::new(),
                                },
                            )
                            .expect("edge insert");
                    }
                    black_box(());
                },
                criterion::BatchSize::NumIterations(1),
            )
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_real_vertex_insert,
    bench_bulk_vertex_insert,
    bench_real_edge_insert,
    bench_edge_insert_density,
    bench_real_cursor_scan,
    bench_scaled_cursor_scan,
    bench_scan_all_vertices,
    bench_real_checkpoint,
    bench_scaled_checkpoint,
    bench_scaled_graph_operations,
    bench_sparse_id_insert_throughput,
);
criterion_main!(benches);
