//! Query stage isolation benchmark: parse / bind / plan+optimize / execute.
//!
//! Addresses the coverage gap in docs/tests/performance_hotspots_and_bench_gaps.md
//! §2.2: per-stage costs were previously only inferable from end-to-end totals.
//!
//! Stage strategy (from outside the query crate):
//! - parse:   `Parser::parse` directly (public, no context needed).
//! - bind:    `Binder::bind` directly (public, needs schema manager + space).
//! - execute: from `execute_query_with_profile`, whose `profile.stages`
//!   records parse_us / execute_us measured inside the pipeline.
//! - plan+optimize: derived as e2e_total - parse - execute.
//!
//! Small dataset (debug-mode friendly): 500 vertices, 4 edges/vertex.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::query::binder::Binder;
use graphdb::query::parser::Parser;
use graphdb::query::pipeline::QueryPipelineManager;
use graphdb::query::optimizer::OptimizerEngine;
use graphdb::query::QueryRequestContext;
use graphdb::storage::{GraphStorage, StorageReader, StorageSchemaContextOps, StorageSchemaOps, StorageWriter};
use graphdb_metrics::StatsManager;
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const VERTEX_COUNT: usize = 500;
const EDGES_PER_VERTEX: usize = 4;

const QUERIES: &[(&str, &str)] = &[
    ("point", "MATCH (n:Node) WHERE n.name == 'n42' RETURN n.name, n.value"),
    ("filter_scan", "MATCH (n:Node) WHERE n.value > 100.0 RETURN count(n)"),
    ("one_hop", "MATCH (a:Node)-[r:Link]->(b:Node) WHERE id(a) == 42 RETURN b.name, r.weight"),
    ("two_hop", "MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) WHERE id(a) == 0 RETURN count(c)"),
    ("aggregate", "MATCH (n:Node) RETURN sum(n.value)"),
];

/// Write a human-readable report into `benches/results/<bench_name>/`.
fn write_results_report(bench_name: &str, filename: &str, content: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("benches")
        .join("results")
        .join(bench_name);
    std::fs::create_dir_all(&dir).expect("create results dir");
    let path = dir.join(filename);
    std::fs::write(&path, content).expect("write results report");
    path
}

fn short_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut group = c.benchmark_group(name);
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(500));
    group
}

fn setup_graph() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let space_name = "stage_bench".to_string();
    let mut space = SpaceInfo::new(space_name.clone()).with_vid_type(DataType::String);
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
            &EdgeTypeInfo::new("Link".to_string()).with_properties(vec![PropertyDef::new(
                "weight".to_string(),
                DataType::Double,
            )]),
        )
        .expect("create edge type");

    for i in 0..VERTEX_COUNT {
        let vertex = Vertex::new(
            VertexId::try_from_string(format!("n{}", i)).expect("valid vertex id"),
            Tag::new(
                "Node".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("n{}", i))),
                    ("value".to_string(), Value::Double(i as f64 * 0.1)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        storage
            .insert_vertex(&space_name, vertex)
            .expect("insert vertex");
    }

    for src in 0..VERTEX_COUNT {
        for k in 1..=EDGES_PER_VERTEX {
            let dst = (src + k) % VERTEX_COUNT;
            let edge = Edge {
                src: VertexId::try_from_string(format!("n{}", src)).expect("valid vertex id"),
                dst: VertexId::try_from_string(format!("n{}", dst)).expect("valid vertex id"),
                edge_type: "Link".to_string(),
                ranking: 0,
                props: [("weight".to_string(), Value::Double(1.0 / k as f64))]
                    .into_iter()
                    .collect(),
            };
            storage.insert_edge(&space_name, edge).expect("insert edge");
        }
    }
    storage
}

