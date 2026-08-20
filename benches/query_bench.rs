use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use graphdb::core::stats::StatsManager;
use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::query::executor::streaming::runtime::{
    ColumnarStatsSnapshot, D1_EVAL_THRESHOLD, D1_TYPED_RATE_THRESHOLD,
};
use graphdb::query::optimizer::OptimizerEngine;
use graphdb::query::pipeline::QueryPipelineManager;
use graphdb::query::QueryRequestContext;
use graphdb::storage::{
    GraphStorage, ScanOptions, StorageReader, StorageSchemaContextOps, StorageSchemaOps,
    StorageWriter,
};
use parking_lot::RwLock;
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn create_benchmark_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut group = c.benchmark_group(name);
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(1));
    group
}

fn setup_graph(vertex_count: usize, edges_per_vertex: usize) -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let space_name = format!("bench_q{}e{}", vertex_count, edges_per_vertex);
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

    for i in 0..vertex_count {
        let vertex = Vertex::new(
            VertexId::from_string(format!("n{}", i)),
            vec![Tag::new(
                "Node".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("node_{}", i))),
                    ("value".to_string(), Value::Double(i as f64 * 0.1)),
                ]
                .into_iter()
                .collect(),
            )],
        );
        storage
            .insert_vertex(&space_name, vertex)
            .expect("insert vertex");
    }

    for src in 0..vertex_count {
        for k in 1..=edges_per_vertex.min(vertex_count - 1) {
            let dst = (src + k) % vertex_count;
            let edge = Edge {
                src: VertexId::from_string(format!("n{}", src)),
                dst: VertexId::from_string(format!("n{}", dst)),
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

fn setup_large_graph(vertex_count: u64, edges_per_vertex: usize) -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let space_name = format!("large_q{}e{}", vertex_count, edges_per_vertex);
    let mut space = SpaceInfo::new(space_name.clone()).with_vid_type(DataType::BigInt);
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

    for i in 0..vertex_count as i64 {
        let vertex = Vertex::new(
            VertexId::from_int64(i),
            vec![Tag::new(
                "Node".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("node_{}", i))),
                    ("value".to_string(), Value::Double(i as f64 * 0.1)),
                ]
                .into_iter()
                .collect(),
            )],
        );
        storage
            .insert_vertex(&space_name, vertex)
            .expect("insert vertex");
    }

    let max_edges = edges_per_vertex.min((vertex_count as usize).saturating_sub(1));
    for src in 0..vertex_count as i64 {
        for k in 1..=max_edges as i64 {
            let dst = (src + k) % vertex_count as i64;
            let edge = Edge {
                src: VertexId::from_int64(src),
                dst: VertexId::from_int64(dst),
                edge_type: "Link".to_string(),
                ranking: 0,
                props: HashMap::new(),
            };
            storage.insert_edge(&space_name, edge).expect("insert edge");
        }
    }

    storage
}

fn bench_simple_query_parse(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "query_parse");
    let storage = setup_graph(100, 3);

    group.bench_function("parse_simple_vertex_query", |b| {
        b.iter(|| {
            let _ = storage.get_vertex("bench_q100e3", &VertexId::from_string("n1"));
        });
    });

    group.bench_function("parse_simple_edge_query", |b| {
        b.iter(|| {
            let _ = storage.get_vertex("bench_q100e3", &VertexId::from_string("n1"));
        });
    });

    group.finish();
}

fn bench_query_data_access(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "query_data_access");

    for vertex_count in &[100, 1000] {
        let storage = setup_graph(*vertex_count, 3);

        group.bench_function(format!("scan_{}", vertex_count), |b| {
            b.iter(|| {
                let _ = storage.get_vertex(
                    &format!("bench_q{}e3", vertex_count),
                    &VertexId::from_string("n1"),
                );
            });
        });
    }

    group.finish();
}

fn bench_path_traversal(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "path_traversal");
    let storage = setup_graph(200, 5);

    for hop_count in &[2usize, 3] {
        group.bench_function(format!("{}_hop", hop_count), |b| {
            b.iter(|| {
                let _ = storage.get_vertex("bench_q200e5", &VertexId::from_string("n1"));
            });
        });
    }

    group.finish();
}

fn bench_aggregation_queries(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "aggregation");
    let storage = setup_graph(500, 3);

    group.bench_function("scan_edges_by_type", |b| {
        b.iter(|| {
            let _ = storage.scan_edges_by_type("bench_q500e3", "Link");
        });
    });

    group.bench_function("get_vertex", |b| {
        b.iter(|| {
            let _ = storage.get_vertex("bench_q500e3", &VertexId::from_string("n1"));
        });
    });

    group.finish();
}

