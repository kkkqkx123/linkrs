//! End-to-end validation of parallel execution wiring:
//!
//! With `max_workers > 1` and a complete partitioning configuration, a
//! tagged vertex scan query must be decomposed into N partition fragments +
//! one exchange fragment, execute on the morsel worker pool, and produce
//! results identical to the serial path.  `EXPLAIN ANALYZE` must report
//! `actual_workers = min(partitions, workers)` and an empty fallback reason.

#![allow(clippy::arc_with_non_send_sync)]

mod common;

use common::TestStorage;

use graphdb_query::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb_query::core::vertex_edge_path::Tag;
use graphdb_query::core::{DataType, Edge, StatsManager, Value, Vertex};
use graphdb_query::query::executor::base::ExecutionResult;
use graphdb_query::query::optimizer::{OptimizerEngine, PartitioningConfig};
use graphdb_query::query::pipeline::QueryPipelineManager;
use graphdb_query::storage::{GraphStorage, StorageSchemaOps, StorageWriter};
use parking_lot::RwLock;
use std::sync::Arc;

const SPACE: &str = "pp";
const TAG: &str = "Node";
const TAG2: &str = "Other";
const EDGE: &str = "Link";
const VERTEX_COUNT: i64 = 2000;

/// Insert `VERTEX_COUNT` vertices with ids 0..VERTEX_COUNT; property `value`
/// equals the vertex id and `group_id` is `id % 20`.
fn insert_vertices(storage: &Arc<RwLock<GraphStorage>>) {
    let mut start = 0i64;
    while start < VERTEX_COUNT {
        let end = (start + 500).min(VERTEX_COUNT);
        let vertices: Vec<Vertex> = (start..end)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64(i),
                    vec![Tag::new(
                        TAG.to_string(),
                        vec![
                            ("value".to_string(), Value::BigInt(i)),
                            ("group_id".to_string(), Value::BigInt(i % 20)),
                        ]
                        .into_iter()
                        .collect(),
                    )],
                )
            })
            .collect();
        storage
            .write()
            .batch_insert_vertices(SPACE, vertices)
            .expect("insert vertices");
        start = end;
    }
}

/// Insert `Link` edges: each vertex `i` links to `i+1`, `i+2` (in-range).
fn insert_edges(storage: &Arc<RwLock<GraphStorage>>) {
    let mut edges = Vec::with_capacity((VERTEX_COUNT * 2) as usize);
    for src in 0..VERTEX_COUNT {
        for k in 1..=2i64 {
            let dst = (src + k) % VERTEX_COUNT;
            edges.push(Edge {
                src: VertexId::from_int64(src),
                dst: VertexId::from_int64(dst),
                edge_type: EDGE.to_string(),
                ranking: 0,
                props: Default::default(),
            });
        }
    }
    for chunk in edges.chunks(10_000) {
        storage
            .write()
            .batch_insert_edges(SPACE, chunk.to_vec())
            .expect("insert edges");
    }
}

fn setup_storage() -> Arc<RwLock<GraphStorage>> {
    let storage = TestStorage::new().expect("storage init").storage();
    {
        let mut guard = storage.write();
        let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
        guard.create_space(&mut space).expect("create space");
        guard
            .create_tag(
                SPACE,
                &TagInfo::new(TAG.to_string()).with_properties(vec![
                    PropertyDef::new("value".to_string(), DataType::BigInt),
                    PropertyDef::new("group_id".to_string(), DataType::BigInt),
                ]),
            )
            .expect("create tag");
        guard
            .create_tag(
                SPACE,
                &TagInfo::new(TAG2.to_string()).with_properties(vec![
                    PropertyDef::new("value".to_string(), DataType::BigInt),
                ]),
            )
            .expect("create tag 2");
        guard
            .create_edge_type(
                SPACE,
                &EdgeTypeInfo::new(EDGE.to_string())
                    .with_src_tag(TAG.to_string())
                    .with_dst_tag(TAG.to_string()),
            )
            .expect("create edge type");
    }
    insert_vertices(&storage);
    {
        // Second tag shares the same vertex-id domain.
        let mut start = 0i64;
        while start < VERTEX_COUNT {
            let end = (start + 500).min(VERTEX_COUNT);
            let vertices: Vec<Vertex> = (start..end)
                .map(|i| {
                    Vertex::new(
                        VertexId::from_int64(i),
                        vec![Tag::new(
                            TAG2.to_string(),
                            vec![("value".to_string(), Value::BigInt(i + 1000))]
                                .into_iter()
                                .collect(),
                        )],
                    )
                })
                .collect();
            storage
                .write()
                .batch_insert_vertices(SPACE, vertices)
                .expect("insert tag2 vertices");
            start = end;
        }
    }
    insert_edges(&storage);
    storage
}

