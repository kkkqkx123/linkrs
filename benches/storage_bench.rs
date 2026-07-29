// benches/storage_bench.rs
//! Storage layer performance benchmarks
//! Tests: vertex operations, edge operations, and persistence

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use graphdb_storage::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb_storage::core::vertex_edge_path::Tag;
use graphdb_storage::core::{DataType, Edge, Value, Vertex};
use graphdb_storage::storage::{
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

criterion_group!(
    benches,
    bench_real_vertex_insert,
    bench_real_edge_insert,
    bench_real_cursor_scan,
    bench_real_checkpoint
);
criterion_main!(benches);