fn bench_large_vertex_scan(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "large_vertex_scan");
    for &vertex_count in &[10_000u64, 100_000] {
        let space_name = format!("large_q{}e3", vertex_count);
        let storage = setup_large_graph(vertex_count, 3);
        group.throughput(Throughput::Elements(vertex_count));
        group.bench_function(BenchmarkId::from_parameter(vertex_count), |b| {
            b.iter(|| {
                let mut cursor = storage
                    .create_vertex_cursor(
                        &space_name,
                        &ScanOptions::new()
                            .with_offset(0)
                            .with_limit(vertex_count as usize),
                    )
                    .expect("vertex cursor");
                let mut count = 0usize;
                while !cursor.next_batch(256).expect("cursor batch").is_empty() {
                    count += 1;
                }
                black_box(count);
            });
        });
    }
    group.finish();
}

fn bench_large_count_operations(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "large_count");
    for &vertex_count in &[10_000u64, 100_000] {
        let space_name = format!("large_q{}e3", vertex_count);
        let storage = setup_large_graph(vertex_count, 3);
        group.bench_function(BenchmarkId::new("count_vertices", vertex_count), |b| {
            b.iter(|| {
                let n = storage
                    .count_vertices_by_tag(&space_name, "Node")
                    .expect("count");
                black_box(n);
            });
        });
        group.bench_function(BenchmarkId::new("count_edges", vertex_count), |b| {
            b.iter(|| {
                let n = storage
                    .count_edges_by_type(&space_name, "Link")
                    .expect("count");
                black_box(n);
            });
        });
    }
    group.finish();
}

fn bench_large_edge_density(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "large_edge_density");
    for &(label, edges_per_vertex) in &[("sparse_1k_x3", 3usize), ("dense_1k_x50", 50)] {
        let space_name = format!("large_q{}e{}", 1_000u64, edges_per_vertex);
        let storage = setup_large_graph(1_000, edges_per_vertex);
        let expected_total_edges = 1_000 * edges_per_vertex;
        group.throughput(Throughput::Elements(expected_total_edges as u64));
        group.bench_function(format!("scan_edges_{}", label), |b| {
            b.iter(|| {
                let edges = storage
                    .scan_edges_by_type(&space_name, "Link")
                    .expect("scan");
                black_box(edges.len());
            });
        });
        group.bench_function(format!("cursor_scan_edges_{}", label), |b| {
            b.iter(|| {
                let options = ScanOptions::new().with_edge_type("Link".to_string());
                let mut cursor = storage
                    .create_edge_cursor(&space_name, &options)
                    .expect("edge cursor");
                let mut count = 0usize;
                while let Ok(batch) = cursor.next_batch(256) {
                    if batch.is_empty() {
                        break;
                    }
                    count += batch.len();
                }
                black_box(count);
            });
        });
        group.bench_function(format!("get_node_edges_{}", label), |b| {
            b.iter(|| {
                let edges = storage
                    .get_node_edges(
                        &space_name,
                        &VertexId::from_int64(0),
                        graphdb_storage::core::EdgeDirection::Out,
                    )
                    .expect("get edges");
                black_box(edges.len());
            });
        });
    }
    group.finish();
}

// ═══════════════════════════════════════════════════════════════════════════
// Columnar typed-column benchmark set
//
// Each group runs a real query through the query engine and aggregates the
// runtime observability counters:
//   - ColumnarStats (hit_rate / typed_hit_rate / selection counters)
//   - per-operator peak_memory_bytes, spill_count, spilled_bytes
//
// Every group emits one machine-readable line per benchmark:
//   BENCHMARK_RESULT {"benchmark":"B1", ...}
// Redirect stdout (or set COLUMNAR_BENCH_OUT) to collect the data rows.

const BASE_DEC_VERTICES: u64 = 20_000;
const DEC_EDGES_PER_VERTEX: usize = 3;

fn dec_vertices() -> u64 {
    static SCALE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *SCALE.get_or_init(|| {
        env::var("COLUMNAR_BENCH_SCALE")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(1)
            .max(1)
            * BASE_DEC_VERTICES
    })
}

fn dec_space() -> String {
    format!("dec_q{}e{}", dec_vertices(), DEC_EDGES_PER_VERTEX)
}

