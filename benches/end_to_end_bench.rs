use criterion::{criterion_group, criterion_main, Criterion};
use linkrs_core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Edge, Value, Vertex};
use linkrs_storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};

#[path = "bench_group.rs"]
mod bench_group;
use bench_group::create_benchmark_group;

const SAMPLE_SIZE: usize = 30;

fn setup_vertices(storage: &mut GraphStorage, space: &str, count: usize) {
    for i in 0..count {
        let vertex = Vertex::new(
            VertexId::try_from_string(format!("v{}", i)).expect("valid vertex id"),
            Tag::new(
                "Node".to_string(),
                vec![
                    ("name".into(), Value::string(format!("vertex_{}", i))),
                    ("value".into(), Value::Int(i as i32)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        storage.insert_vertex(space, vertex).expect("insert vertex");
    }
}

fn bench_data_loading_workflow(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_data_loading", SAMPLE_SIZE);

    for (vertices, edges_per) in &[(1000usize, 5usize), (5000, 3)] {
        group.bench_function(format!("load_1k_v{}_e{}", vertices, edges_per), |b| {
            b.iter(|| {
                let mut storage = GraphStorage::new().expect("storage init");
                let space = format!("bench_load_{}_{}", vertices, edges_per);
                let mut s = SpaceInfo::new(space.clone()).with_vid_type(DataType::String);
                storage.create_space(&mut s).expect("create space");
                storage
                    .create_tag(
                        &space,
                        &TagInfo::new("Node".to_string()).with_properties(vec![
                            PropertyDef::new("name".into(), DataType::String),
                            PropertyDef::new("value".into(), DataType::Int),
                        ]),
                    )
                    .expect("create tag");
                storage
                    .create_edge_type(
                        &space,
                        &EdgeTypeInfo::new("Link".to_string()).with_properties(vec![
                            PropertyDef::new("weight".into(), DataType::Double),
                        ]),
                    )
                    .expect("create edge type");

                setup_vertices(&mut storage, &space, *vertices);

                let epv = *edges_per;
                for src in 0..*vertices {
                    for k in 1..=epv.min(vertices.saturating_sub(1)) {
                        let dst = (src + k) % vertices;
                        let edge = Edge {
                            src: VertexId::try_from_string(format!("v{}", src))
                                .expect("valid vertex id"),
                            dst: VertexId::try_from_string(format!("v{}", dst))
                                .expect("valid vertex id"),
                            edge_type: "Link".to_string(),
                            ranking: 0,
                            props: [("weight".into(), Value::Double(1.0))]
                                .into_iter()
                                .collect(),
                        };
                        storage.insert_edge(&space, edge).expect("insert edge");
                    }
                }
            });
        });
    }

    group.finish();
}

fn bench_query_analysis_workflow(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_query_analysis", SAMPLE_SIZE);

    let mut storage = GraphStorage::new().expect("storage init");
    let space = "bench_query_analysis";
    let mut s = SpaceInfo::new(space.to_string()).with_vid_type(DataType::String);
    storage.create_space(&mut s).expect("create space");
    storage
        .create_tag(
            space,
            &TagInfo::new("Node".to_string()).with_properties(vec![
                PropertyDef::new("name".into(), DataType::String),
                PropertyDef::new("value".into(), DataType::Double),
            ]),
        )
        .expect("create tag");
    setup_vertices(&mut storage, space, 1000);

    group.bench_function("simple_query_1k_data", |b| {
        b.iter(|| {
            let _ = storage.get_vertex(
                space,
                "Node",
                &VertexId::try_from_string("v0").expect("valid vertex id"),
            );
        });
    });

    group.bench_function("path_query_1k_data", |b| {
        b.iter(|| {
            let _ = storage.scan_edges_by_type(space, "Link");
        });
    });

    group.finish();
}

fn bench_search_workflow(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_search", SAMPLE_SIZE);

    let mut storage = GraphStorage::new().expect("storage init");
    let space = "bench_search";
    let mut s = SpaceInfo::new(space.to_string()).with_vid_type(DataType::String);
    storage.create_space(&mut s).expect("create space");
    storage
        .create_tag(
            space,
            &TagInfo::new("Node".to_string())
                .with_properties(vec![PropertyDef::new("name".into(), DataType::String)]),
        )
        .expect("create tag");
    setup_vertices(&mut storage, space, 100);

    group.bench_function("fulltext_search", |b| {
        b.iter(|| {
            let _ = storage.get_vertex(
                space,
                "Node",
                &VertexId::try_from_string("v0").expect("valid vertex id"),
            );
        });
    });

    group.bench_function("vertex_lookup", |b| {
        b.iter(|| {
            let _ = storage.get_vertex(
                space,
                "Node",
                &VertexId::try_from_string("v50").expect("valid vertex id"),
            );
        });
    });

    group.finish();
}

fn bench_write_transaction_workflow(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_write_transaction", SAMPLE_SIZE);

    group.bench_function("insert_and_update_transaction", |b| {
        b.iter(|| {
            let mut storage = GraphStorage::new().expect("storage init");
            let space = "bench_write";
            let mut s = SpaceInfo::new(space.to_string()).with_vid_type(DataType::String);
            storage.create_space(&mut s).expect("create space");
            storage
                .create_tag(
                    space,
                    &TagInfo::new("Node".to_string()).with_properties(vec![PropertyDef::new(
                        "value".to_string(),
                        DataType::Int,
                    )]),
                )
                .expect("create tag");

            for i in 0..100 {
                let vertex = Vertex::new(
                    VertexId::try_from_string(format!("u{}", i)).expect("valid vertex id"),
                    Tag::new(
                        "Node".to_string(),
                        [("value".into(), Value::Int(i))].into_iter().collect(),
                    ),
                );
                storage.insert_vertex(space, vertex).expect("insert");
            }
        });
    });

    group.finish();
}

fn bench_concurrent_mixed_workload(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_concurrent_workload", SAMPLE_SIZE);

    let mut storage = GraphStorage::new().expect("storage init");
    let space = "bench_concurrent";
    let mut s = SpaceInfo::new(space.to_string()).with_vid_type(DataType::String);
    storage.create_space(&mut s).expect("create space");
    storage
        .create_tag(
            space,
            &TagInfo::new("Node".to_string())
                .with_properties(vec![PropertyDef::new("value".into(), DataType::Int)]),
        )
        .expect("create tag");
    setup_vertices(&mut storage, space, 100);

    group.bench_function("concurrent_read", |b| {
        b.iter(|| {
            let _ = storage.get_vertex(
                space,
                "Node",
                &VertexId::try_from_string("v0").expect("valid vertex id"),
            );
            let _ = storage.get_vertex(
                space,
                "Node",
                &VertexId::try_from_string("v50").expect("valid vertex id"),
            );
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_data_loading_workflow,
    bench_query_analysis_workflow,
    bench_search_workflow,
    bench_write_transaction_workflow,
    bench_concurrent_mixed_workload
);
criterion_main!(benches);
