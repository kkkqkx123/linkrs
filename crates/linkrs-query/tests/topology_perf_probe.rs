//! Small comparative probe for the identity-scan topology gap.
//!
//! Compares a seed-only identity scan (empty flat projection) against flat
//! and boxed scans on narrow vs wide vertex schemas. If the seed-only scan
//! still decodes every property, its latency scales with schema width even
//! though it needs no properties.
//!
//! Run explicitly (ignored by default so normal test runs stay deterministic):
//!   cargo test -p linkrs-query --test topology_perf_probe -- --ignored --nocapture

use linkrs_core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Edge, Value, Vertex};
use linkrs_metrics::StatsManager;
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::optimizer::OptimizerEngine;
use linkrs_query::pipeline::QueryPipelineManager;
use linkrs_storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

const VERTEX_COUNT: i64 = 5000;
const WARMUP: usize = 2;
const ITERATIONS: usize = 11;

const SEED_COUNT_Q: &str = "MATCH (a:person)-[:knows]->(b:person) RETURN count(b)";
const FLAT_Q: &str = "MATCH (a:person) RETURN a.value";
const BOXED_Q: &str = "MATCH (a:person) RETURN a";
const EDGE_COUNT_Q: &str = "MATCH ()-[e:knows]->() RETURN count(e)";

fn narrow_props() -> Vec<PropertyDef> {
    vec![
        PropertyDef::new("value".into(), DataType::BigInt),
        PropertyDef::new("group_id".into(), DataType::BigInt),
    ]
}

fn wide_props() -> Vec<PropertyDef> {
    let mut props = narrow_props();
    for name in ["s1", "s2", "s3", "s4"] {
        props.push(PropertyDef::new(name.into(), DataType::String));
    }
    props
}

fn vertex_props(i: i64, wide: bool) -> HashMap<Arc<str>, Value> {
    let mut map: HashMap<Arc<str>, Value> = HashMap::new();
    map.insert("value".into(), Value::BigInt(i));
    map.insert("group_id".into(), Value::BigInt(i % 20));
    if wide {
        let pad = "x".repeat(32);
        for name in ["s1", "s2", "s3", "s4"] {
            let named: Arc<str> = Arc::from(name);
            map.insert(named, Value::string(format!("{name}-{i}-{pad}")));
        }
    }
    map
}

fn setup(wide: bool) -> (Arc<RwLock<GraphStorage>>, SpaceInfo) {
    let space_name = if wide { "probe_wide" } else { "probe_narrow" };
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(space_name.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");
    let props = if wide { wide_props() } else { narrow_props() };
    storage
        .create_tag(
            space_name,
            &TagInfo::new("person".to_string()).with_properties(props),
        )
        .expect("create tag");
    storage
        .create_edge_type(
            space_name,
            &EdgeTypeInfo::new("knows".to_string())
                .with_src_tag("person".to_string())
                .with_dst_tag("person".to_string()),
        )
        .expect("create edge type");

    let mut start = 0i64;
    while start < VERTEX_COUNT {
        let end = (start + 1000).min(VERTEX_COUNT);
        let vertices: Vec<Vertex> = (start..end)
            .map(|i| {
                Vertex::new(
                    VertexId::try_from_int64(i).expect("valid vertex id"),
                    Tag::new("person".to_string(), vertex_props(i, wide)),
                )
            })
            .collect();
        storage
            .batch_insert_vertices(space_name, vertices)
            .expect("insert vertices");
        start = end;
    }
    let edges: Vec<Edge> = (0..VERTEX_COUNT - 1)
        .map(|src| Edge {
            src: VertexId::try_from_int64(src).expect("valid vertex id"),
            dst: VertexId::try_from_int64(src + 1).expect("valid vertex id"),
            edge_type: "knows".to_string(),
            ranking: 0,
            props: HashMap::new(),
        })
        .collect();
    for chunk in edges.chunks(2000) {
        storage
            .batch_insert_edges(space_name, chunk.to_vec())
            .expect("insert edges");
    }
    let storage = Arc::new(RwLock::new(storage));
    let id = storage
        .read()
        .get_space_id(space_name)
        .expect("resolve space id");
    let mut info = SpaceInfo::new(space_name.to_string());
    info.space_id = id;
    (storage, info)
}

fn build_pipeline(storage: &Arc<RwLock<GraphStorage>>) -> QueryPipelineManager<GraphStorage> {
    let engine = OptimizerEngine::default();
    let stats = Arc::new(StatsManager::new());
    QueryPipelineManager::with_optimizer(storage.clone(), stats, Arc::new(engine))
}

fn run_once(
    pipeline: &mut QueryPipelineManager<GraphStorage>,
    query: &str,
    space: &SpaceInfo,
) -> ExecutionResult {
    pipeline
        .execute_query_with_space(query, Some(space.clone()))
        .expect("query should succeed")
}

fn median_ms(
    pipeline: &mut QueryPipelineManager<GraphStorage>,
    query: &str,
    space: &SpaceInfo,
) -> f64 {
    for _ in 0..WARMUP {
        run_once(pipeline, query, space);
    }
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        run_once(pipeline, query, space);
        samples.push(start.elapsed().as_micros() as f64 / 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite samples"));
    samples[ITERATIONS / 2]
}

fn count_of(result: &ExecutionResult) -> i64 {
    match result {
        ExecutionResult::DataSet { data, .. } => match data.rows.first().and_then(|r| r.first()) {
            Some(Value::BigInt(v)) => *v,
            Some(Value::Int(v)) => *v as i64,
            other => panic!("expected integer count, got {other:?}"),
        },
        other => panic!("expected dataset, got {other:?}"),
    }
}

#[test]
#[ignore]
fn topology_gap_probe() {
    for wide in [false, true] {
        let label = if wide {
            "wide(6 props)"
        } else {
            "narrow(2 props)"
        };
        let (storage, space) = setup(wide);
        let mut pipeline = build_pipeline(&storage);

        let seed = run_once(&mut pipeline, SEED_COUNT_Q, &space);
        assert_eq!(
            count_of(&seed),
            VERTEX_COUNT - 1,
            "seed count must match chain edges"
        );
        let edge = run_once(&mut pipeline, EDGE_COUNT_Q, &space);
        assert_eq!(
            count_of(&edge),
            VERTEX_COUNT - 1,
            "edge count must match chain edges"
        );
        let flat = run_once(&mut pipeline, FLAT_Q, &space);
        assert_eq!(flat.count(), VERTEX_COUNT as usize, "flat row count");
        let boxed = run_once(&mut pipeline, BOXED_Q, &space);
        assert_eq!(boxed.count(), VERTEX_COUNT as usize, "boxed row count");

        let seed_ms = median_ms(&mut pipeline, SEED_COUNT_Q, &space);
        let flat_ms = median_ms(&mut pipeline, FLAT_Q, &space);
        let boxed_ms = median_ms(&mut pipeline, BOXED_Q, &space);
        let edge_ms = median_ms(&mut pipeline, EDGE_COUNT_Q, &space);
        println!(
            "[topology-probe] schema={label} vertices={VERTEX_COUNT}\n  seed-count={seed_ms:.2}ms flat={flat_ms:.2}ms boxed={boxed_ms:.2}ms edge-count={edge_ms:.2}ms"
        );
    }
}
