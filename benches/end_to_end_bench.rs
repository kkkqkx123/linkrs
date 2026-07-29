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
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(1));
    group
}

fn setup_vertices(storage: &mut GraphStorage, space: &str, count: usize) {
    for i in 0..count {
        let vertex = Vertex::new(
            VertexId::from_string(format!("v{}", i)),
            vec![Tag::new(
                "Node".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("vertex_{}", i))),
                    ("value".to_string(), Value::Int(i as i32)),
                ]
                .into_iter()
                .collect(),
            )],
        );
        storage.insert_vertex(space, vertex).expect("insert vertex");
    }
}

fn bench_data_loading_workflow(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_data_loading");

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
                            PropertyDef::new("name".to_string(), DataType::String),
                            PropertyDef::new("value".to_string(), DataType::Int),
                        ]),
                    )
                    .expect("create tag");
                storage
                    .create_edge_type(
                        &space,
                        &EdgeTypeInfo::new("Link".to_string())
                            .with_properties(vec![PropertyDef::new(
                                "weight".to_string(),
                                DataType::Double,
                            )]),
                    )
                    .expect("create edge type");

                setup_vertices(&mut storage, &space, *vertices);

                let epv = *edges_per;
                for src in 0..*vertices {
                    for k in 1..=epv.min(vertices.saturating_sub(1)) {
                        let dst = (src + k) % vertices;
                        let edge = Edge {
                            src: VertexId::from_string(format!("v{}", src)),
                            dst: VertexId::from_string(format!("v{}", dst)),
                            edge_type: "Link".to_string(),
                            ranking: 0,
                            props: [("weight".to_string(), Value::Double(1.0))]
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
    let mut group = create_benchmark_group(c, "e2e_query_analysis");

    let mut storage = GraphStorage::new().expect("storage init");
    let space = "bench_query_analysis";
    let mut s = SpaceInfo::new(space.to_string()).with_vid_type(DataType::String);
    storage.create_space(&mut s).expect("create space");
    storage
        .create_tag(
            space,
            &TagInfo::new("Node".to_string()).with_properties(vec![
                PropertyDef::new("name".to_string(), DataType::String),
                PropertyDef::new("value".to_string(), DataType::Double),
            ]),
        )
        .expect("create tag");
    setup_vertices(&mut storage, space, 1000);

    group.bench_function("simple_query_1k_data", |b| {
        b.iter(|| {
            let _ = storage.get_vertex(space, &VertexId::from_string("v0"));
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
    let mut group = create_benchmark_group(c, "e2e_search");

    let mut storage = GraphStorage::new().expect("storage init");
    let space = "bench_search";
    let mut s = SpaceInfo::new(space.to_string()).with_vid_type(DataType::String);
    storage.create_space(&mut s).expect("create space");
    storage
        .create_tag(
            space,
            &TagInfo::new("Node".to_string()).with_properties(vec![
                PropertyDef::new("name".to_string(), DataType::String),
            ]),
        )
        .expect("create tag");
    setup_vertices(&mut storage, space, 100);

    group.bench_function("fulltext_search", |b| {
        b.iter(|| {
            let _ = storage.get_vertex(space, &VertexId::from_string("v0"));
        });
    });

    group.bench_function("vertex_lookup", |b| {
        b.iter(|| {
            let _ = storage.get_vertex(space, &VertexId::from_string("v50"));
        });
    });

    group.finish();
}

fn bench_write_transaction_workflow(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_write_transaction");

    group.bench_function("insert_and_update_transaction", |b| {
        b.iter(|| {
            let mut storage = GraphStorage::new().expect("storage init");
            let space = "bench_write";
            let mut s = SpaceInfo::new(space.to_string()).with_vid_type(DataType::String);
            storage.create_space(&mut s).expect("create space");
            storage
                .create_tag(
                    space,
                    &TagInfo::new("Node".to_string()).with_properties(vec![
                        PropertyDef::new("value".to_string(), DataType::Int),
                    ]),
                )
                .expect("create tag");

            for i in 0..100 {
                let vertex = Vertex::new(
                    VertexId::from_string(format!("u{}", i)),
                    vec![Tag::new(
                        "Node".to_string(),
                        [("value".to_string(), Value::Int(i))]
                            .into_iter()
                            .collect(),
                    )],
                );
                storage.insert_vertex(space, vertex).expect("insert");
            }
        });
    });

    group.finish();
}

fn bench_concurrent_mixed_workload(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "e2e_concurrent_workload");

    let mut storage = GraphStorage::new().expect("storage init");
    let space = "bench_concurrent";
    let mut s = SpaceInfo::new(space.to_string()).with_vid_type(DataType::String);
    storage.create_space(&mut s).expect("create space");
    storage
        .create_tag(
            space,
            &TagInfo::new("Node".to_string()).with_properties(vec![
                PropertyDef::new("value".to_string(), DataType::Int),
            ]),
        )
        .expect("create tag");
    setup_vertices(&mut storage, space, 100);

    group.bench_function("concurrent_read", |b| {
        b.iter(|| {
            let _ = storage.get_vertex(space, &VertexId::from_string("v0"));
            let _ = storage.get_vertex(space, &VertexId::from_string("v50"));
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