/// Aggregates runtime observability across all criterion iterations of a
/// benchmark, so the reported counters reflect a large number of runs.
#[derive(Default)]
struct Accum {
    runs: AtomicU64,
    columnar_hits: AtomicU64,
    columnar_misses: AtomicU64,
    typed_hits: AtomicU64,
    selection_attached: AtomicU64,
    selection_materialized: AtomicU64,
    selection_pushed: AtomicU64,
    column_block_hits: AtomicU64,
    peak_memory_bytes: AtomicU64,
    spill_count: AtomicU64,
    spilled_bytes: AtomicU64,
    output_rows: AtomicU64,
}

impl Accum {
    fn record(&self, stats: &ColumnarStatsSnapshot, collector: &ProfileCollectorView) {
        self.runs.fetch_add(1, Ordering::Relaxed);
        self.columnar_hits
            .fetch_add(stats.columnar_hits, Ordering::Relaxed);
        self.columnar_misses
            .fetch_add(stats.columnar_misses, Ordering::Relaxed);
        self.typed_hits
            .fetch_add(stats.columnar_typed_hits, Ordering::Relaxed);
        self.selection_attached
            .fetch_add(stats.selection_attached, Ordering::Relaxed);
        self.selection_materialized
            .fetch_add(stats.selection_materialized, Ordering::Relaxed);
        self.selection_pushed
            .fetch_add(stats.selection_pushed, Ordering::Relaxed);
        self.column_block_hits
            .fetch_add(stats.column_block_hits, Ordering::Relaxed);
        self.peak_memory_bytes
            .fetch_max(collector.peak_memory_bytes, Ordering::Relaxed);
        self.spill_count
            .fetch_add(collector.spill_count, Ordering::Relaxed);
        self.spilled_bytes
            .fetch_add(collector.spilled_bytes, Ordering::Relaxed);
        self.output_rows
            .fetch_add(collector.output_rows, Ordering::Relaxed);
    }

    /// Emit one machine-readable JSON line per benchmark run.
    fn emit(&self, benchmark: &str, name: &str) {
        let runs = self.runs.load(Ordering::Relaxed);
        if runs == 0 {
            return;
        }
        let hits = self.columnar_hits.load(Ordering::Relaxed);
        let misses = self.columnar_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total == 0 {
            1.0
        } else {
            hits as f64 / total as f64
        };
        let typed_hit_rate = if hits == 0 {
            0.0
        } else {
            self.typed_hits.load(Ordering::Relaxed) as f64 / hits as f64
        };
        let record = serde_json::json!({
            "benchmark": benchmark,
            "name": name,
            "runs": runs,
            "columnar_hits": hits,
            "columnar_misses": misses,
            "hit_rate": hit_rate,
            "typed_hit_rate": typed_hit_rate,
            "selection_attached": self.selection_attached.load(Ordering::Relaxed),
            "selection_materialized": self.selection_materialized.load(Ordering::Relaxed),
            "selection_pushed": self.selection_pushed.load(Ordering::Relaxed),
            "column_block_hits": self.column_block_hits.load(Ordering::Relaxed),
            "peak_memory_bytes": self.peak_memory_bytes.load(Ordering::Relaxed),
            "spill_count": self.spill_count.load(Ordering::Relaxed),
            "spilled_bytes": self.spilled_bytes.load(Ordering::Relaxed),
            "output_rows": self.output_rows.load(Ordering::Relaxed),
        });
        let line = format!(
            "BENCHMARK_RESULT {}",
            serde_json::to_string(&record).expect("json")
        );
        println!("{}", line);
        if let Some(path) = std::env::var_os("COLUMNAR_BENCH_OUT") {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(file, "{}", line);
            }
        }

        // Emit a status line with columnar evaluation observability.
        let d1_holds = total > D1_EVAL_THRESHOLD && typed_hit_rate < D1_TYPED_RATE_THRESHOLD;
        let d1_status = serde_json::json!({
            "benchmark": benchmark,
            "name": name,
            "evals": total,
            "threshold": D1_EVAL_THRESHOLD,
            "typed_rate": typed_hit_rate,
            "d1_holds": d1_holds,
        });
        println!(
            "D1_STATUS {}",
            serde_json::to_string(&d1_status).expect("json")
        );

        // Emit a spill-proxy line with memory observability.
        let output_rows = self.output_rows.load(Ordering::Relaxed);
        let spill_count = self.spill_count.load(Ordering::Relaxed);
        let spill_rate = if output_rows > 0 {
            spill_count as f64 / output_rows as f64
        } else {
            0.0
        };
        let d2_proxy = serde_json::json!({
            "benchmark": benchmark,
            "name": name,
            "peak_memory_bytes": self.peak_memory_bytes.load(Ordering::Relaxed),
            "output_rows": output_rows,
            "spill_count": spill_count,
            "spill_rate": spill_rate,
        });
        println!(
            "D2_PROXY {}",
            serde_json::to_string(&d2_proxy).expect("json")
        );
    }
}

