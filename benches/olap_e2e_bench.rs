//! End-to-end OLAP benchmarks over a synthetic scale-free graph.
//!
//! Establishes the reproducible baseline required before judging any OLAP
//! improvement (columnar scans, vectorization, factorization). The dataset is
//! generated with a fixed-seed preferential-attachment model so every run sees
//! identical data. Scale via `OLAP_BENCH_SCALE` (multiplier on the base vertex
//! count), e.g. `OLAP_BENCH_SCALE=5 cargo bench --bench olap_e2e_bench`.
//!
//! Query families:
//!   Q1  two-hop traversal count (unanchored, full graph)
//!   Q2  two-hop traversal count (anchored on one vertex)
//!   Q3  expand + group aggregate TopN (`RETURN key, count(*) ORDER BY ... LIMIT`)
//!   Q4  edge-scan with range predicate (storage-side pushdown path)
//!   Q5  vertex-scan with range predicate (50% selectivity)
//!
//! Run with: cargo bench --bench olap_e2e_bench

use std::sync::Arc;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use parking_lot::RwLock;

use graphdb::core::stats::StatsManager;
use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::query::optimizer::OptimizerEngine;
use graphdb::query::pipeline::QueryPipelineManager;
use graphdb::query::QueryRequestContext;
use graphdb::storage::{
    GraphStorage, StorageReader, StorageSchemaContextOps, StorageSchemaOps, StorageWriter,
};

const SPACE: &str = "olap_e2e";
const TAG: &str = "Person";
const EDGE_TYPE: &str = "Knows";
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;
const BASE_VERTICES: u64 = 5_000;
/// Out-edges per newly attached vertex (BA model parameter m).
const M_PER_VERTEX: usize = 5;
const BATCH: usize = 10_000;

fn scale() -> u64 {
    std::env::var("OLAP_BENCH_SCALE")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1)
        .max(1)
}

fn vertex_count() -> u64 {
    BASE_VERTICES * scale()
}

/// xorshift64*: tiny deterministic PRNG, avoids external rand dependency.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound.max(1)
    }
}

type SharedStorage = Arc<RwLock<GraphStorage>>;

static STORAGE: std::sync::OnceLock<SharedStorage> = std::sync::OnceLock::new();

fn storage() -> SharedStorage {
    STORAGE.get_or_init(build_storage).clone()
}

/// Preferential-attachment graph: vertices attach to earlier vertices with
/// probability proportional to current degree, yielding a power-law out-degree
/// distribution with hub vertices that stress Expand and CSR paths.
fn build_storage() -> SharedStorage {
    let n = vertex_count();
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");

    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string()).with_properties(vec![
                PropertyDef::new("name".to_string(), DataType::String),
                PropertyDef::new("value".to_string(), DataType::BigInt),
                PropertyDef::new("bucket".to_string(), DataType::Int),
            ]),
        )
        .expect("create tag");
    storage
        .create_edge_type(
            SPACE,
            &EdgeTypeInfo::new(EDGE_TYPE.to_string()).with_properties(vec![PropertyDef::new(
                "weight".to_string(),
                DataType::Double,
            )]),
        )
        .expect("create edge type");

    // Vertices, inserted in deterministic batches.
    let mut rng = Rng(SEED);
    for start in (0..n).step_by(BATCH) {
        let end = (start + BATCH as u64).min(n);
        let vertices: Vec<Vertex> = (start..end)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64(i as i64),
                    vec![Tag::new(
                        TAG.to_string(),
                        [
                            ("name".to_string(), Value::string(format!("p{}", i))),
                            ("value".to_string(), Value::BigInt((i * 7919 % n) as i64)),
                            ("bucket".to_string(), Value::Int((i % 32) as i32)),
                        ]
                        .into_iter()
                        .collect(),
                    )],
                )
            })
            .collect();
        StorageWriter::batch_insert_vertices(&mut storage, SPACE, vertices)
            .expect("batch insert vertices");
    }

    // Edges: vertex i attaches to m earlier vertices chosen proportionally to
    // degree via a repeated-endpoint candidate list.
    let mut candidates: Vec<u32> = Vec::with_capacity((n as usize) * M_PER_VERTEX);
    let mut edges: Vec<Edge> = Vec::with_capacity(BATCH * M_PER_VERTEX);
    for i in 0..n {
        let attachments = if i == 0 {
            0
        } else {
            M_PER_VERTEX.min(i as usize)
        };
        // Per-vertex dedup: the storage rejects a repeated (src,dst) pair.
        let mut attached: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut attempts = 0usize;
        while attached.len() < attachments && attempts < attachments * 8 {
            attempts += 1;
            // Preferential attachment once the candidate list is populated;
            // uniform random attachment for the first edge.
            let dst = if candidates.is_empty() {
                rng.below(i)
            } else {
                candidates[rng.below(candidates.len() as u64) as usize] as u64
            };
            if !attached.insert(dst) {
                continue;
            }
            edges.push(Edge {
                src: VertexId::from_int64(i as i64),
                dst: VertexId::from_int64(dst as i64),
                edge_type: EDGE_TYPE.to_string(),
                ranking: 0,
                props: [(
                    "weight".to_string(),
                    Value::Double((rng.next_u64() % 1000) as f64 / 1000.0),
                )]
                .into_iter()
                .collect(),
            });
            candidates.push(dst as u32);
        }
        for _ in 0..attachments {
            candidates.push(i as u32);
        }
        if edges.len() >= BATCH * M_PER_VERTEX || i + 1 == n {
            let batch = std::mem::take(&mut edges);
            StorageWriter::batch_insert_edges(&mut storage, SPACE, batch)
                .expect("batch insert edges");
        }
    }

    Arc::new(RwLock::new(storage))
}