fn build_pipeline(
    storage: &Arc<RwLock<GraphStorage>>,
    workers: usize,
) -> QueryPipelineManager<GraphStorage> {
    let mut engine = OptimizerEngine::default();
    if workers > 1 {
        engine.set_partitioning_config(PartitioningConfig {
            enabled: true,
            min_rows_per_partition: 400,
            max_partitions: workers * 2,
            vertex_id_range: Some(0i64..VERTEX_COUNT),
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

fn space_info() -> SpaceInfo {
    let mut info = SpaceInfo::new(SPACE.to_string());
    info.space_id = 1;
    info
}

fn query_rows(pipeline: &mut QueryPipelineManager<GraphStorage>, query: &str) -> Vec<Vec<Value>> {
    let space = space_info();
    match pipeline
        .execute_query_with_space(query, Some(space))
        .expect("query should succeed")
    {
        ExecutionResult::DataSet { data, .. } => data.rows,
        ExecutionResult::Empty => vec![],
        other => panic!("unexpected result: {:?}", other),
    }
}

fn sorted(rows: &mut [Vec<Value>]) {
    // Value has no stable total order for graph entities (Edge/Vertex); use a
    // deterministic debug representation as the sort key.
    rows.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
}

#[test]
fn partitioned_count_query_matches_serial_results() {
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1);
    let mut parallel = build_pipeline(&storage, 2);

    let q1 = "MATCH (n:Node) WHERE n.value < 800 RETURN count(n)";
    let serial_rows = query_rows(&mut serial, q1);
    let parallel_rows = query_rows(&mut parallel, q1);
    assert_eq!(serial_rows, parallel_rows, "Q1 (filter + count) mismatch");

    let q2 = "MATCH (n:Node) WHERE n.value >= 500 RETURN n.group_id, count(*)";
    let mut serial_rows = query_rows(&mut serial, q2);
    let mut parallel_rows = query_rows(&mut parallel, q2);
    sorted(&mut serial_rows);
    sorted(&mut parallel_rows);
    assert_eq!(serial_rows, parallel_rows, "Q2 (group-by count) mismatch");

    let q3 = "MATCH (n:Node) RETURN count(*)";
    let serial_rows = query_rows(&mut serial, q3);
    let parallel_rows = query_rows(&mut parallel, q3);
    assert_eq!(serial_rows, parallel_rows, "Q3 (plain count) mismatch");
}

#[test]
fn explain_analyze_reports_active_parallelism() {
    let storage = setup_storage();
    let mut parallel = build_pipeline(&storage, 2);

    let output = query_rows(
        &mut parallel,
        "EXPLAIN ANALYZE MATCH (n:Node) WHERE n.value < 800 RETURN count(n)",
    );
    assert_eq!(output.len(), 1, "EXPLAIN ANALYZE returns one plan string");
    let plan = output[0][0].to_string().unwrap_or_default();
    assert!(
        plan.contains("actual="),
        "EXPLAIN ANALYZE should report actual workers, got:\n{plan}"
    );
    assert!(
        !plan.contains("fallback_reason"),
        "partitioned plan must not report a fallback reason, got:\n{plan}"
    );
}

#[test]
fn explain_analyze_reports_fallback_reason_without_partitioning_config() {
    let storage = setup_storage();

    // max_workers=2 but partitioning disabled: the decision reason must be
    // surfaced by EXPLAIN ANALYZE instead of being silently dropped.
    let mut engine = OptimizerEngine::default();
    engine.set_partitioning_config(PartitioningConfig {
        enabled: false,
        max_workers: 2,
        ..PartitioningConfig::default()
    });
    let stats = Arc::new(StatsManager::new());
    let mut pipeline =
        QueryPipelineManager::with_optimizer(storage.clone(), stats, Arc::new(engine));
    pipeline
        .collect_statistics(SPACE, true)
        .expect("collect statistics");

    let output = query_rows(
        &mut pipeline,
        "EXPLAIN ANALYZE MATCH (n:Node) RETURN count(n)",
    );
    let plan = output[0][0].to_string().unwrap_or_default();
    assert!(
        plan.contains("fallback_reason"),
        "fallback reason must be visible, got:\n{plan}"
    );
}

#[test]
fn partitioned_edge_scan_matches_serial_results() {
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1);
    let mut parallel = build_pipeline(&storage, 2);

    // Pure edge-table chain: no src/dst vertex properties required.
    let q1 = "LOOKUP ON EDGE Link";
    let mut serial_rows = query_rows(&mut serial, q1);
    let mut parallel_rows = query_rows(&mut parallel, q1);
    sorted(&mut serial_rows);
    sorted(&mut parallel_rows);
    assert_eq!(serial_rows, parallel_rows, "E-Q1 (edge scan) mismatch");

    let q2 = "LOOKUP ON EDGE Link YIELD Link.src";
    let mut serial_rows = query_rows(&mut serial, q2);
    let mut parallel_rows = query_rows(&mut parallel, q2);
    sorted(&mut serial_rows);
    sorted(&mut parallel_rows);
    assert_eq!(serial_rows, parallel_rows, "E-Q2 (edge projection) mismatch");
}

#[test]
fn explain_analyze_edge_scan_reports_partition_shape() {
    let storage = setup_storage();
    let mut parallel = build_pipeline(&storage, 2);

    let output = query_rows(&mut parallel, "EXPLAIN ANALYZE LOOKUP ON EDGE Link");
    assert_eq!(output.len(), 1, "EXPLAIN ANALYZE returns one plan string");
    let plan = output[0][0].to_string().unwrap_or_default();
    assert!(
        plan.contains("Partitioning:"),
        "edge-scan plan must describe its partition spec, got:\n{plan}"
    );
    assert!(
        plan.contains("actual="),
        "edge-scan plan must report actual workers, got:\n{plan}"
    );
    assert!(
        !plan.contains("fallback_reason"),
        "partitioned edge-scan plan must not report a fallback reason, got:\n{plan}"
    );
    // The edge table must be split into multiple src-id ranges (exchange
    // contract): at least the requested worker count worth of partitions.
    let actual = plan
        .split("actual=")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(
        actual >= 2,
        "edge-scan plan must execute on at least 2 workers, got actual={actual}:\n{plan}"
    );
}

#[test]
fn anchored_hop_filter_is_pushed_below_expand() {
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1);
    let mut parallel = build_pipeline(&storage, 2);

    // Anchored 1-hop: the anchor predicate must be evaluated before expansion
    // (100 anchors x 2 edges = 200 matches) and results must match serial.
    let q = "MATCH (a:Node)-[:Link]->(b:Node) WHERE a.value < 100 RETURN count(b)";
    let serial_rows = query_rows(&mut serial, q);
    let parallel_rows = query_rows(&mut parallel, q);
    assert_eq!(serial_rows, parallel_rows, "anchored 1-hop mismatch");
    assert_eq!(
        serial_rows,
        vec![vec![Value::BigInt(200)]],
        "anchored 1-hop must expand only 100 anchors, got {serial_rows:?}"
    );

    // The anchor filter must appear below the ExpandAll in the physical plan:
    // the table lists operators scan-first, so a Filter between the anchor
    // scan and the expand indicates the predicate is evaluated pre-expansion.
    let output = query_rows(&mut serial, "EXPLAIN MATCH (a:Node)-[:Link]->(b:Node) WHERE a.value < 100 RETURN count(b)");
    let plan = output[0][0].to_string().unwrap_or_default();
    assert!(
        plan.contains("ExpandAll"),
        "plan must contain ExpandAll, got:\n{plan}"
    );
    let scan_pos = plan.find("StorageScan").expect("anchor scan");
    let filter_pos = plan.find("Filter").expect("pushed filter");
    let expand_pos = plan.find("ExpandAll").expect("expand");
    assert!(
        scan_pos < filter_pos && filter_pos < expand_pos,
        "anchor predicate must be evaluated before expansion, got:\n{plan}"
    );
}

#[test]
fn unanchored_two_hop_returns_correct_count() {
    let storage = setup_storage();
    let mut pipeline = build_pipeline(&storage, 1);

    let q = "MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)";
    let rows = query_rows(&mut pipeline, q);
    // 2000 anchors x 2 hops x 2 edges = 8000 paths; count(c) counts rows.
    assert_eq!(rows, vec![vec![Value::BigInt(8000)]]);
}

#[test]
fn partitioned_union_of_independent_scans_matches_serial_results() {
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1);
    let mut parallel = build_pipeline(&storage, 2);

    let q1 = "MATCH (a:Node) WHERE a.value < 800 RETURN a.value \
              UNION ALL \
              MATCH (b:Other) WHERE b.value >= 1000 RETURN b.value";
    let mut serial_rows = query_rows(&mut serial, q1);
    let mut parallel_rows = query_rows(&mut parallel, q1);
    sorted(&mut serial_rows);
    sorted(&mut parallel_rows);
    assert_eq!(serial_rows, parallel_rows, "U-Q1 (union all) mismatch");
}