/// Aggregated profile view extracted from the execution runtime.
struct ProfileCollectorView {
    peak_memory_bytes: u64,
    spill_count: u64,
    spilled_bytes: u64,
    output_rows: u64,
}

/// Setup the query-engine benchmark graph (BigInt vids, Node tag with
/// name/value, Link edges with weight).
fn setup_query_graph() -> Arc<RwLock<GraphStorage>> {
    let mut storage = GraphStorage::new().expect("storage init");
    let space_name = dec_space();
    let mut space = SpaceInfo::new(space_name.clone()).with_vid_type(DataType::BigInt);
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
    for i in 0..dec_vertices() as i64 {
        let vertex = Vertex::new(
            VertexId::from_int64(i),
            vec![Tag::new(
                "Node".to_string(),
                vec![
                    ("name".to_string(), Value::string(format!("node_{}", i))),
                    ("value".to_string(), Value::Double(i as f64 * 0.1)),
                ]
                .into_iter()
                .collect(),
            )],
        );
        storage
            .insert_vertex(&space_name, vertex)
            .expect("insert vertex");
    }
    let max_edges = DEC_EDGES_PER_VERTEX.min((dec_vertices() as usize).saturating_sub(1));
    for src in 0..dec_vertices() as i64 {
        for k in 1..=max_edges as i64 {
            let dst = (src + k) % dec_vertices() as i64;
            let edge = Edge {
                src: VertexId::from_int64(src),
                dst: VertexId::from_int64(dst),
                edge_type: "Link".to_string(),
                ranking: 0,
                props: [("weight".to_string(), Value::Double(1.0 / k as f64))]
                    .into_iter()
                    .collect(),
            };
            storage.insert_edge(&space_name, edge).expect("insert edge");
        }
    }
    Arc::new(RwLock::new(storage))
}

/// Build a query pipeline bound to the benchmark graph.
fn setup_query_pipeline(
    storage: Arc<RwLock<GraphStorage>>,
) -> (QueryPipelineManager<GraphStorage>, SpaceInfo) {
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
        let space_name = dec_space();
        guard
            .get_space(&space_name)
            .expect("space")
            .expect("space info")
    };
    (pipeline, space_info)
}

/// Execute one query through the pipeline and record its observability into
/// the accumulator.
fn run_query_and_accumulate(
    pipeline: &mut QueryPipelineManager<GraphStorage>,
    query: &str,
    space: &SpaceInfo,
    accum: &Accum,
) {
    let rctx = Arc::new(QueryRequestContext::new(query.to_string()));
    let result = pipeline
        .execute_query_stream_with_request(query, rctx, Some(space.clone()))
        .expect("query should succeed");
    while let Ok(Some(chunk)) = result.next_chunk() {
        black_box(chunk.len());
    }
    result.close().ok();
    let stats = ColumnarStatsSnapshot::from_stats(&result.runtime().columnar_stats());
    let collector = result.runtime().profile().flush_to_collector();
    let view = ProfileCollectorView {
        peak_memory_bytes: collector
            .operators
            .values()
            .map(|p| p.peak_memory_bytes)
            .max()
            .unwrap_or(0),
        spill_count: collector.operators.values().map(|p| p.spill_count).sum(),
        spilled_bytes: collector.operators.values().map(|p| p.spilled_bytes).sum(),
        output_rows: collector.operators.values().map(|p| p.output_rows).sum(),
    };
    accum.record(&stats, &view);
}

/// B1: scan + filter (large vertex scan with a pushdown predicate).
fn bench_b1_scan_filter(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "B1_scan_filter");
    let (mut pipeline, space) = setup_query_pipeline(setup_query_graph());
    let accum = RefCell::new(Accum::default());
    let query = "MATCH (n:Node) WHERE n.value > 1000.0 RETURN n.name";
    group.bench_function("scan_filter_20k", |b| {
        b.iter(|| run_query_and_accumulate(&mut pipeline, query, &space, &accum.borrow()))
    });
    group.finish();
    accum.borrow().emit("B1", "scan_filter_20k");
}

/// B2: index-style point lookup / neighborhood expansion.
fn bench_b2_point_lookup(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "B2_point_lookup");
    let (mut pipeline, space) = setup_query_pipeline(setup_query_graph());
    let accum = RefCell::new(Accum::default());
    let query = "MATCH (a:Node)-[r:Link]->(b:Node) WHERE id(a) == 42 RETURN b.name, r.weight";
    group.bench_function("point_lookup_neighbors", |b| {
        b.iter(|| run_query_and_accumulate(&mut pipeline, query, &space, &accum.borrow()))
    });
    group.finish();
    accum.borrow().emit("B2", "point_lookup_neighbors");
}

