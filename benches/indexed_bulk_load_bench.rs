use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use graphdb_core::types::{
    Index, IndexConfig, IndexField, IndexType, PropertyDef, SpaceInfo, TagInfo, VertexId,
};
use graphdb_core::vertex_edge_path::Tag;
use graphdb_core::{DataType, Value, Vertex};
use graphdb_storage::{GraphStorage, StorageSchemaOps, StorageWriter};
use std::time::Duration;

fn create_benchmark_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut group = c.benchmark_group(name);
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group
}

fn indexed_storage() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage should initialize");
    let mut space = SpaceInfo::new("bench".to_string()).with_vid_type(DataType::BigInt);
    storage
        .create_space(&mut space)
        .expect("space should be created");
    storage
        .create_tag(
            "bench",
            &TagInfo::new("Node".to_string()).with_properties(vec![
                PropertyDef::new("name".to_string(), DataType::String),
                PropertyDef::new("age".to_string(), DataType::Int),
                PropertyDef::new("city".to_string(), DataType::String),
            ]),
        )
        .expect("tag should be created");
    for (id, field_name) in ["name", "age", "city"].into_iter().enumerate() {
        let index = Index::new(IndexConfig {
            id: id as u64,
            name: format!("idx_node_{field_name}"),
            space_id: 1,
            schema_name: "Node".to_string(),
            fields: vec![IndexField::new(
                field_name.to_string(),
                Value::string(""),
                false,
            )],
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        });
        storage
            .create_tag_index("bench", &index)
            .expect("tag index should be created");
    }
    storage
}

fn build_vertex(id: i64) -> Vertex {
    Vertex::new(
        VertexId::from_int64(id),
        vec![Tag::new(
            "Node".to_string(),
            [
                ("name".to_string(), Value::string(format!("node_{id}"))),
                ("age".to_string(), Value::Int(id as i32)),
                (
                    "city".to_string(),
                    Value::string(format!("city_{}", id % 1000)),
                ),
            ]
            .into_iter()
            .collect(),
        )],
    )
}

/// Bulk-load N vertices into a tag with three native tag indexes.
///
/// The write path publishes one index generation per statement and performs a
/// per-statement resource snapshot for admission control. This benchmark guards
/// against the O(generations) regressions that made bulk loads quadratic: per
/// statement cost must stay flat as the number of already-loaded vertices
/// (and thus generations) grows.
fn bench_indexed_bulk_load(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "indexed_bulk_load");
    for vertex_count in [1_000usize, 2_000, 5_000, 10_000] {
        group.throughput(Throughput::Elements(vertex_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(vertex_count),
            &vertex_count,
            |b, &count| {
                b.iter_batched(
                    indexed_storage,
                    |mut storage| {
                        for id in 0..count as i64 {
                            storage
                                .insert_vertex("bench", build_vertex(id))
                                .expect("vertex insert");
                        }
                        black_box(storage);
                    },
                    criterion::BatchSize::NumIterations(1),
                );
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_indexed_bulk_load);
criterion_main!(benches);
