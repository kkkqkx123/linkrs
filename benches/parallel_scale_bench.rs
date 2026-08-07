//! Phase 3 (P3.2) parallel speedup validation: end-to-end Cypher queries at
//! 1/2/4/8 workers through the full pipeline (parse -> optimize -> partition
//! -> execute -> profile).
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench parallel_scale_bench
//!
//! Outputs T(n) medians, speedup E(n) = T(1)/T(n), parallel efficiency
//! eta(n) = parallel_work_time/parallel_wall_time, actual worker count and
//! fallback reason from `EXPLAIN ANALYZE`, plus storage-read share R.

use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, StatsManager, Value, Vertex};
use graphdb::query::optimizer::{OptimizerEngine, PartitioningConfig};
use graphdb::query::pipeline::QueryPipelineManager;
use graphdb::storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

const SPACE: &str = "pb";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const VERTEX_COUNT: u64 = 100_000;
const WORKERS: [usize; 4] = [1, 2, 4, 8];
const ITERATIONS: usize = 11;

const Q1: &str = "MATCH (n:Node) WHERE n.value < 50000 RETURN count(n)";
const Q2: &str = "MATCH (n:Node) RETURN n.group_id, count(*)";
const Q3: &str = "MATCH (a:Node)-[:Link]->(b:Node) WHERE a.value < 100 RETURN count(b)";
// E1: multi-scan (independent tagged vertex scans, E1a) union.
const E1: &str = "MATCH (a:Node) WHERE a.value < 50000 RETURN a.group_id \
                  UNION ALL \
                  MATCH (b:Node) WHERE b.value >= 50000 RETURN b.group_id";
// E2: pure edge-table scan partitioned by src-id ranges (E2).
const E2: &str = "LOOKUP ON EDGE Link YIELD Link.src";

#[derive(Debug, Default, Clone)]
struct ExplainMetrics {
    requested_workers: usize,
    actual_workers: usize,
    parallel_wall_us: u64,
    parallel_work_us: u64,
    fallback_reason: String,
    storage_read_us: u64,
    /// Sum of per-node times (each cumulative); used for the storage share
    /// denominator via the root (max) node time instead.
    total_operator_us: u64,
    /// Max per-node cumulative time = root node total (children included).
    root_total_us: u64,
}

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
            // Size partitions so the desired partition count matches the
            // worker count: desired = rows / min_rows (clamped to 2..=max_partitions).
            min_rows_per_partition: (VERTEX_COUNT as usize / workers).max(1) as u64,
            max_partitions: workers,
            vertex_id_range: Some(0i64..VERTEX_COUNT as i64),
            max_workers: workers,
            max_buffered_chunks: 10,
        });
    } else {
        engine.set_partitioning_config(PartitioningConfig {
            max_workers: 1,
            ..PartitioningConfig::default()
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

fn run_query(
    pipeline: &mut QueryPipelineManager<GraphStorage>,
    query: &str,
    space: &SpaceInfo,
) -> Result<(), String> {
    pipeline
        .execute_query_with_space(query, Some(space.clone()))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn median_us(samples: &[u64]) -> u64 {
    let mut v = samples.to_vec();
    v.sort_unstable();
    v[v.len() / 2]
}

/// Parse the `Parallel Workers: ...` header line of the TABLE-format explain
/// output (this header is not truncated).
fn parse_explain_table_header(output: &str) -> ExplainMetrics {
    let mut metrics = ExplainMetrics::default();
    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Parallel Workers: requested=") {
            for token in rest.split(',') {
                let token = token.trim();
                if let Some(v) = token.strip_prefix("actual=") {
                    metrics.actual_workers = v.parse().unwrap_or(0);
                } else if let Some(v) = token.strip_prefix("wall_time=") {
                    metrics.parallel_wall_us = v.trim_end_matches("us").parse().unwrap_or(0);
                } else if let Some(v) = token.strip_prefix("work_time=") {
                    metrics.parallel_work_us = v.trim_end_matches("us").parse().unwrap_or(0);
                }
            }
            metrics.requested_workers = rest
                .split(',')
                .next()
                .unwrap_or("0")
                .trim()
                .parse()
                .unwrap_or(0);
            if let Some(idx) = output.find("fallback_reason=\"") {
                let tail = &output[idx + "fallback_reason=\"".len()..];
                metrics.fallback_reason = tail.split('"').next().unwrap_or("").to_string();
            }
        }
    }
    metrics
}

/// Parse `EXPLAIN ANALYZE <query> FORMAT = DOT` output. DOT labels are not
/// truncated (unlike the table format) so per-node times are recoverable.
fn parse_explain_dot(output: &str) -> ExplainMetrics {
    let mut metrics = ExplainMetrics::default();
    for line in output.lines() {
        let line = line.trim();
        if let Some(label) = line.strip_prefix("label=\"Execution Model: Batch Pull | ") {
            let inner = label.trim_end_matches('"');
            let parts: Vec<&str> = inner.split(" | ").collect();
            let parallel = *parts.get(1).unwrap_or(&"");
            if let Some(idx) = parallel.find("requested=") {
                metrics.requested_workers = parallel[idx + "requested=".len()..]
                    .split(|c: char| !c.is_ascii_digit())
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
            }
            if let Some(idx) = parallel.find("actual=") {
                metrics.actual_workers = parallel[idx + "actual=".len()..]
                    .split(|c: char| !c.is_ascii_digit())
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
            }
            if let Some(idx) = parallel.find("wall=") {
                metrics.parallel_wall_us = parallel[idx + "wall=".len()..]
                    .split(|c: char| !c.is_ascii_digit())
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
            }
            if let Some(idx) = parallel.find("fallback (requested") {
                let tail = &parallel[idx + "fallback (requested=".len()..];
                metrics.fallback_reason = tail.split(')').next().unwrap_or("").trim().to_string();
            }
        } else if let Some(idx) = line.find("[label=\"") {
            let after = &line[idx + "[label=\"".len()..];
            let mut label_parts = after.split('\\');
            let name = label_parts.next().unwrap_or("").to_string();
            let mut time = 0u64;
            for part in label_parts {
                if let Some(t) = part.strip_prefix("ntime: ") {
                    time = t
                        .split(|c: char| !c.is_ascii_digit())
                        .next()
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0);
                    break;
                }
            }
            metrics.total_operator_us += time;
            metrics.root_total_us = metrics.root_total_us.max(time);
            if name.contains("ScanVertices")
                || name.contains("ScanEdges")
                || name.contains("GetVertices")
                || name.contains("AppendVertices")
                || name.contains("Expand")
            {
                metrics.storage_read_us += time;
            }
        }
    }
    metrics
}

fn measure(
    pipeline: &mut QueryPipelineManager<GraphStorage>,
    query: &str,
    space: &SpaceInfo,
) -> (u64, ExplainMetrics) {
    for _ in 0..3 {
        run_query(pipeline, query, space).expect("warmup query");
    }
    let mut samples: Vec<u64> = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        run_query(pipeline, query, space).expect("timed query");
        samples.push(start.elapsed().as_micros() as u64);
    }
    let mut last_metrics = ExplainMetrics::default();
    let table = format!("EXPLAIN ANALYZE {query}");
    if let Ok(graphdb::query::executor::base::ExecutionResult::DataSet { data, .. }) =
        pipeline.execute_query_with_space(&table, Some(space.clone()))
    {
        if let Some(row) = data.rows.first() {
            if let Some(Value::String(text)) = row.first() {
                last_metrics = parse_explain_table_header(text);
            }
        }
    }
    let dot = format!("EXPLAIN ANALYZE FORMAT = DOT {query}");
    if let Ok(graphdb::query::executor::base::ExecutionResult::DataSet { data, .. }) =
        pipeline.execute_query_with_space(&dot, Some(space.clone()))
    {
        if let Some(row) = data.rows.first() {
            if let Some(Value::String(text)) = row.first() {
                let dot_metrics = parse_explain_dot(text);
                last_metrics.storage_read_us = dot_metrics.storage_read_us;
                last_metrics.total_operator_us = dot_metrics.total_operator_us;
                last_metrics.root_total_us = dot_metrics.root_total_us;
            }
        }
    }
    (median_us(&samples), last_metrics)
}