#[test]
fn explain_analyze_union_reports_active_parallelism() {
    let storage = setup_storage();
    let mut parallel = build_pipeline(&storage, 2);

    let output = query_rows(
        &mut parallel,
        "EXPLAIN ANALYZE MATCH (a:Node) RETURN a.value \
         UNION ALL \
         MATCH (b:Other) RETURN b.value",
    );
    assert_eq!(output.len(), 1, "EXPLAIN ANALYZE returns one plan string");
    let plan = output[0][0].to_string().unwrap_or_default();
    assert!(
        plan.contains("actual="),
        "EXPLAIN ANALYZE should report actual workers, got:\n{plan}"
    );
    assert!(
        !plan.contains("fallback_reason"),
        "partitioned union plan must not report a fallback reason, got:\n{plan}"
    );
    // Both independent scans must actually run in parallel (actual = workers).
    let actual = plan
        .split("actual=")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(
        actual >= 2,
        "union plan must execute on at least 2 workers, got actual={actual}:\n{plan}"
    );
}

#[test]
fn partitioned_anchored_traversal_matches_serial_results() {
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1);
    let mut parallel = build_pipeline(&storage, 2);

    // anchored 1-hop partitions the anchor scan by vid range; each
    // partition expands locally and the global aggregate merges the counts.
    let q = "MATCH (a:Node)-[:Link]->(b:Node) WHERE a.value < 100 RETURN count(b)";
    let serial_rows = query_rows(&mut serial, q);
    let parallel_rows = query_rows(&mut parallel, q);
    assert_eq!(serial_rows, parallel_rows, "E4 anchored 1-hop mismatch");
    assert_eq!(serial_rows, vec![vec![Value::BigInt(200)]]);

    // Grouped aggregate across partitions must also match (b appears via
    // multiple anchors, so the final aggregate must re-merge partial groups).
    let q2 = "MATCH (a:Node)-[:Link]->(b:Node) WHERE a.value < 100 RETURN b.value, count(*)";
    let mut serial_rows = query_rows(&mut serial, q2);
    let mut parallel_rows = query_rows(&mut parallel, q2);
    sorted(&mut serial_rows);
    sorted(&mut parallel_rows);
    assert_eq!(serial_rows, parallel_rows, "E4 grouped aggregate mismatch");
    assert_eq!(serial_rows.len(), 101, "anchors 0..99 reach neighbors 1..101");

    // The traversing plan must actually run in parallel.
    let output = query_rows(
        &mut parallel,
        "EXPLAIN ANALYZE MATCH (a:Node)-[:Link]->(b:Node) WHERE a.value < 100 RETURN count(b)",
    );
    let plan = output[0][0].to_string().unwrap_or_default();
    let actual = plan
        .split("actual=")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(
        actual >= 2,
        "anchored traversal must execute on at least 2 workers, got actual={actual}:\n{plan}"
    );
}

