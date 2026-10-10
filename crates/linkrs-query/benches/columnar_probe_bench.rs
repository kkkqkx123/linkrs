//! Columnar probe-gate benchmark: seed count, flat projection,
//! narrow vs wide edge count.
//!
//! Zero product-code changes: this binary only drives the public pipeline
//! entry on a fixed synthetic dataset and records wall-time medians plus
//! allocated bytes per probe. The gate decision (is the remaining bottleneck
//! cross-operator row materialization?) is made from the report, not here.
//!
//! Run with the release-like bench profile on a quiet machine:
//! `cargo bench -p linkrs-query --bench columnar_probe_bench`

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use linkrs_core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Edge, Value, Vertex};
use linkrs_metrics::StatsManager;
use linkrs_query::optimizer::OptimizerEngine;
use linkrs_query::parser::Parser;
use linkrs_query::pipeline::QueryPipelineManager;
use linkrs_query::QueryRequestContext;
use linkrs_storage::{
    GraphStorage, StorageReader, StorageSchemaContextOps, StorageSchemaOps, StorageWriter,
};
use parking_lot::RwLock;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[path = "results_report.rs"]
mod report;
use report::write_results_report;

struct CountingAllocator;

static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

const VERTEX_COUNT: usize = 5_000;
const EDGES_PER_VERTEX: usize = 4;
const EDGE_COUNT: usize = VERTEX_COUNT * EDGES_PER_VERTEX;
const MEDIAN_ITERS: usize = 31;

const PROBES: &[(&str, &str, usize)] = &[
    ("seed_count", "MATCH (n:Node) RETURN count(n)", VERTEX_COUNT),
    (
        "flat_projection",
        "MATCH (n:Node) RETURN n.name",
        VERTEX_COUNT,
    ),
    (
        "edge_count_narrow",
        "MATCH ()-[r:Link]->() RETURN count(r)",
        EDGE_COUNT,
    ),
    ("edge_count_wide", "LOOKUP ON Link", EDGE_COUNT),
    // Fixed two-hop chain: every vertex fans out to EDGES_PER_VERTEX
    // neighbors per hop with duplicates preserved, so the count is exact.
    (
        "two_hop",
        "MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)",
        VERTEX_COUNT * EDGES_PER_VERTEX * EDGES_PER_VERTEX,
    ),
];

fn setup_graph() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let space_name = "probe_bench".to_string();
    let mut space = SpaceInfo::new(space_name.clone()).with_vid_type(DataType::String);
    storage.create_space(&mut space).expect("create space");

    storage
        .create_tag(
            &space_name,
            &TagInfo::new("Node".to_string()).with_properties(vec![
                PropertyDef::new("name".into(), DataType::String),
                PropertyDef::new("value".into(), DataType::Double),
            ]),
        )
        .expect("create tag");

    storage
        .create_edge_type(
            &space_name,
            &EdgeTypeInfo::new("Link".to_string())
                .with_src_tag("Node".to_string())
                .with_dst_tag("Node".to_string())
                .with_properties(vec![PropertyDef::new(
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
                    ("name".into(), Value::string(format!("n{}", i))),
                    ("value".into(), Value::Double(i as f64 * 0.1)),
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
                props: [("weight".into(), Value::Double(1.0 / k as f64))]
                    .into_iter()
                    .collect(),
            };
            storage.insert_edge(&space_name, edge).expect("insert edge");
        }
    }
    storage
}

