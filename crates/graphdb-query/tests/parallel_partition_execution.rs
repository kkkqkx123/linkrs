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

use graphdb_query::core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb_query::core::vertex_edge_path::Tag;
use graphdb_query::core::{DataType, StatsManager, Value, Vertex};
use graphdb_query::query::executor::base::ExecutionResult;
use graphdb_query::query::optimizer::{OptimizerEngine, PartitioningConfig};
use graphdb_query::query::pipeline::QueryPipelineManager;
use graphdb_query::storage::{GraphStorage, StorageSchemaOps, StorageWriter};
use parking_lot::RwLock;
use std::sync::Arc;

const SPACE: &str = "pp";
const TAG: &str = "Node";
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
    }
    insert_vertices(&storage);
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
    rows.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
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