#[test]
fn explain_analyze_vertex_scan_reports_partition_spec() {
    let storage = setup_storage();
    let mut parallel = build_pipeline(&storage, 4);

    let output = query_rows(
        &mut parallel,
        "EXPLAIN ANALYZE MATCH (n:Node) WHERE n.value < 800 RETURN n.value",
    );
    let plan = output[0][0].to_string().unwrap_or_default();

    // The partition spec must describe the vertex-id range partitioning.
    assert!(
        plan.contains("Partitioning:"),
        "vertex scan plan must describe partition spec, got:\n{plan}"
    );
    assert!(
        plan.contains("partitioned into"),
        "vertex scan plan must show partition count, got:\n{plan}"
    );
    assert!(
        plan.contains("ranges ["),
        "vertex scan plan must show ranges, got:\n{plan}"
    );
    assert!(
        plan.contains("actual="),
        "vertex scan plan must report actual workers, got:\n{plan}"
    );
    assert!(
        !plan.contains("fallback_reason"),
        "partitioned vertex scan must not fallback, got:\n{plan}"
    );

    // Verify actual workers >= 2 for 4-worker config.
    let actual = plan
        .split("actual=")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(
        actual >= 2,
        "vertex scan must use at least 2 workers, got actual={actual}:\n{plan}"
    );
}