/// B3: aggregation (count / sum over the vertex scan).
fn bench_b3_aggregation(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "B3_aggregation");
    let (mut pipeline, space) = setup_query_pipeline(setup_query_graph());
    let count_accum = RefCell::new(Accum::default());
    let sum_accum = RefCell::new(Accum::default());
    let count_query = "MATCH (n:Node) RETURN count(n)";
    let sum_query = "MATCH (n:Node) RETURN sum(n.value)";
    group.bench_function("aggregate_count", |b| {
        b.iter(|| {
            run_query_and_accumulate(&mut pipeline, count_query, &space, &count_accum.borrow())
        })
    });
    group.bench_function("aggregate_sum", |b| {
        b.iter(|| run_query_and_accumulate(&mut pipeline, sum_query, &space, &sum_accum.borrow()))
    });
    group.finish();
    count_accum.borrow().emit("B3", "aggregate_count");
    sum_accum.borrow().emit("B3", "aggregate_sum");
}

/// B4: sort + TopN.
fn bench_b4_sort_topn(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "B4_sort_topn");
    let (mut pipeline, space) = setup_query_pipeline(setup_query_graph());
    let accum = RefCell::new(Accum::default());
    let query = "MATCH (n:Node) RETURN n.name, n.value ORDER BY n.value DESC LIMIT 10";
    group.bench_function("sort_limit_10", |b| {
        b.iter(|| run_query_and_accumulate(&mut pipeline, query, &space, &accum.borrow()))
    });
    group.finish();
    accum.borrow().emit("B4", "sort_limit_10");
}

/// B5: multi-hop path traversal (2-hop from a seed vertex).
fn bench_b5_path_traversal(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "B5_path_traversal");
    let (mut pipeline, space) = setup_query_pipeline(setup_query_graph());
    let accum = RefCell::new(Accum::default());
    let query =
        "MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) WHERE id(a) == 0 RETURN count(c)";
    group.bench_function("two_hop_count", |b| {
        b.iter(|| run_query_and_accumulate(&mut pipeline, query, &space, &accum.borrow()))
    });
    group.finish();
    accum.borrow().emit("B5", "two_hop_count");
}

/// B6: high edge density workload (full edge scan + count).
fn bench_b6_edge_density(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "B6_edge_density");
    let (mut pipeline, space) = setup_query_pipeline(setup_query_graph());
    let accum = RefCell::new(Accum::default());
    let query = "MATCH ()-[r:Link]->() RETURN count(r)";
    group.bench_function("edge_scan_count", |b| {
        b.iter(|| run_query_and_accumulate(&mut pipeline, query, &space, &accum.borrow()))
    });
    group.finish();
    accum.borrow().emit("B6", "edge_scan_count");
}

/// B7: mixed read/write (DML) scenario: insert -> read -> delete, cycling
/// over a rotating id range to avoid key collisions across iterations.
fn bench_b7_mixed_rw(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "B7_mixed_rw");
    let (mut pipeline, space) = setup_query_pipeline(setup_query_graph());
    let accum = RefCell::new(Accum::default());
    let mut counter = 0u64;
    group.bench_function("dml_cycle", |b| {
        b.iter(|| {
            let vid = 100_000 + (counter % 1_000);
            counter += 1;
            let insert = format!(
                "INSERT VERTEX Node(name, value) VALUES {}:('tmp', 0.5)",
                vid
            );
            run_query_and_accumulate(&mut pipeline, &insert, &space, &accum.borrow());
            let read = "MATCH (n:Node) WHERE n.name == 'tmp' RETURN count(n)";
            run_query_and_accumulate(&mut pipeline, read, &space, &accum.borrow());
            let delete = format!("DELETE VERTEX {}", vid);
            run_query_and_accumulate(&mut pipeline, &delete, &space, &accum.borrow());
        })
    });
    group.finish();
    accum.borrow().emit("B7", "dml_cycle");
}

criterion_group!(
    benches,
    bench_simple_query_parse,
    bench_query_data_access,
    bench_path_traversal,
    bench_aggregation_queries,
    bench_large_vertex_scan,
    bench_large_count_operations,
    bench_large_edge_density,
    bench_b1_scan_filter,
    bench_b2_point_lookup,
    bench_b3_aggregation,
    bench_b4_sort_topn,
    bench_b5_path_traversal,
    bench_b6_edge_density,
    bench_b7_mixed_rw,
);
criterion_main!(benches);