fn main() {
    println!("== P3.2 parallel speedup validation ==");
    println!("vertices={VERTEX_COUNT}, edges/vertex=3, iterations={ITERATIONS}");
    let storage = setup_data();
    println!(
        "data ready ({} vertices, {} edges)",
        VERTEX_COUNT,
        VERTEX_COUNT * 3
    );

    let space = space_info(&storage);

    for (q_name, query) in [
        ("Q1 scan+filter+agg", Q1),
        ("Q2 scan+groupby", Q2),
        ("Q3 2-hop traversal", Q3),
        ("E1 union multi-scan", E1),
        ("E2 edge scan", E2),
    ] {
        println!("\n### {q_name}: {query}");
        println!(
            "{:>7} | {:>6} | {:>7} | {:>8} | {:>7} | {:>8} | {:>7} | {:>7} | fallback / storage R",
            "workers", "actual", "T(1)/T(n)", "T(n) us", "E(n)", "work us", "wall us", "eta"
        );
        let mut medians: Vec<u64> = Vec::new();
        let mut metrics_list: Vec<ExplainMetrics> = Vec::new();
        for &workers in &WORKERS {
            let mut pipeline = build_pipeline(&storage, workers);
            let (median, metrics) = measure(&mut pipeline, query, &space);
            medians.push(median);
            metrics_list.push(metrics);
        }
        for (idx, &workers) in WORKERS.iter().enumerate() {
            let t1 = medians[0] as f64;
            let tn = medians[idx] as f64;
            let speedup = t1 / tn;
            let m = &metrics_list[idx];
            let eta = if m.parallel_wall_us > 0 {
                m.parallel_work_us as f64 / m.parallel_wall_us as f64
            } else {
                0.0
            };
            let r = if m.root_total_us > 0 {
                m.storage_read_us as f64 / m.root_total_us as f64
            } else if m.total_operator_us > 0 {
                m.storage_read_us as f64 / m.total_operator_us as f64
            } else {
                0.0
            };
            let extra = if m.fallback_reason.is_empty() {
                format!("storage R={:.0}%", r * 100.0)
            } else {
                format!("fallback=\"{}\"", m.fallback_reason)
            };
            println!(
                "{:>7} | {:>6} | {:>7.2} | {:>8} | {:>7.2} | {:>8} | {:>8} | {:>7.2} | {}",
                workers,
                m.actual_workers,
                t1 / tn,
                medians[idx],
                speedup,
                m.parallel_work_us,
                m.parallel_wall_us,
                eta,
                extra
            );
        }
    }
    let _ = space;
}
