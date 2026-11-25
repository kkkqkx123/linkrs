//! Partition boundary execution probe for the pure-count release shape.
//!
//! Planner unit tests pin where the row gate flips; this probe proves the
//! flip has an effect at execution time. The same storage population runs
//! each count three ways: serial, partitioned above the gate (parallel
//! fragments must engage and match), and partitioned below the gate (must
//! fall back with an observable reason and still match). Equality alone
//! would pass vacuously on a silent fallback, so every partitioned run
//! also asserts its engagement state.

mod common;

use common::TestStorage;

use linkrs_core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Value, Vertex};
use linkrs_metrics::StatsManager;
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::optimizer::{OptimizerEngine, PartitioningConfig};
use linkrs_query::pipeline::QueryPipelineManager;
use linkrs_query::storage::{GraphStorage, StorageSchemaOps, StorageWriter};
use parking_lot::RwLock;
use std::sync::Arc;

const SPACE: &str = "ppb";
const TAG: &str = "Node";
const VERTEX_COUNT: i64 = 2000;

fn insert_vertices(storage: &Arc<RwLock<GraphStorage>>) {
    let mut start = 0i64;
    while start < VERTEX_COUNT {
        let end = (start + 500).min(VERTEX_COUNT);
        let vertices: Vec<Vertex> = (start..end)
            .map(|i| {
                Vertex::new(
                    VertexId::try_from_int64(i).expect("test vertex id"),
                    Tag::new(
                        TAG.to_string(),
                        vec![
                            ("id".to_string(), Value::BigInt(i)),
                            ("value".to_string(), Value::BigInt(i)),
                            ("group_id".to_string(), Value::BigInt(i % 20)),
                        ]
                        .into_iter()
                        .map(|(name, value)| (Arc::from(name.as_str()), value))
                        .collect(),
                    ),
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
                    PropertyDef::new("id".to_string(), DataType::BigInt),
                    PropertyDef::new("value".to_string(), DataType::BigInt),
                    PropertyDef::new("group_id".to_string(), DataType::BigInt),
                ]),
            )
            .expect("create tag");
    }
    insert_vertices(&storage);
    storage
}

fn build_pipeline(
    storage: &Arc<RwLock<GraphStorage>>,
    workers: usize,
    min_rows_per_partition: u64,
) -> QueryPipelineManager<GraphStorage> {
    let mut engine = OptimizerEngine::default();
    if workers > 1 {
        engine.set_partitioning_config(PartitioningConfig {
            enabled: true,
            min_rows_per_partition,
            max_partitions: workers * 2,
            vertex_id_range: Some(0i64..VERTEX_COUNT),
            max_workers: workers,
            max_buffered_chunks: 10,
            fixed_overhead_rows: 0,
            per_partition_rows: 0,
            exchange_row_per_mille: 0,
            allow_pure_count_chain: false,
            allow_global_aggregate: false,
            allow_ordered_parallel_expand: false,
            allow_parallel_count_frontier: false,
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
    rows.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
}

fn plan_string(pipeline: &mut QueryPipelineManager<GraphStorage>, query: &str) -> String {
    let output = query_rows(pipeline, &format!("EXPLAIN ANALYZE {query}"));
    assert_eq!(output.len(), 1, "EXPLAIN ANALYZE returns one plan string");
    output[0][0].to_string().unwrap_or_default()
}

fn active_workers(plan: &str) -> usize {
    plan.split("actual=")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0)
}

const PURE_COUNT: &str = "MATCH (n:Node) RETURN count(*)";
const FILTERED_COUNT: &str = "MATCH (n:Node) WHERE n.value < 800 RETURN count(n)";
const GROUPED_COUNT: &str = "MATCH (n:Node) RETURN n.group_id, count(*)";

#[test]
fn partitioned_pure_count_engages_workers_and_matches_serial() {
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1, 400);
    let mut parallel = build_pipeline(&storage, 2, 400);

    let serial_rows = query_rows(&mut serial, PURE_COUNT);
    let parallel_rows = query_rows(&mut parallel, PURE_COUNT);
    assert_eq!(serial_rows, parallel_rows, "pure count mismatch");
    assert!(
        serial_rows[0].contains(&Value::BigInt(VERTEX_COUNT)),
        "pure count must equal inserted vertices, got {:?}",
        serial_rows[0]
    );

    let plan = plan_string(&mut parallel, PURE_COUNT);
    assert!(
        !plan.contains("fallback_reason"),
        "above-gate release shape must not fall back, got:\n{plan}"
    );
    let actual = active_workers(&plan);
    assert!(
        actual >= 2,
        "above-gate pure count must execute on at least 2 workers, got actual={actual}:\n{plan}"
    );
}

#[test]
fn below_threshold_config_falls_back_with_equal_counts() {
    // Same population, only the gate moves: min_rows 5000 puts the 2000-row
    // table ten times under the threshold, so every partitioned run must
    // fall back and still agree with serial execution.
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1, 400);
    let mut below_gate = build_pipeline(&storage, 2, 5_000);

    for query in [PURE_COUNT, FILTERED_COUNT] {
        let serial_rows = query_rows(&mut serial, query);
        let gated_rows = query_rows(&mut below_gate, query);
        assert_eq!(serial_rows, gated_rows, "below-gate mismatch for {query}");
    }
    let mut serial_grouped = query_rows(&mut serial, GROUPED_COUNT);
    let mut gated_grouped = query_rows(&mut below_gate, GROUPED_COUNT);
    sorted(&mut serial_grouped);
    sorted(&mut gated_grouped);
    assert_eq!(serial_grouped, gated_grouped, "below-gate grouped mismatch");
    assert_eq!(serial_grouped.len(), 20, "twenty group_id buckets expected");

    let plan = plan_string(&mut below_gate, PURE_COUNT);
    assert!(
        plan.contains("fallback_reason"),
        "below-gate run must surface its fallback reason, got:\n{plan}"
    );
}

#[test]
fn threshold_change_flips_execution_path_with_stable_counts() {
    // One dataset, two partitioned pipelines differing only in the gate:
    // engagement flips while every count stays pinned.
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage, 1, 400);
    let mut above_gate = build_pipeline(&storage, 2, 400);
    let mut below_gate = build_pipeline(&storage, 2, 5_000);

    for query in [PURE_COUNT, FILTERED_COUNT] {
        let expected = query_rows(&mut serial, query);
        let above = query_rows(&mut above_gate, query);
        let below = query_rows(&mut below_gate, query);
        assert_eq!(expected, above, "above-gate mismatch for {query}");
        assert_eq!(expected, below, "below-gate mismatch for {query}");
    }

    let above_plan = plan_string(&mut above_gate, PURE_COUNT);
    let below_plan = plan_string(&mut below_gate, PURE_COUNT);
    assert!(
        active_workers(&above_plan) >= 2 && !above_plan.contains("fallback_reason"),
        "above-gate run must engage parallelism, got:\n{above_plan}"
    );
    assert!(
        below_plan.contains("fallback_reason"),
        "below-gate run must report fallback, got:\n{below_plan}"
    );
}
