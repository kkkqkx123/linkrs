//! A2 traversal-path performance validation.
//!
//! Measures the anchored 1-hop and unanchored 2-hop on a 100k-vertex x 3-edge
//! dataset through the full pipeline.  Targets (see
//! `docs/issue/traversal-query-pathology.md`):
//!   anchored 1-hop  <= 100ms
//!   unanchored 2-hop <= 2s
//!
//! Run with:
//!   cargo bench --bench traversal_perf_bench

use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, StatsManager, Value, Vertex};
use graphdb::query::optimizer::{OptimizerEngine, PartitioningConfig};
use graphdb::query::pipeline::QueryPipelineManager;
use graphdb::storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

const SPACE: &str = "tv";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const VERTEX_COUNT: u64 = 100_000;
const ITERATIONS: usize = 7;
const SLOW_ITERATIONS: usize = 2;

const Q1: &str = "MATCH (a:Node)-[:Link]->(b:Node) WHERE a.value < 100 RETURN count(b)";
const Q2: &str = "MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)";

fn setup_data() -> Arc<RwLock<GraphStorage>> {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string()).with_properties(vec![
                PropertyDef::new("value".to_string(), DataType::BigInt),
                PropertyDef::new("group_id".to_string(), DataType::BigInt),
            ]),
        )
        .expect("create tag");
    storage
        .create_edge_type(
            SPACE,
            &EdgeTypeInfo::new(EDGE.to_string())
                .with_src_tag(TAG.to_string())
                .with_dst_tag(TAG.to_string()),
        )
        .expect("create edge type");

    let mut start = 0usize;
    while start < VERTEX_COUNT as usize {
        let end = (start + 20_000).min(VERTEX_COUNT as usize);
        let vertices: Vec<Vertex> = (start..end)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64(i as i64),
                    vec![Tag::new(
                        TAG.to_string(),
                        vec![
                            ("value".to_string(), Value::BigInt(i as i64)),
                            ("group_id".to_string(), Value::BigInt((i % 20) as i64)),
                        ]
                        .into_iter()
                        .collect(),
                    )],
                )
            })
            .collect();
        storage
            .batch_insert_vertices(SPACE, vertices)
            .expect("batch insert vertices");
        start = end;
    }

    let mut edges = Vec::with_capacity((VERTEX_COUNT * 3) as usize);
    for src in 0..VERTEX_COUNT as i64 {
        for k in 1..=3i64 {
            edges.push(Edge {
                src: VertexId::from_int64(src),
                dst: VertexId::from_int64((src + k) % VERTEX_COUNT as i64),
                edge_type: EDGE.to_string(),
                ranking: 0,
                props: Default::default(),
            });
        }
    }
    for chunk in edges.chunks(100_000) {
        storage
            .batch_insert_edges(SPACE, chunk.to_vec())
            .expect("batch insert edges");
    }
    Arc::new(RwLock::new(storage))
}

fn build_pipeline(
    storage: &Arc<RwLock<GraphStorage>>,
    workers: usize,
) -> QueryPipelineManager<GraphStorage> {
    let mut engine = OptimizerEngine::default();
    if workers > 1 {
        engine.set_partitioning_config(PartitioningConfig {
            enabled: true,
            min_rows_per_partition: (VERTEX_COUNT as usize / workers).max(1) as u64,
            max_partitions: workers,
            vertex_id_range: Some(0i64..VERTEX_COUNT as i64),
            max_workers: workers,
            max_buffered_chunks: 10,
        });
    }
    let stats = Arc::new(StatsManager::new());
    let pipeline = QueryPipelineManager::with_optimizer(storage.clone(), stats, Arc::new(engine));
    pipeline
        .collect_statistics(SPACE, true)
        .expect("collect statistics");
    pipeline
}

fn space_info(storage: &Arc<RwLock<GraphStorage>>) -> SpaceInfo {
    let id = storage
        .read()
        .get_space_id(SPACE)
        .expect("resolve space id");
    let mut info = SpaceInfo::new(SPACE.to_string());
    info.space_id = id;
    info
}

fn run(pipeline: &mut QueryPipelineManager<GraphStorage>, query: &str, space: &SpaceInfo) {
    pipeline
        .execute_query_with_space(query, Some(space.clone()))
        .expect("query should succeed");
}

fn explain(pipeline: &mut QueryPipelineManager<GraphStorage>, query: &str, space: &SpaceInfo) {
    if let Ok(graphdb::query::executor::base::ExecutionResult::DataSet { data, .. }) =
        pipeline.execute_query_with_space(&format!("EXPLAIN ANALYZE {query}"), Some(space.clone()))
    {
        if let Some(row) = data.rows.first() {
            if let Some(Value::String(text)) = row.first() {
                println!("--- EXPLAIN ANALYZE:\n{text}");
            }
        }
    }
}

fn median_us(samples: &[u64]) -> u64 {
    let mut v = samples.to_vec();
    v.sort_unstable();
    v[v.len() / 2]
}

fn main() {
    println!("== A2/E4 traversal performance validation ==");
    println!("vertices={VERTEX_COUNT}, edges/vertex=3, iterations={ITERATIONS}");
    let storage = setup_data();
    println!("data ready");
    let space = space_info(&storage);

    for (name, query, target_ms) in [("anchored 1-hop", Q1, 100), ("unanchored 2-hop", Q2, 2000)] {
        for workers in [1usize, 2, 4] {
            let mut pipeline = build_pipeline(&storage, workers);
            for _ in 0..2 {
                run(&mut pipeline, query, &space);
            }
            if workers == 1 && name == "unanchored 2-hop" {
                explain(&mut pipeline, query, &space);
            }
            let iterations = if name == "unanchored 2-hop" {
                SLOW_ITERATIONS
            } else {
                ITERATIONS
            };
            let mut samples = Vec::with_capacity(iterations);
            for _ in 0..iterations {
                let start = Instant::now();
                run(&mut pipeline, query, &space);
                samples.push(start.elapsed().as_micros() as u64);
            }
            let median = median_us(&samples) / 1000;
            let workers_label = if workers == 1 {
                "serial".to_string()
            } else {
                format!("{workers} workers")
            };
            println!(
                "{name} ({workers_label}): {query}\n  median = {median} ms (target <= {target_ms} ms) {}",
                if median <= target_ms {
                    "PASS"
                } else {
                    "FAIL"
                }
            );
        }
    }
}