fn bench_query_stages(c: &mut Criterion) {
    let storage = Arc::new(RwLock::new(setup_graph()));
    let schema_manager = StorageSchemaContextOps::get_schema_manager(&*storage.read())
        .expect("schema manager");
    let space = storage
        .read()
        .get_space("stage_bench")
        .expect("get space")
        .expect("space exists");
    let mut pipeline = QueryPipelineManager::with_optimizer(
        storage.clone(),
        Arc::new(StatsManager::new()),
        Arc::new(OptimizerEngine::default()),
    )
    .with_schema_manager(schema_manager.clone());

    let mut report = String::from("query stage isolation (debug build)\n");
    report.push_str(&format!(
        "dataset: {} vertices x {} edges/vertex\n\n",
        VERTEX_COUNT, EDGES_PER_VERTEX
    ));
    report.push_str(
        "parse and bind are measured directly (also in criterion groups above);\n\
         plan+optimize+execute is the e2e remainder from the stream entry.\n\n",
    );
    report.push_str("| query | parse | bind | plan+optimize+execute (derived) | e2e total |\n");
    report.push_str("|---|---|---|---|---|\n");

    for (label, query) in QUERIES {
        let group_name = format!("stage_{}", label);

        // Stage 1: parse only.
        {
            let mut group = short_group(c, &group_name);
            group.bench_function("1_parse", |b| {
                b.iter(|| {
                    let result = Parser::new(query).parse().expect("parse ok");
                    black_box(result.ast);
                });
            });
        }

        // Stage 2: bind only (AST parsed once outside the timed loop).
        {
            let ast = Parser::new(query).parse().expect("parse ok").ast;
            let mut group = short_group(c, &group_name);
            group.bench_function("2_bind", |b| {
                b.iter(|| {
                    let binder = Binder::new()
                        .with_space(Some(space.space_name.clone()), space.space_id)
                        .with_schema_manager(schema_manager.clone());
                    let bound = binder.bind(ast.clone()).expect("bind ok");
                    black_box(bound);
                });
            });
        }

        // Stages 3+4: end-to-end through the pipeline.
        {
            let mut group = short_group(c, &group_name);
            group.bench_function("4_e2e_profiled", |b| {
                b.iter(|| {
                    let result = env_execute(&mut pipeline, query, &space);
                    black_box(result);
                });
            });
        }

        // Collect per-stage numbers for the report (outside criterion timing).
        // The profiled entry (`execute_query_with_profile`) resolves the space
        // only from the request context, which we cannot set from outside the
        // crate, so the report uses: parse (direct), bind (direct, criterion),
        // and e2e wall via the stream entry; plan+optimize+execute is derived
        // as the remainder.
        let iters = 30;
        let mut wall_total = Duration::ZERO;
        let mut parse_ns = 0u128;
        let mut bind_ns = 0u128;
        for _ in 0..iters {
            let start = Instant::now();
            let p = Parser::new(query).parse().expect("parse ok");
            parse_ns += start.elapsed().as_nanos();
            let start = Instant::now();
            let binder = Binder::new()
                .with_space(Some(space.space_name.clone()), space.space_id)
                .with_schema_manager(schema_manager.clone());
            let _bound = binder.bind(p.ast).expect("bind ok");
            bind_ns += start.elapsed().as_nanos();

            let start = Instant::now();
            env_execute(&mut pipeline, query, &space);
            wall_total += start.elapsed();
        }
        {
            let parse_ms = parse_ns as f64 / iters as f64 / 1.0e6;
            let bind_ms = bind_ns as f64 / iters as f64 / 1.0e6;
            let total_ms = wall_total.as_secs_f64() * 1000.0 / iters as f64;
            let rest_ms = (total_ms - parse_ms - bind_ms).max(0.0);
            report.push_str(&format!(
                "| {} | {:.3} | {:.3} | {:.3} | {:.3} |\n",
                label, parse_ms, bind_ms, rest_ms, total_ms
            ));
        }
    }

    let path = write_results_report("query_stage_bench", "results.txt", &report);
    println!("report written to {}", path.display());
}

fn env_execute(
    pipeline: &mut QueryPipelineManager<GraphStorage>,
    query: &str,
    space: &SpaceInfo,
) -> usize {
    let rctx = Arc::new(QueryRequestContext::new(query.to_string()));
    let result = pipeline
        .execute_query_stream_with_request(query, rctx, Some(space.clone()))
        .expect("query should succeed");
    let mut rows = 0usize;
    while let Some(chunk) = result.next_chunk().expect("chunk ok") {
        rows += chunk.len();
    }
    result.close().ok();
    rows
}

criterion_group!(benches, bench_query_stages);
criterion_main!(benches);