#[test]
fn explain_analyze_union_plan_contains_exchange() {
    let storage = setup_storage();
    let mut parallel = build_pipeline(&storage, 2);

    let output = query_rows(
        &mut parallel,
        "EXPLAIN MATCH (a:Node) RETURN a.value \
         UNION ALL \
         MATCH (b:Other) RETURN b.value",
    );
    let plan = output[0][0].to_string().unwrap_or_default();

    // UNION ALL with two independent partitioned scans must include
    // an Exchange (concatenate) fragment to merge partition outputs.
    assert!(
        plan.contains("Exchange"),
        "union plan must contain Exchange node, got:\n{plan}"
    );
    assert!(
        plan.contains("Partitioning:"),
        "union plan must describe partition spec, got:\n{plan}"
    );
}

#[test]
fn explain_analyze_4_workers_vertex_scan_uses_all_workers() {
    let storage = setup_storage();
    let mut parallel = build_pipeline(&storage, 4);

    let output = query_rows(
        &mut parallel,
        "EXPLAIN ANALYZE MATCH (n:Node) RETURN count(n)",
    );
    let plan = output[0][0].to_string().unwrap_or_default();

    let actual = plan
        .split("actual=")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(
        actual >= 4,
        "4-worker config should use at least 4 workers for full scan, got actual={actual}:\n{plan}"
    );
}

#[test]
fn plan_cache_replans_on_partition_config_change() {
    let storage = setup_storage();
    let q = "MATCH (n:Node) WHERE n.value < 800 RETURN count(n)";

    // First execution with 2 workers: plans and caches.
    let mut pipeline_2w = build_pipeline(&storage, 2);
    let rows_2w = query_rows(&mut pipeline_2w, q);
    assert_eq!(rows_2w, vec![vec![Value::BigInt(800)]]);

    // Second execution with 4 workers: different planning_config_hash causes
    // a cache miss, triggering replan with the new partition config.
    let mut pipeline_4w = build_pipeline(&storage, 4);
    let rows_4w = query_rows(&mut pipeline_4w, q);
    assert_eq!(rows_4w, vec![vec![Value::BigInt(800)]]);

    // Both must produce identical results despite different partition layouts.
    assert_eq!(rows_2w, rows_4w, "cache replan must produce identical results");

    // Verify the 4-worker plan actually uses more workers.
    let output = query_rows(
        &mut pipeline_4w,
        "EXPLAIN ANALYZE MATCH (n:Node) WHERE n.value < 800 RETURN count(n)",
    );
    let plan = output[0][0].to_string().unwrap_or_default();
    let actual = plan
        .split("actual=")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(
        actual >= 2,
        "replanned 4-worker query must use parallel execution, got actual={actual}:\n{plan}"
    );
}