fn pipeline() -> (QueryPipelineManager<GraphStorage>, SpaceInfo, SharedStorage) {
    let storage = storage();
    let stats_manager = Arc::new(StatsManager::new());
    let schema_manager = {
        let guard = storage.read();
        StorageSchemaContextOps::get_schema_manager(&*guard).expect("schema manager")
    };
    let pipeline = QueryPipelineManager::with_optimizer(
        storage.clone(),
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    )
    .with_schema_manager(schema_manager);
    let space_info = {
        let guard = storage.read();
        guard.get_space(SPACE).expect("space").expect("space info")
    };
    (pipeline, space_info, storage)
}

/// Execute one query, drain all output chunks, return total row count.
fn run_query(
    pipeline: &mut QueryPipelineManager<GraphStorage>,
    query: &str,
    space: &SpaceInfo,
) -> usize {
    let rctx = Arc::new(QueryRequestContext::new(query.to_string()));
    let result = pipeline
        .execute_query_stream_with_request(query, rctx, Some(space.clone()))
        .unwrap_or_else(|e| panic!("query failed: {e}: {query}"));
    let mut rows = 0usize;
    while let Ok(Some(chunk)) = result.next_chunk() {
        rows += chunk.len();
    }
    result.close().ok();
    rows
}

fn dataset_summary(storage: &SharedStorage) -> String {
    let guard = storage.read();
    let vertices = guard.count_vertices_by_tag(SPACE, TAG).expect("count");
    let edges = guard.count_edges_by_type(SPACE, EDGE_TYPE).expect("count");
    format!(
        "dataset: {} vertices / {} edges (scale={})",
        vertices,
        edges,
        scale()
    )
}

fn bench_q1_two_hop_unanchored(c: &mut Criterion) {
    let mut group = c.benchmark_group("olap_q1_two_hop_unanchored");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(20);
    let (mut pipeline, space, storage) = pipeline();
    println!("{}", dataset_summary(&storage));
    let query = "MATCH (a:Person)-[:Knows]->(b:Person)-[:Knows]->(c:Person) RETURN count(c)";
    println!(
        "q1 baseline rows: {:?}",
        run_query(&mut pipeline, query, &space)
    );
    group.bench_function(BenchmarkId::from_parameter(vertex_count()), |b| {
        b.iter(|| black_box(run_query(&mut pipeline, query, &space)))
    });
    group.finish();
}

fn bench_q2_two_hop_anchored(c: &mut Criterion) {
    let mut group = c.benchmark_group("olap_q2_two_hop_anchored");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(30);
    let (mut pipeline, space, _) = pipeline();
    // Anchor on the newest vertex: it has M_PER_VERTEX out-edges, while early
    // BA vertices only have in-edges.
    let anchor = vertex_count() - 1;
    let query = format!(
        "MATCH (a:Person)-[:Knows]->(b:Person)-[:Knows]->(c:Person) \
         WHERE id(a) == {anchor} RETURN count(c)"
    );
    group.bench_function(BenchmarkId::from_parameter(vertex_count()), |b| {
        b.iter(|| black_box(run_query(&mut pipeline, &query, &space)))
    });
    group.finish();
}

fn bench_q3_group_aggregate_topn(c: &mut Criterion) {
    let mut group = c.benchmark_group("olap_q3_group_aggregate_topn");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(30);
    let (mut pipeline, space, _) = pipeline();
    let query = "MATCH (a:Person)-[:Knows]->(b:Person) \
                 RETURN b.bucket AS grp, count(*) AS cnt \
                 ORDER BY cnt DESC LIMIT 10";
    group.bench_function(BenchmarkId::from_parameter(vertex_count()), |b| {
        b.iter(|| black_box(run_query(&mut pipeline, query, &space)))
    });
    group.finish();
}

fn bench_q4_filtered_edge_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("olap_q4_filtered_edge_scan");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(30);
    let (mut pipeline, space, _) = pipeline();
    let query = "MATCH ()-[r:Knows]->() WHERE r.weight > 0.5 RETURN count(r)";
    group.bench_function(BenchmarkId::from_parameter(vertex_count()), |b| {
        b.iter(|| black_box(run_query(&mut pipeline, query, &space)))
    });
    group.finish();
}

fn bench_q5_filtered_vertex_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("olap_q5_filtered_vertex_scan");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(30);
    let (mut pipeline, space, _) = pipeline();
    let half = (vertex_count() / 2) as i64;
    let query = format!("MATCH (n:Person) WHERE n.value > {half} RETURN count(n)");
    group.bench_function(BenchmarkId::from_parameter(vertex_count()), |b| {
        b.iter(|| black_box(run_query(&mut pipeline, &query, &space)))
    });
    group.finish();
}

criterion_group!(
    olap_benches,
    bench_q1_two_hop_unanchored,
    bench_q2_two_hop_anchored,
    bench_q3_group_aggregate_topn,
    bench_q4_filtered_edge_scan,
    bench_q5_filtered_vertex_scan,
);
criterion_main!(olap_benches);