fn run_query(
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

fn run_count_value(
    pipeline: &mut QueryPipelineManager<GraphStorage>,
    query: &str,
    space: &SpaceInfo,
) -> usize {
    let rctx = Arc::new(QueryRequestContext::new(query.to_string()));
    let result = pipeline
        .execute_query_stream_with_request(query, rctx, Some(space.clone()))
        .expect("query should succeed");
    let mut value = 0usize;
    let mut seen = false;
    while let Some(chunk) = result.next_chunk().expect("chunk ok") {
        if !seen {
            if let Some(cell) = chunk.rows.first().and_then(|row| row.first()) {
                value = match cell {
                    Value::BigInt(v) => *v as usize,
                    Value::Int(v) => *v as usize,
                    other => panic!("expected integer count, got {other:?}"),
                };
                seen = true;
            }
        }
    }
    result.close().ok();
    assert!(seen, "count probe must emit one row");
    value
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    values[values.len() / 2]
}

fn bench_probes(c: &mut Criterion) {
    let storage = Arc::new(RwLock::new(setup_graph()));
    let space = storage
        .read()
        .get_space("probe_bench")
        .expect("get space")
        .expect("space exists");
    let schema_manager =
        StorageSchemaContextOps::get_schema_manager(&*storage.read()).expect("schema manager");
    let mut pipeline = QueryPipelineManager::with_optimizer(
        storage.clone(),
        Arc::new(StatsManager::new()),
        Arc::new(OptimizerEngine::default()),
    )
    .with_schema_manager(schema_manager.clone());

    use linkrs_query::binder::Binder;

    let mut report = String::from("columnar probe gate (bench profile)\n");
    report.push_str(&format!(
        "dataset: {} vertices, {} edges, string vids\n",
        VERTEX_COUNT, EDGE_COUNT
    ));
    report.push_str(&format!(
        "manual median loop: {} iters per probe\n\n",
        MEDIAN_ITERS
    ));
    report
        .push_str("| probe | rows/value | median wall ms | median alloc bytes | parse+bind ms |\n");
    report.push_str("|---|---|---|---|---|\n");

    for (label, query, expected) in PROBES {
        let mut group = c.benchmark_group("probe");
        group.measurement_time(Duration::from_secs(2));
        group.sample_size(20);
        group.warm_up_time(Duration::from_millis(500));
        group.bench_function(*label, |b| {
            b.iter(|| {
                let rows = run_query(&mut pipeline, query, &space);
                black_box(rows);
            });
        });
        group.finish();

        // Frontend share measured directly (parse + bind, outside the
        // timed execute path), so the execution-dominated remainder is
        // visible per probe.
        let mut front_ms = Vec::with_capacity(MEDIAN_ITERS);
        for _ in 0..MEDIAN_ITERS {
            let start = Instant::now();
            let parsed = Parser::new(query).parse().expect("parse ok");
            let binder = Binder::new()
                .with_space(Some(space.space_name.clone()), space.space_id)
                .with_schema_manager(schema_manager.clone());
            black_box(binder.bind(parsed.ast).expect("bind ok"));
            front_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        }

        let mut walls = Vec::with_capacity(MEDIAN_ITERS);
        let mut allocs = Vec::with_capacity(MEDIAN_ITERS);
        for _ in 0..MEDIAN_ITERS {
            ALLOCATED_BYTES.store(0, Ordering::Relaxed);
            let start = Instant::now();
            let rows = run_query(&mut pipeline, query, &space);
            walls.push(start.elapsed().as_secs_f64() * 1000.0);
            allocs.push(ALLOCATED_BYTES.load(Ordering::Relaxed) as f64);
            if *label == "flat_projection" || *label == "edge_count_wide" {
                assert_eq!(rows, *expected, "probe {label} row count changed");
            }
            if *label == "seed_count" || *label == "edge_count_narrow" || *label == "two_hop" {
                assert_eq!(
                    run_count_value(&mut pipeline, query, &space),
                    *expected,
                    "probe {label} count value changed"
                );
            }
        }
        report.push_str(&format!(
            "| {} | {} | {:.3} | {:.0} | {:.3} |\n",
            label,
            expected,
            median(walls),
            median(allocs),
            median(front_ms)
        ));
    }

    let path = write_results_report("columnar_probe_bench", "results.txt", &report);
    println!("report written to {}", path.display());
}

criterion_group!(benches, bench_probes);
criterion_main!(benches);