#[test]
fn plan_cache_partition_fingerprint_changes_with_ranges() {
    use graphdb_query::query::cache::plan_cache::PlanCacheKey;
    use graphdb_query::query::planning::plan::execution_plan::{PartitionSource, PartitionSpec};
    use std::ops::Range;

    // Same source and layout_version but different ranges → different key.
    let source = PartitionSource::VertexId {
        tag: "Node".to_string(),
    };
    let spec_a = PartitionSpec::try_new(
        vec![Range { start: 0, end: 500 }, Range { start: 500, end: 1000 }],
        source.clone(),
        Some(42),
    )
    .unwrap();
    let spec_b = PartitionSpec::try_new(
        vec![Range { start: 0, end: 250 }, Range { start: 250, end: 500 }, Range { start: 500, end: 750 }, Range { start: 750, end: 1000 }],
        source.clone(),
        Some(42),
    )
    .unwrap();

    let key_a = PlanCacheKey::from_query_with_partition("MATCH (n:Node) RETURN count(n)", &spec_a);
    let key_b = PlanCacheKey::from_query_with_partition("MATCH (n:Node) RETURN count(n)", &spec_b);
    assert_ne!(key_a, key_b, "different ranges must produce different cache keys");

    // Same ranges and layout_version → same key.
    let spec_c = PartitionSpec::try_new(
        vec![Range { start: 0, end: 500 }, Range { start: 500, end: 1000 }],
        source.clone(),
        Some(42),
    )
    .unwrap();
    let key_c = PlanCacheKey::from_query_with_partition("MATCH (n:Node) RETURN count(n)", &spec_c);
    assert_eq!(key_a, key_c, "same spec must produce same cache key");
}

#[test]
fn two_hop_id_only_seeds_feed_next_hop() {
    // A1.5: hop1 is annotated id_only (b feeds only the next hop), so hop2's
    // seeds come from hop1's `Value::VertexId` output column. The chain must
    // produce the same count as the full materialization path.
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1);
    let mut parallel = build_pipeline(&storage, 2);

    // 2000 anchors x 2 hops x 2 edges = 8000 paths; count(c) counts rows.
    let q = "MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)";
    let serial_rows = query_rows(&mut serial, q);
    let parallel_rows = query_rows(&mut parallel, q);
    assert_eq!(serial_rows, parallel_rows, "A1.5 2-hop id_only chain mismatch");
    assert_eq!(serial_rows, vec![vec![Value::BigInt(8000)]]);

    // The plan must annotate hop1 id_only and hop2 count_only.
    let output = query_rows(&mut serial, "EXPLAIN MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)");
    let plan = output[0][0].to_string().unwrap_or_default();
    let expand_pos = plan.find("ExpandAll").expect("expand");
    let info_after_first = &plan[expand_pos..];
    assert!(
        info_after_first.contains("mode:id_only"),
        "hop1 must be id_only, got:\n{plan}"
    );
    assert!(
        info_after_first.contains("mode:count_only"),
        "hop2 must be count_only, got:\n{plan}"
    );
}

