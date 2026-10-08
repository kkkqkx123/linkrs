use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use linkrs_core::types::{
    Index, IndexConfig, IndexField, IndexType, PropertyDef, SpaceInfo, TagInfo, VertexId,
};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Value, Vertex};
use linkrs_storage::{GraphStorage, StorageSchemaOps, StorageWriter};
use std::sync::Arc;

#[path = "bench_group.rs"]
mod bench_group;
use bench_group::create_benchmark_group;

const SAMPLE_SIZE: usize = 20;

fn indexed_storage() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage should initialize");
    let mut space = SpaceInfo::new("bench".into()).with_vid_type(DataType::BigInt);
    storage
        .create_space(&mut space)
        .expect("space should be created");
    storage
        .create_tag(
            "bench",
            &TagInfo::new("Node".to_string()).with_properties(vec![
                PropertyDef::new("name".into(), DataType::String),
                PropertyDef::new("age".into(), DataType::Int),
                PropertyDef::new("city".into(), DataType::String),
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
        VertexId::try_from_int64(id).expect("valid vertex id"),
        Tag::new(
            "Node".to_string(),
            [
                ("name".into(), Value::string(format!("node_{id}"))),
                ("age".into(), Value::Int(id as i32)),
                (
                    Arc::from("city"),
                    Value::string(format!("city_{}", id % 1000)),
                ),
            ]
            .into_iter()
            .collect(),
        ),
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
    let mut group = create_benchmark_group(c, "indexed_bulk_load", SAMPLE_SIZE);
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