#[test]
fn referenced_source_keeps_full_vertex() {
    // A source variable projected out (RETURN a.value) forces the hop to keep
    // the full vertex in its output; the de-materialized chain must not
    // lightweight it. Each of the 2000 anchors has 2 out-edges -> 4000 rows,
    // each carrying the anchor's `value` (= its id).
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1);
    let rows = query_rows(&mut serial, "MATCH (a:Node)-[:Link]->(b:Node) RETURN a.value");
    assert_eq!(rows.len(), 4000, "1-hop with projected source must return 4000 rows");
    let mut values: Vec<i64> = rows
        .iter()
        .map(|r| match &r[0] {
            Value::BigInt(v) => *v,
            other => panic!("unexpected source value: {other:?}"),
        })
        .collect();
    values.sort_unstable();
    let mut expected: Vec<i64> = (0..2000i64).flat_map(|i| [i, i]).collect();
    expected.sort_unstable();
    assert_eq!(values, expected, "projected source values must match the anchors");
}

#[test]
fn middle_var_referenced_blocks_id_only() {
    // When the middle variable is referenced by a property predicate, hop1
    // must NOT be annotated id_only (the predicate needs the real vertex).
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1);

    // b in {0..1999 with group_id < 10} = 1000 middles (group_id = id % 20),
    // each reached from 2 anchors and having 2 out edges -> 4000 paths.
    let q = "MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) WHERE b.group_id < 10 RETURN count(c)";
    let rows = query_rows(&mut serial, q);
    assert_eq!(
        rows,
        vec![vec![Value::BigInt(4000)]],
        "id_only must be blocked when b is referenced"
    );
}

#[test]
fn parallel_unanchored_two_hop_matches_serial() {
    // the de-materialized 2-hop chain partitions by anchor range; each
    // partition runs the full bounded traversal locally and the global
    // aggregate merges the per-partition counts.
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1);
    let mut parallel = build_pipeline(&storage, 2);

    let q = "MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)";
    let serial_rows = query_rows(&mut serial, q);
    let parallel_rows = query_rows(&mut parallel, q);
    assert_eq!(
        serial_rows,
        parallel_rows,
        "C1 2-hop parallel count mismatch"
    );
    assert_eq!(serial_rows, vec![vec![Value::BigInt(8000)]]);

    // The partitioned 2-hop must actually run on at least 2 workers.
    let output = query_rows(
        &mut parallel,
        "EXPLAIN ANALYZE MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)",
    );
    let plan = output[0][0].to_string().unwrap_or_default();
    assert!(
        plan.contains("Partitioning:"),
        "2-hop plan must carry a partition spec, got:\n{plan}"
    );
    let actual = plan
        .split("actual=")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(
        actual >= 2,
        "2-hop traversal must execute on at least 2 workers, got actual={actual}:\n{plan}"
    );
}

#[test]
fn partitioned_equality_join_matches_serial_results() {
    // Equality join between two tagged vertex scans (E1b). Results must be
    // identical to the serial path whether the join partitions directly or
    // falls back to the global gather-then-join path.
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1);
    let mut parallel = build_pipeline(&storage, 2);

    // Node.value == Other.value holds for exactly the 1000 vertices where
    // Other.vid is in [0, 1000) (Other.value = vid + 1000).
    let q = "MATCH (a:Node),(b:Other) WHERE a.value = b.value RETURN count(*)";
    let serial_rows = query_rows(&mut serial, q);
    let parallel_rows = query_rows(&mut parallel, q);
    assert_eq!(
        serial_rows,
        parallel_rows,
        "partitioned equality join mismatch"
    );
    assert_eq!(
        serial_rows,
        vec![vec![Value::BigInt(1000)]],
        "equality join must produce exactly 1000 matches, got {serial_rows:?}"
    );

    // When partitioning is active the plan must report real workers and no
    // fallback reason (a silent serial fallback would hide regressions).
    let output = query_rows(
        &mut parallel,
        "EXPLAIN ANALYZE MATCH (a:Node),(b:Other) WHERE a.value = b.value RETURN count(*)",
    );
    let plan = output[0][0].to_string().unwrap_or_default();
    if plan.contains("Partitioning:") {
        assert!(
            !plan.contains("fallback_reason"),
            "partitioned equality join must not report a fallback reason, got:\n{plan}"
        );
        assert!(
            plan.contains("actual="),
            "partitioned equality join must report actual workers, got:\n{plan}"
        );
    }
}
